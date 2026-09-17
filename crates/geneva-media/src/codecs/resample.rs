//! A rate change on packed stereo `f32`, for audio that is processed at
//! a rate other than the output's. Timing is kept: the output sample at
//! `k` is the input at `k` scaled by the rates, whatever is buffered
//! inside between calls.

use ffmpeg_next::software::resampling;
use ffmpeg_next::util::channel_layout::ChannelLayout;
use ffmpeg_next::util::format::{Sample, sample};
use ffmpeg_next::util::frame;

use super::codec_error;
use crate::MediaError;

/// The converter and the rates it runs between.
pub struct Resampler {
    ctx: resampling::Context,
    from: u32,
    to: u32,
}

/// An output frame with room for `frames` frames. The context's own
/// allocation sizes the output by the input, which is too small going
/// up in rate and nothing at all on a flush, so the room is made here.
fn room(frames: usize) -> frame::Audio {
    frame::Audio::new(
        Sample::F32(sample::Type::Packed),
        frames.max(1),
        ChannelLayout::STEREO,
    )
}

impl Resampler {
    /// A converter from `from` Hz to `to` Hz.
    pub fn new(from: u32, to: u32) -> Result<Self, MediaError> {
        let packed = Sample::F32(sample::Type::Packed);
        let ctx = resampling::Context::get(
            packed,
            ChannelLayout::STEREO,
            from,
            packed,
            ChannelLayout::STEREO,
            to,
        )
        .map_err(|e| codec_error("audio rate conversion", e))?;
        Ok(Self { ctx, from, to })
    }

    /// Frames still inside the converter, in output frames.
    fn buffered(&self) -> usize {
        self.ctx.delay().map_or(0, |d| d.output.max(0) as usize)
    }

    /// Converts interleaved frames and returns what is ready.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>, MediaError> {
        let n = samples.len() / 2;
        if n == 0 {
            return Ok(Vec::new());
        }
        let mut packed =
            frame::Audio::new(Sample::F32(sample::Type::Packed), n, ChannelLayout::STEREO);
        packed.set_rate(self.from);
        for (dst, v) in packed.data_mut(0).chunks_exact_mut(4).zip(samples) {
            dst.copy_from_slice(&v.to_le_bytes());
        }
        let expected = (n as f64 * f64::from(self.to) / f64::from(self.from)).ceil() as usize;
        let mut out = room(expected + self.buffered() + 64);
        self.ctx
            .run(&packed, &mut out)
            .map_err(|e| codec_error("audio rate conversion", e))?;
        Ok(take(&out))
    }

    /// Returns what is still buffered, once the input has ended.
    pub fn finish(&mut self) -> Result<Vec<f32>, MediaError> {
        let mut out = Vec::new();
        loop {
            let mut tail = room(self.buffered() + 64);
            self.ctx
                .flush(&mut tail)
                .map_err(|e| codec_error("audio rate conversion", e))?;
            if tail.samples() == 0 {
                break;
            }
            out.extend(take(&tail));
        }
        Ok(out)
    }
}

fn take(f: &frame::Audio) -> Vec<f32> {
    let n = f.samples();
    if n == 0 {
        return Vec::new();
    }
    f.data(0)[..n * 2 * 4]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect()
}
