use std::path::Path;

use ffmpeg_next::util::frame;
use geneva_timeline::{Composition, Ratio};

use super::copy::untouched_video_clips;
use super::decode::VideoReader;
use super::init;
use crate::MediaError;
use crate::convert::Yuv420p;

/// Decoded frames of a composition that shows its video sources exactly
/// as they are, delivered in the encoder's format without passing through
/// the compositing pipeline.
///
/// The composition must be one layer of untouched video clips (see the
/// stream-copy rules) whose streams are 8-bit 4:2:0 and carry the same
/// color tags as the output, so that no pixel would change on the way
/// through the renderer.
pub struct DirectSource {
    clips: Vec<DirectClip>,
}

struct DirectClip {
    start: Ratio,
    end: Ratio,
    in_: Ratio,
    reader: VideoReader,
}

impl DirectSource {
    /// Opens the sources when the composition qualifies; `None` otherwise.
    pub fn open(comp: &Composition, root: &Path) -> Result<Option<Self>, MediaError> {
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
            if !reader.is_yuv420p() || reader.tags() != comp.color {
                return Ok(None);
            }
            clips.push(DirectClip {
                start: clip.start,
                end: clip.end,
                in_: clip.in_,
                reader,
            });
        }
        Ok(Some(Self { clips }))
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
    pub fn frame(&mut self, t: Ratio) -> Result<Yuv420p, MediaError> {
        let clip = self
            .clips
            .iter_mut()
            .find(|c| c.start <= t && t < c.end)
            .ok_or_else(|| MediaError::Codec {
                context: "direct transcode".to_owned(),
                reason: format!("no clip covers {t}s"),
            })?;
        let raw = clip.reader.raw_frame_at(clip.in_ + (t - clip.start))?;
        Ok(planes_to_yuv420p(raw))
    }
}

/// Copies a decoded 4:2:0 frame's planes out of their padded rows.
fn planes_to_yuv420p(raw: &frame::Video) -> Yuv420p {
    let (width, height) = (raw.width(), raw.height());
    let cw = width.div_ceil(2) as usize;
    let ch = height.div_ceil(2) as usize;
    let copy = |plane: usize, w: usize, h: usize| {
        let stride = raw.stride(plane);
        let data = raw.data(plane);
        let mut out = Vec::with_capacity(w * h);
        for row in 0..h {
            out.extend_from_slice(&data[row * stride..row * stride + w]);
        }
        out
    };
    Yuv420p {
        width,
        height,
        y: copy(0, width as usize, height as usize),
        cb: copy(1, cw, ch),
        cr: copy(2, cw, ch),
    }
}
