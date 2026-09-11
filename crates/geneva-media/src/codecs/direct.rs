use std::path::Path;

use ffmpeg_next::software::scaling;
use ffmpeg_next::util::format::Pixel;
use ffmpeg_next::util::frame;
use geneva_color::{Matrix, Range, ResolvedTags, Transfer};
use geneva_render::Frame;
use geneva_timeline::{Composition, Ratio};
use rayon::prelude::*;

use super::copy::{Place, base_video_clips, filling_video_clips};
use super::decode::VideoReader;
use super::encode::{format_of, pixel_of};
use super::{codec_error, ffi, init};
use crate::MediaError;
use crate::convert::{PlaneFormat, Planes, frame_to_planes};

/// Decoded frames of a composition that shows its video sources as they
/// are, or merely scaled to the output size, delivered in the encoder's
/// sample layout without passing through the compositing pipeline.
///
/// The composition must be one layer of video clips that fill the frame
/// (see the stream-copy rules) whose streams carry the same color tags as
/// the output. Frames already in the encoder's layout at the output size
/// are copied; otherwise they are scaled and repacked between YCbCr
/// layouts in their coded encoding, which is what a plain transcode does
/// and what a resize or a change of codec needs. A picture fitted onto a
/// larger frame of one opaque color (a landscape video on a portrait
/// canvas) is scaled to its place and laid onto that color; one that
/// covers the frame by cropping has the region it shows scaled to it.
///
/// An RGB output (PNG) is also served here from 8-bit YCbCr sources: the
/// scaler applies the source's matrix and range, and a per-channel table
/// re-encodes the source's transfer curve as the output's, which is what
/// the compositor computes for such a frame.
pub struct DirectSource {
    clips: Vec<DirectClip>,
    format: PlaneFormat,
    width: u32,
    height: u32,
    /// What happens to the decoded frames on the way to the encoder.
    how: Conversion,
    /// Whether frames are converted to RGB.
    to_rgb: bool,
    /// Whether the composition has layers above the video, which the
    /// caller composites onto the frames that show them.
    with_overlays: bool,
    /// The frame in the output layout cleared to the background, for
    /// clips that do not cover it; `None` when every clip does.
    bars: Option<Planes>,
}

/// How decoded frames reach the encoder's layout and size.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Conversion {
    /// Copied as they are.
    AsIs,
    /// Scaled or repacked.
    Scaled,
    /// One region scaled to the frame.
    Cropped,
}

struct DirectClip {
    start: Ratio,
    end: Ratio,
    in_: Ratio,
    /// How the picture is placed when scaling alone does not do it.
    place: Option<Place>,
    reader: VideoReader,
    converter: Converter,
}

/// The conversion of one clip's decoded frames to the output layout.
struct Converter {
    /// Converter from the decoder's frames to the output layout and size,
    /// built from the first frame; `None` while frames can be copied.
    scaler: Option<ffi::ThreadedScaler>,
    /// Input layout the scaler was built for.
    scaler_input: Option<(Pixel, u32, u32)>,
    scratch: frame::Video,
    /// Source matrix and range, for a conversion to RGB.
    matrix: Matrix,
    range: Range,
    /// Re-encodes RGB samples from the source's transfer curve to the
    /// output's; `None` when the two are the same.
    transfer_lut: Option<Box<[u8; 256]>>,
}

