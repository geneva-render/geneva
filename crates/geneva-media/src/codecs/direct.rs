use std::path::Path;

use ffmpeg_next::util::frame;
use geneva_color::ResolvedTags;
use geneva_timeline::{Composition, Ratio};

use super::copy::untouched_video_clips;
use super::decode::VideoReader;
use super::encode::format_of;
use super::init;
use crate::MediaError;
use crate::convert::{PlaneFormat, Planes};

/// Decoded frames of a composition that shows its video sources exactly
/// as they are, delivered in the encoder's format without passing through
/// the compositing pipeline.
///
/// The composition must be one layer of untouched video clips (see the
/// stream-copy rules) whose streams are already in the encoder's sample
/// layout and carry the same color tags as the output, so that no pixel
/// would change on the way through the renderer.
pub struct DirectSource {
    clips: Vec<DirectClip>,
    format: PlaneFormat,
}

struct DirectClip {
    start: Ratio,
    end: Ratio,
    in_: Ratio,
    reader: VideoReader,
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
        let Some(untouched) = untouched_video_clips(comp, root)? else {
            return Ok(None);
        };
        if untouched.is_empty() {
            return Ok(None);
        }
        let mut clips = Vec::with_capacity(untouched.len());
        for clip in untouched {
            let overrides = comp
                .assets
                .get(&clip.asset)
                .map(|a| a.color)
                .unwrap_or_default();
            let reader = VideoReader::open(&clip.path, overrides)?;
            if format_of(reader.pixel_format()) != Some(format) || reader.tags() != tags {
                return Ok(None);
            }
            clips.push(DirectClip {
                start: clip.start,
                end: clip.end,
                in_: clip.in_,
                reader,
            });
        }
        Ok(Some(Self { clips, format }))
    }

    /// Why this path was taken, for the render report.
    pub fn reason(&self) -> String {
        if self.clips.len() == 1 {
            "the video is used as is, so decoded frames go straight to the encoder".to_owned()
        } else {
            format!(
                "all {} sources are used as is, so decoded frames go straight to the encoder",
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
        Ok(copy_planes(raw, self.format))
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
