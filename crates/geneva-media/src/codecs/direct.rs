use std::path::Path;

use ffmpeg_next::software::scaling;
use ffmpeg_next::util::format::Pixel;
use ffmpeg_next::util::frame;
use geneva_color::{Matrix, Range, ResolvedTags, Transfer};
use geneva_timeline::{Composition, Ratio};
use rayon::prelude::*;

use super::copy::filling_video_clips;
use super::decode::VideoReader;
use super::encode::{format_of, pixel_of};
use super::{codec_error, ffi, init};
use crate::MediaError;
use crate::convert::{PlaneFormat, Planes};

/// Decoded frames of a composition that shows its video sources as they
/// are, or merely scaled to the output size, delivered in the encoder's
/// sample layout without passing through the compositing pipeline.
///
/// The composition must be one layer of video clips that fill the frame
/// (see the stream-copy rules) whose streams carry the same color tags as
/// the output. Frames already in the encoder's layout at the output size
/// are copied; otherwise they are scaled and repacked between YCbCr
/// layouts in their coded encoding, which is what a plain transcode does
/// and what a resize or a change of codec needs.
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
    scaled: bool,
    /// Whether frames are converted to RGB.
    to_rgb: bool,
}

struct DirectClip {
    start: Ratio,
    end: Ratio,
    in_: Ratio,
    reader: VideoReader,
    /// Converter from the decoder's frames to the output layout and size,
    /// built from the first frame; `None` while frames can be copied.
    scaler: Option<scaling::Context>,
    /// The converter to RGB, which runs on several threads.
    rgb_scaler: Option<ffi::ThreadedScaler>,
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
        init();
        let Some(filling) = filling_video_clips(comp, root)? else {
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
        let mut scaled = false;
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
            if !(same_layout && same_size) {
                // Only YCbCr sources are scaled here; RGB ones would need
                // a matrix conversion, which is the compositor's job.
                if ffi::is_rgb(pixel) {
                    return Ok(None);
                }
                scaled = true;
            }
            clips.push(DirectClip {
                start: clip.start,
                end: clip.end,
                in_: clip.in_,
                reader,
                scaler: None,
                rgb_scaler: None,
                scaler_input: None,
                scratch: frame::Video::empty(),
                matrix: source.matrix,
                range: source.range,
                transfer_lut,
            });
        }
        Ok(Some(Self {
            clips,
            format,
            width: comp.width,
            height: comp.height,
            scaled,
            to_rgb,
        }))
    }

    /// Why this path was taken, for the render report.
    pub fn reason(&self) -> String {
        let what = if self.to_rgb {
            "converted to RGB straight from the decoder"
        } else if self.scaled {
            "scaled and repacked straight from the decoder to the encoder"
        } else {
            "handed to the encoder as decoded"
        };
        if self.clips.len() == 1 {
            format!("the video is used as it is, so frames are {what}")
        } else {
            format!(
                "all {} sources are used as they are, so frames are {what}",
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
        let needs_scaler = raw.width() != self.width
            || raw.height() != self.height
            || format_of(raw.format()) != Some(self.format);
        if !needs_scaler {
            return Ok(copy_planes(raw, self.format));
        }
        let dst = pixel_of(self.format);
        let input = (raw.format(), raw.width(), raw.height());
        if clip.scaler_input != Some(input) {
            let flags = scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND;
            if self.to_rgb {
                // Full chroma interpolation: without it the library takes a
                // reduced-precision path to RGB with chroma repeated. That
                // path is the generic one, which can run on several threads.
                let flags = flags | scaling::Flags::FULL_CHR_H_INT | scaling::Flags::FULL_CHR_H_INP;
                let threads = std::thread::available_parallelism().map_or(1, usize::from);
                let mut scaler = ffi::ThreadedScaler::new(
                    raw.format(),
                    (raw.width(), raw.height()),
                    dst,
                    (self.width, self.height),
                    flags,
                    threads,
                )
                .map_err(|e| codec_error("scaling", e))?;
                scaler.set_input_colorspace(clip.matrix, clip.range);
                clip.rgb_scaler = Some(scaler);
            } else {
                clip.scaler = Some(
                    scaling::Context::get(
                        raw.format(),
                        raw.width(),
                        raw.height(),
                        dst,
                        self.width,
                        self.height,
                        flags,
                    )
                    .map_err(|e| codec_error("scaling", e))?,
                );
            }
            clip.scaler_input = Some(input);
        }
        if clip.scratch.width() != self.width
            || clip.scratch.height() != self.height
            || clip.scratch.format() != dst
        {
            clip.scratch = frame::Video::new(dst, self.width, self.height);
        }
        if let Some(scaler) = clip.rgb_scaler.as_mut() {
            scaler
                .run(raw, &mut clip.scratch)
                .map_err(|e| codec_error("scaling", e))?;
        } else {
            clip.scaler
                .as_mut()
                .expect("scaler was just built")
                .run(raw, &mut clip.scratch)
                .map_err(|e| codec_error("scaling", e))?;
        }
        let mut planes = copy_planes(&clip.scratch, self.format);
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