/// The per-channel table from one transfer curve to another.
fn transfer_lut(from: Transfer, to: Transfer) -> Option<Box<[u8; 256]>> {
    if from == to {
        return None;
    }
    let mut lut = Box::new([0u8; 256]);
    for (v, out) in lut.iter_mut().enumerate() {
        let linear = from.to_linear(f64::from(v as u8) / 255.0);
        *out = (to.from_linear(linear).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    Some(lut)
}

impl DirectSource {
    /// Opens the sources when the composition qualifies; `None` otherwise.
    pub fn open(
        comp: &Composition,
        root: &Path,
        format: PlaneFormat,
        tags: ResolvedTags,
    ) -> Result<Option<Self>, MediaError> {
        Self::open_with(comp, root, format, tags, false)
    }

    /// Opens the sources of a composition whose first layer qualifies and
    /// whose further layers hold overlays that composite normally. The
    /// frames come out without the overlays; the caller draws those
    /// (`CpuRenderer::render_overlays`) and lays them over the planes
    /// (`convert::blend_overlay`) for the frames that show them.
    pub fn open_base(
        comp: &Composition,
        root: &Path,
        format: PlaneFormat,
        tags: ResolvedTags,
    ) -> Result<Option<Self>, MediaError> {
        if comp.layers.len() < 2 {
            return Ok(None);
        }
        Self::open_with(comp, root, format, tags, true)
    }

    fn open_with(
        comp: &Composition,
        root: &Path,
        format: PlaneFormat,
        tags: ResolvedTags,
        with_overlays: bool,
    ) -> Result<Option<Self>, MediaError> {
        init();
        let clips = if with_overlays {
            base_video_clips(comp, root)?
        } else {
            filling_video_clips(comp, root)?
        };
        let Some(filling) = clips else {
            return Ok(None);
        };
        if filling.is_empty() {
            return Ok(None);
        }
        // Scaling and repacking happen in the coded YCbCr encoding; a
        // conversion to RGB applies the source's matrix and re-encodes its
        // transfer curve. Anything else (a change of matrix or primaries
        // between YCbCr encodings, RGB sources) is the compositor's.
        let to_rgb = format == PlaneFormat::Rgba8;
        let mut clips = Vec::with_capacity(filling.len());
        let mut how = Conversion::AsIs;
        for clip in filling {
            let overrides = comp
                .assets
                .get(&clip.asset)
                .map(|a| a.color)
                .unwrap_or_default();
            let reader = VideoReader::open(&clip.path, overrides)?;
            let source = reader.tags();
            let pixel = reader.pixel_format();
            let transfer_lut = if to_rgb {
                let eight_bit_ycbcr = !ffi::is_rgb(pixel)
                    && format_of(pixel).is_some_and(|f| f.bits() == 8)
                    && source.matrix != Matrix::Identity;
                if !eight_bit_ycbcr || source.primaries != tags.primaries {
                    return Ok(None);
                }
                transfer_lut(source.transfer, tags.transfer)
            } else {
                if source != tags {
                    return Ok(None);
                }
                None
            };
            let same_layout = format_of(pixel) == Some(format);
            let same_size = clip.width == comp.width && clip.height == comp.height;
            if !(same_layout && same_size) || clip.place.is_some() {
                // Only YCbCr sources are scaled here; RGB ones would need
                // a matrix conversion, which is the compositor's job.
                if ffi::is_rgb(pixel) {
                    return Ok(None);
                }
                how = Conversion::Scaled;
            }
            clips.push(DirectClip {
                start: clip.start,
                end: clip.end,
                in_: clip.in_,
                place: clip.place,
                reader,
                converter: Converter {
                    scaler: None,
                    scaler_input: None,
                    scratch: frame::Video::empty(),
                    matrix: source.matrix,
                    range: source.range,
                    transfer_lut,
                },
            });
        }
        let bars = clips
            .iter()
            .any(|c| matches!(c.place, Some(Place::Bars(_))))
            .then(|| {
                frame_to_planes(
                    &Frame::new(comp.width, comp.height, comp.background),
                    tags,
                    format,
                )
            });
        if clips
            .iter()
            .any(|c| matches!(c.place, Some(Place::Crop(_))))
        {
            how = Conversion::Cropped;
        }
        Ok(Some(Self {
            clips,
            format,
            width: comp.width,
            height: comp.height,
            how,
            to_rgb,
            with_overlays,
            bars,
        }))
    }

    /// Why this path was taken, for the render report.
    pub fn reason(&self) -> String {
        let what = if self.to_rgb {
            "converted to RGB straight from the decoder"
        } else if self.bars.is_some() {
            "scaled straight from the decoder onto the background of the frame"
        } else if self.how == Conversion::Cropped {
            "cropped and scaled straight from the decoder to the encoder"
        } else if self.how == Conversion::Scaled {
            "scaled and repacked straight from the decoder to the encoder"
        } else {
            "handed to the encoder as decoded"
        };
        let overlays = if self.with_overlays {
            ", with the layers above drawn onto the frames that show them"
        } else {
            ""
        };
        if self.clips.len() == 1 {
            format!("the video is used as it is, so frames are {what}{overlays}")
        } else {
            format!(
                "all {} sources are used as they are, so frames are {what}{overlays}",
                self.clips.len()
            )
        }
    }

    /// The output frame shown at time `t`.
    pub fn frame(&mut self, t: Ratio) -> Result<Planes, MediaError> {
        let clip = self
            .clips
            .iter_mut()
            .find(|c| c.start <= t && t < c.end)
            .ok_or_else(|| MediaError::Codec {
                context: "direct transcode".to_owned(),
                reason: format!("no clip covers {t}s"),
            })?;
        let raw = clip.reader.raw_frame_at(clip.in_ + (t - clip.start))?;
        // A cropped clip shows one region of the decoded frame.
        let view;
        let raw = match clip.place {
            Some(Place::Crop([x, y, w, h])) => {
                view = ffi::cropped(raw, x, y, w, h).map_err(|e| codec_error("cropping", e))?;
                &view
            }
            _ => raw,
        };
        // The picture's size: the frame's, or its place on the background.
        let (width, height) = match clip.place {
            Some(Place::Bars(r)) => (r[2], r[3]),
            _ => (self.width, self.height),
        };
        let needs_scaler = raw.width() != width
            || raw.height() != height
            || format_of(raw.format()) != Some(self.format);
        let planes = if needs_scaler {
            clip.converter
                .scale(raw, self.format, self.to_rgb, width, height)?
        } else {
            copy_planes(raw, self.format)
        };
        match (clip.place, &self.bars) {
            (Some(Place::Bars(rect)), Some(bars)) => {
                let mut out = bars.clone();
                blit_planes(&mut out, &planes, rect[0], rect[1]);
                Ok(out)
            }
            _ => Ok(planes),
        }
    }
}

impl Converter {
    /// Scales `raw` to `width`×`height` in `format`, building the scaler
    /// on the first frame and whenever the decoder's layout changes.
    fn scale(
        &mut self,
        raw: &frame::Video,
        format: PlaneFormat,
        to_rgb: bool,
        width: u32,
        height: u32,
    ) -> Result<Planes, MediaError> {
        let clip = self;
        let dst = pixel_of(format);
        let input = (raw.format(), raw.width(), raw.height());
        if clip.scaler_input != Some(input) {
            let mut flags = scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND;
            if to_rgb {
                // Full chroma interpolation: without it the library takes a
                // reduced-precision path to RGB with chroma repeated. That
                // path is the generic one, which can run on several threads.
                flags |= scaling::Flags::FULL_CHR_H_INT | scaling::Flags::FULL_CHR_H_INP;
            }
            let threads = std::thread::available_parallelism().map_or(1, usize::from);
            let mut scaler = ffi::ThreadedScaler::new(
                raw.format(),
                (raw.width(), raw.height()),
                dst,
                (width, height),
                flags,
                threads,
            )
            .map_err(|e| codec_error("scaling", e))?;
            if to_rgb {
                scaler.set_input_colorspace(clip.matrix, clip.range);
            }
            clip.scaler = Some(scaler);
            clip.scaler_input = Some(input);
        }
        if clip.scratch.width() != width
            || clip.scratch.height() != height
            || clip.scratch.format() != dst
        {
            clip.scratch = frame::Video::new(dst, width, height);
        }
        clip.scaler
            .as_mut()
            .expect("scaler was just built")
            .run(raw, &mut clip.scratch)
            .map_err(|e| codec_error("scaling", e))?;
        let mut planes = copy_planes(&clip.scratch, format);
        if let Some(lut) = &clip.transfer_lut {
            let plane = &mut planes.planes[0];
            let stride = plane.stride;
            plane.data.par_chunks_mut(stride).for_each(|row| {
                for px in row.chunks_exact_mut(4) {
                    px[0] = lut[px[0] as usize];
                    px[1] = lut[px[1] as usize];
                    px[2] = lut[px[2] as usize];
                }
            });
        }
        Ok(planes)
    }
}

/// Lays `src` onto `dst` with its top-left corner at (`x`, `y`), both in
/// the same layout; the corner must be even where chroma is subsampled.
fn blit_planes(dst: &mut Planes, src: &Planes, x: u32, y: u32) {
    debug_assert_eq!(dst.format, src.format);
    let (dx, dy) = dst.format.chroma_divisors();
    for (i, (d, s)) in dst.planes.iter_mut().zip(&src.planes).enumerate() {
        let (px, py) = if i == 0 {
            (x as usize, y as usize)
        } else {
            (x as usize / dx, y as usize / dy)
        };
        let sample_bytes = s.stride / s.width;
        let x_bytes = px * sample_bytes;
        for row in 0..s.height.min(d.height.saturating_sub(py)) {
            let width = (s.stride).min(d.stride.saturating_sub(x_bytes));
            let from = &s.data[row * s.stride..row * s.stride + width];
            let at = (py + row) * d.stride + x_bytes;
            d.data[at..at + width].copy_from_slice(from);
        }
    }
}

/// Copies a decoded frame's planes out of their padded rows.
fn copy_planes(raw: &frame::Video, format: PlaneFormat) -> Planes {
    let mut out = Planes::new(format, raw.width(), raw.height());
    for (i, plane) in out.planes.iter_mut().enumerate() {
        let stride = raw.stride(i);
        let data = raw.data(i);
        let row_bytes = plane.stride;
        for row in 0..plane.height {
            plane.data[row * row_bytes..(row + 1) * row_bytes]
                .copy_from_slice(&data[row * stride..row * stride + row_bytes]);
        }
    }
    out
}
