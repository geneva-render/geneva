//! True peak: the largest sample the signal would have between the ones
//! it has, found by interpolating four times as many (twice at rates of
//! 96 kHz and up), as BS.1770-4 Annex 2 measures it.

/// Taps per phase of the interpolation filter.
const TAPS: usize = 12;

/// A polyphase interpolator with the history it needs, one per channel.
#[derive(Debug, Clone)]
pub struct TruePeak {
    channels: usize,
    phases: Vec<Vec<f32>>,
    /// The last `TAPS` samples of each channel, most recent first.
    history: Vec<[f32; TAPS]>,
}

impl TruePeak {
    /// An interpolator for `channels` channels at `rate` Hz.
    #[must_use]
    pub fn new(rate: u32, channels: usize) -> Self {
        let factor = if rate >= 96_000 { 2 } else { 4 };
        Self {
            channels: channels.max(1),
            phases: phases(factor),
            history: vec![[0.0; TAPS]; channels.max(1)],
        }
    }

    /// How many samples of delay the interpolation puts between a peak
    /// arriving and being reported.
    #[must_use]
    pub fn delay(&self) -> usize {
        TAPS / 2
    }

    /// Feeds one frame and returns the largest magnitude, over all
    /// channels, of the interpolated samples around the frame
    /// [`delay`](Self::delay) frames back, the frame itself included.
    pub fn push(&mut self, frame: &[f32]) -> f32 {
        let mut peak = 0.0f32;
        for (c, &x) in frame.iter().enumerate().take(self.channels) {
            let h = &mut self.history[c];
            h.copy_within(0..TAPS - 1, 1);
            h[0] = x;
            peak = peak.max(h[TAPS / 2].abs());
            for phase in &self.phases {
                let v: f32 = phase.iter().zip(h.iter()).map(|(t, s)| t * s).sum();
                peak = peak.max(v.abs());
            }
        }
        peak
    }
}

/// The interpolation filter as one set of taps per phase: a windowed
/// sinc low-pass at the original Nyquist frequency, each phase scaled to
/// pass a steady signal at unity.
fn phases(factor: usize) -> Vec<Vec<f32>> {
    let len = TAPS * factor;
    let centre = (len - 1) as f64 / 2.0;
    let taps: Vec<f64> = (0..len)
        .map(|n| {
            let t = (n as f64 - centre) / factor as f64;
            let sinc = if t.abs() < 1e-12 {
                1.0
            } else {
                (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
            };
            let w = 2.0 * std::f64::consts::PI * n as f64 / (len - 1) as f64;
            let blackman = 0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos();
            sinc * blackman
        })
        .collect();
    (0..factor)
        .map(|p| {
            let phase: Vec<f64> = (0..TAPS).map(|k| taps[p + factor * k]).collect();
            let sum: f64 = phase.iter().sum();
            phase.iter().map(|t| (t / sum) as f32).collect()
        })
        .collect()
}

/// The true peak of interleaved audio, as a linear amplitude.
#[must_use]
pub fn true_peak(rate: u32, channels: usize, samples: &[f32]) -> f32 {
    let channels = channels.max(1);
    let mut tp = TruePeak::new(rate, channels);
    let mut peak = 0.0f32;
    for frame in samples.chunks_exact(channels) {
        peak = peak.max(tp.push(frame));
    }
    // The last few frames are still inside the interpolator.
    for _ in 0..tp.delay() {
        peak = peak.max(tp.push(&vec![0.0; channels]));
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_between_samples_peaks_above_its_samples() {
        // At a quarter of the rate, sampled off its crests, a sine's
        // samples reach only cos(45 degrees) of its amplitude; the true
        // peak is the amplitude.
        let rate = 48_000;
        let s: Vec<f32> = (0..rate)
            .map(|i| (2.0 * std::f64::consts::PI * (f64::from(i) / 4.0 + 0.125)).sin() as f32)
            .collect();
        let sample_peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(
            (sample_peak - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3,
            "{sample_peak}"
        );
        let tp = true_peak(rate as u32, 1, &s);
        assert!((tp - 1.0).abs() < 0.02, "{tp}");
    }

    #[test]
    fn a_steady_signal_passes_at_unity() {
        // Once the edge where it switched on has left the interpolator;
        // a step overshoots under a sinc, as it does in any true-peak
        // meter.
        let mut tp = TruePeak::new(48_000, 2);
        let mut peak = 0.0f32;
        for i in 0..4800 {
            let p = tp.push(&[0.5, 0.5]);
            if i > 100 {
                peak = peak.max(p);
            }
        }
        assert!((peak - 0.5).abs() < 1e-4, "{peak}");
    }
}
