use std::path::Path;

use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame;
use geneva_color::ResolvedTags;
use geneva_timeline::{Composition, Ratio};

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
pub struct DirectSource {
    clips: Vec<DirectClip>,
    format: PlaneFormat,
    width: u32,
    height: u32,
    scaled: bool,
}

struct DirectClip {
    start: Ratio,
    end: Ratio,
    in_: Ratio,
    reader: VideoReader,
    /// Converter from the decoder's frames to the output layout and size,
    /// built from the first frame; `None` while frames can be copied.
    scaler: Option<scaling::Context>,
    scratch: frame::Video,
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
        // Scaling and repacking happen in the coded YCbCr encoding; only
        // the compositor converts between color encodings or to RGB.
        if format == PlaneFormat::Rgba8 {
            return Ok(None);
        }
        let mut clips = Vec::with_capacity(filling.len());
        let mut scaled = false;
        for clip in filling {
            let overrides = comp
                .assets
                .get(&clip.asset)
                .map(|a| a.color)
                .unwrap_or_default();
            let reader = VideoReader::open(&clip.path, overrides)?;
            if reader.tags() != tags {
                return Ok(None);
            }
            let pixel = reader.pixel_format();
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
                scratch: frame::Video::empty(),
            });
        }
        Ok(Some(Self {
            clips,
            format,
            width: comp.width,
            height: comp.height,
            scaled,
        }))
    }

    /// Why this path was taken, for the render report.
    pub fn reason(&self) -> String {
        let what = if self.scaled {
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
        let scaler = match clip.scaler.as_mut() {
            Some(s) => {
                s.cached(
                    raw.format(),
                    raw.width(),
                    raw.height(),
                    dst,
                    self.width,
                    self.height,
                    scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND,
                );
                s
            }
            None => clip.scaler.insert(
                scaling::Context::get(
                    raw.format(),
                    raw.width(),
                    raw.height(),
                    dst,
                    self.width,
                    self.height,
                    scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND,
                )
                .map_err(|e| codec_error("scaling", e))?,
            ),
        };
        if clip.scratch.width() != self.width
            || clip.scratch.height() != self.height
            || clip.scratch.format() != dst
        {
            clip.scratch = frame::Video::new(dst, self.width, self.height);
        }
        scaler
            .run(raw, &mut clip.scratch)
            .map_err(|e| codec_error("scaling", e))?;
        Ok(copy_planes(&clip.scratch, self.format))
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
