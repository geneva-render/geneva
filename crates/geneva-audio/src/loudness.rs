//! Integrated loudness to ITU-R BS.1770-4: K-weighting, mean square over
//! 400 ms blocks every 100 ms, an absolute gate at -70 LUFS and a
//! relative gate 10 LU under the mean of what passed it.

/// One second-order section in direct form I.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b,
            a,
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[1] * self.y[0]
            - self.a[2] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// The two stages of the K-weighting filter at `rate`: the high shelf
/// that stands in for the head, then the high-pass. BS.1770 tabulates
/// them at 48 kHz; these are the same analogue prototypes discretised at
/// any rate, and match the table at 48 kHz to eleven places.
fn k_weighting(rate: f64) -> [Biquad; 2] {
    let (f0, gain_db, q) = (
        1_681.974_450_955_533,
        3.999_843_853_973_347,
        0.707_175_236_955_419_6,
    );
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let vh = 10f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad::new(
        [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad::new(
        [1.0, -2.0, 1.0],
        [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    );
    [shelf, high_pass]
}

/// Integrated loudness of a stream, fed a block at a time.
#[derive(Debug, Clone)]
pub struct Meter {
    channels: usize,
    filters: Vec<[Biquad; 2]>,
    weights: Vec<f64>,
    /// Frames in one 100 ms step.
    hop: usize,
    /// Sum of squares per channel over the step in progress.
    step: Vec<f64>,
    filled: usize,
    /// The last four complete steps, per channel.
    recent: std::collections::VecDeque<Vec<f64>>,
    /// Mean square of each complete 400 ms block, channel-weighted.
    blocks: Vec<f64>,
}

impl Meter {
    /// A meter for interleaved audio of `channels` channels at `rate`
    /// Hz. Channels beyond the first two are weighted as BS.1770 weights
    /// a 5.0 layout: centre at unity, the surrounds 1.5 dB up.
    #[must_use]
    pub fn new(rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let weights = (0..channels)
            .map(|c| if c == 3 || c == 4 { 1.41 } else { 1.0 })
            .collect();
        Self {
            channels,
            filters: vec![k_weighting(f64::from(rate)); channels],
            weights,
            hop: (f64::from(rate) * 0.1).round().max(1.0) as usize,
            step: vec![0.0; channels],
            filled: 0,
            recent: std::collections::VecDeque::with_capacity(4),
            blocks: Vec::new(),
        }
    }

    /// Feeds interleaved frames. A trailing part of a frame is ignored.
    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            for (c, &s) in frame.iter().enumerate() {
                let [shelf, high_pass] = &mut self.filters[c];
                let w = high_pass.run(shelf.run(f64::from(s)));
                self.step[c] += w * w;
            }
            self.filled += 1;
            if self.filled == self.hop {
                self.close_step();
            }
        }
    }

    fn close_step(&mut self) {
        let step = std::mem::replace(&mut self.step, vec![0.0; self.channels]);
        self.filled = 0;
        if self.recent.len() == 4 {
            self.recent.pop_front();
        }
        self.recent.push_back(step);
        if self.recent.len() == 4 {
            let frames = (4 * self.hop) as f64;
            let z: f64 = (0..self.channels)
                .map(|c| self.weights[c] * self.recent.iter().map(|s| s[c]).sum::<f64>() / frames)
                .sum();
            self.blocks.push(z);
        }
    }

    /// The integrated loudness in LUFS of everything fed so far, or
    /// `None` when no 400 ms block rose above the absolute gate (silence,
    /// or a stream shorter than 400 ms).
    #[must_use]
    pub fn integrated(&self) -> Option<f64> {
        let loudness = |z: f64| -0.691 + 10.0 * z.max(f64::MIN_POSITIVE).log10();
        let above_absolute: Vec<f64> = self
            .blocks
            .iter()
            .copied()
            .filter(|&z| loudness(z) > -70.0)
            .collect();
        if above_absolute.is_empty() {
            return None;
        }
        let mean = above_absolute.iter().sum::<f64>() / above_absolute.len() as f64;
        let relative_gate = loudness(mean) - 10.0;
        let gated: Vec<f64> = above_absolute
            .into_iter()
            .filter(|&z| loudness(z) > relative_gate)
            .collect();
        if gated.is_empty() {
            return None;
        }
        Some(loudness(gated.iter().sum::<f64>() / gated.len() as f64))
    }
}

/// The integrated loudness of a whole buffer.
#[must_use]
pub fn integrated(rate: u32, channels: usize, samples: &[f32]) -> Option<f64> {
    let mut meter = Meter::new(rate, channels);
    meter.push(samples);
    meter.integrated()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, seconds: f64, hz: f64, amplitude: f64) -> Vec<f32> {
        (0..(f64::from(rate) * seconds) as usize)
            .map(|i| {
                (amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin())
                    as f32
            })
            .collect()
    }

    #[test]
    fn the_48k_coefficients_match_the_table_in_the_standard() {
        let [shelf, high_pass] = k_weighting(48_000.0);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(shelf.b[0], 1.535_124_859_586_97));
        assert!(close(shelf.b[1], -2.691_696_189_406_38));
        assert!(close(shelf.b[2], 1.198_392_810_852_85));
        assert!(close(shelf.a[1], -1.690_659_293_182_41));
        assert!(close(shelf.a[2], 0.732_480_774_215_85));
        assert!(close(high_pass.a[1], -1.990_047_454_833_98));
        assert!(close(high_pass.a[2], 0.990_072_250_366_21));
    }

    #[test]
    fn a_full_scale_tone_in_one_channel_reads_minus_three() {
        // The standard's own check: a 997 Hz sine at 0 dBFS in one
        // channel of a stereo pair reads -3.01 LKFS.
        let mono = sine(48_000, 5.0, 997.0, 1.0);
        let mut stereo = Vec::with_capacity(mono.len() * 2);
        for s in mono {
            stereo.push(s);
            stereo.push(0.0);
        }
        let lufs = integrated(48_000, 2, &stereo).unwrap();
        assert!((lufs - -3.01).abs() < 0.05, "{lufs}");
    }

    #[test]
    fn the_same_tone_at_another_rate_reads_the_same() {
        let a = integrated(44_100, 1, &sine(44_100, 5.0, 997.0, 0.1)).unwrap();
        let b = integrated(48_000, 1, &sine(48_000, 5.0, 997.0, 0.1)).unwrap();
        let c = integrated(16_000, 1, &sine(16_000, 5.0, 997.0, 0.1)).unwrap();
        // The filters are the standard's analogue prototypes discretised
        // at each rate, so the rates agree to within a few hundredths.
        assert!((a - b).abs() < 0.1, "{a} {b}");
        assert!((c - b).abs() < 0.1, "{c} {b}");
        assert!((b - -23.01).abs() < 0.02, "{b}");
    }

    #[test]
    fn silence_and_short_streams_read_nothing() {
        assert_eq!(integrated(48_000, 2, &vec![0.0; 48_000 * 2]), None);
        assert_eq!(integrated(48_000, 1, &sine(48_000, 0.3, 997.0, 0.5)), None);
    }

    #[test]
    fn the_gate_drops_a_quiet_tail() {
        // Two seconds of tone then eight of near silence: without the
        // relative gate the mean would fall by about 7 dB. With it, the
        // 17 blocks of tone count, the three straddling its end (at
        // three quarters, a half and a quarter tone) are within 10 LU
        // and count too, and the rest are gated out: a mean power of
        // 18.5 / 20, which is 0.34 dB under the tone's own -23.01.
        let rate = 48_000;
        let mut s = sine(rate, 2.0, 997.0, 0.1);
        s.extend(sine(rate, 8.0, 997.0, 0.000_1));
        let lufs = integrated(rate, 1, &s).unwrap();
        assert!((lufs - -23.35).abs() < 0.05, "{lufs}");
    }
}
