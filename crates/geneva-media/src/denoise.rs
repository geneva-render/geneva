//! Speech denoising with DeepFilterNet, the model embedded in the
//! binary. The mix is processed at 48 kHz, which the model is trained
//! at, as a mid and a side channel: the model sees the centre, where
//! speech is, and the difference, where the width is, and the output
//! is put back into left and right, so the stereo image survives the
//! way the model's decisions leave it rather than being made twice.
//!
//! The model looks ahead, so its output lags its input by a fixed
//! number of samples; that lag is taken off the front and the tail is
//! flushed with silence, so the sound stays where it was.

use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;

use crate::MediaError;
use crate::codecs::Resampler;

/// The model's rate.
const MODEL_RATE: u32 = 48_000;

/// The model with its state, and the frames waiting for a whole hop.
pub struct Denoiser {
    model: DfTract,
    hop: usize,
    /// Frames of lag still to drop from the output.
    skip: usize,
    /// Frames of lag, for the flush.
    delay: usize,
    to_model: Option<Resampler>,
    from_model: Option<Resampler>,
    mid: Vec<f32>,
    side: Vec<f32>,
    /// The output's rate.
    rate: u32,
    /// Frames of real input at the output's rate, and frames let out
    /// at the model's, so the flush stops where the input did.
    fed: usize,
    emitted: usize,
}

fn model_error(e: &anyhow::Error) -> MediaError {
    MediaError::Codec {
        context: "denoise".to_owned(),
        reason: format!("{e:#}"),
    }
}

impl Denoiser {
    /// A denoiser for interleaved stereo at `rate` Hz.
    pub fn new(rate: u32) -> Result<Self, MediaError> {
        let model = DfTract::new(DfParams::default(), &RuntimeParams::default_with_ch(2))
            .map_err(|e| model_error(&e))?;
        let hop = model.hop_size;
        let delay = model.fft_size - hop + model.lookahead * hop;
        let (to_model, from_model) = if rate == MODEL_RATE {
            (None, None)
        } else {
            (
                Some(Resampler::new(rate, MODEL_RATE)?),
                Some(Resampler::new(MODEL_RATE, rate)?),
            )
        };
        Ok(Self {
            model,
            hop,
            skip: delay,
            delay,
            to_model,
            from_model,
            mid: Vec::new(),
            side: Vec::new(),
            rate,
            fed: 0,
            emitted: 0,
        })
    }

    /// Feeds interleaved frames and returns what is ready. Over a whole
    /// stream, [`finish`](Self::finish) included, as many frames come
    /// out as went in, give or take the rounding of a rate change.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>, MediaError> {
        let at_model = match self.to_model.as_mut() {
            Some(r) => r.push(samples)?,
            None => samples.to_vec(),
        };
        for lr in at_model.chunks_exact(2) {
            self.mid.push((lr[0] + lr[1]) * 0.5);
            self.side.push((lr[0] - lr[1]) * 0.5);
        }
        self.fed += samples.len() / 2;
        let out = self.run_hops()?;
        match self.from_model.as_mut() {
            Some(r) => r.push(&out),
            None => Ok(out),
        }
    }

    /// How many frames the real input makes at the model's rate.
    fn fed_at_model(&self) -> usize {
        (self.fed as f64 * f64::from(MODEL_RATE) / f64::from(self.rate)).round() as usize
    }

    /// Runs every whole hop that is buffered.
    fn run_hops(&mut self) -> Result<Vec<f32>, MediaError> {
        let hop = self.hop;
        let mut out = Vec::new();
        let mut noisy = Array2::<f32>::zeros((2, hop));
        let mut enhanced = Array2::<f32>::zeros((2, hop));
        while self.mid.len() >= hop {
            for i in 0..hop {
                noisy[[0, i]] = self.mid[i];
                noisy[[1, i]] = self.side[i];
            }
            self.model
                .process(noisy.view(), enhanced.view_mut())
                .map_err(|e| model_error(&e))?;
            self.mid.drain(..hop);
            self.side.drain(..hop);
            for i in 0..hop {
                if self.skip > 0 {
                    self.skip -= 1;
                    continue;
                }
                if self.emitted >= self.fed_at_model() {
                    break;
                }
                let (m, s) = (enhanced[[0, i]], enhanced[[1, i]]);
                out.push(m + s);
                out.push(m - s);
                self.emitted += 1;
            }
        }
        Ok(out)
    }

    /// Flushes the lag out with silence and returns the rest.
    pub fn finish(&mut self) -> Result<Vec<f32>, MediaError> {
        // Enough silence to push the last real frames through the model
        // and fill the hop the input ended in.
        let pad = self.delay + self.hop;
        let mut out = match self.to_model.as_mut() {
            Some(r) => {
                let mut v = r.push(&vec![0.0; pad * 2])?;
                v.extend(r.finish()?);
                v
            }
            None => vec![0.0; pad * 2],
        };
        // The padding is not real input: `fed` stays where it is, so
        // the hops stop letting frames out where the input ended.
        for lr in out.chunks_exact(2) {
            self.mid.push((lr[0] + lr[1]) * 0.5);
            self.side.push((lr[0] - lr[1]) * 0.5);
        }
        out = self.run_hops()?;
        match self.from_model.as_mut() {
            Some(r) => {
                let mut v = r.push(&out)?;
                v.extend(r.finish()?);
                Ok(v)
            }
            None => Ok(out),
        }
    }
}
