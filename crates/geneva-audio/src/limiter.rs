//! A true-peak limiter: a fixed gain, then whatever gain reduction it
//! takes to keep the interpolated peaks under a ceiling. It looks 5 ms
//! ahead, so the reduction is in place when the peak arrives, ramps in
//! over those 5 ms, and releases over 100 ms. Fixed curves, no knobs.

use std::collections::VecDeque;

use crate::peak::TruePeak;

/// Seconds of look-ahead, which is also the attack time.
const LOOK_AHEAD: f64 = 0.005;
/// Release time constant in seconds.
const RELEASE: f64 = 0.1;

/// The limiter and the frames it holds back.
#[derive(Debug, Clone)]
pub struct Limiter {
    channels: usize,
    gain: f32,
    ceiling: f32,
    look: usize,
    peak: TruePeak,
    /// Frames after the gain, waiting for their gain reduction.
    delay: VecDeque<f32>,
    /// Frames fed so far (padding at the end included), frames that
    /// were real, and frames let out.
    fed: u64,
    arrived: u64,
    emitted: u64,
    /// Sliding-window minimum of the required gain, as (frame, gain),
    /// gains rising from front to back.
    min_window: VecDeque<(u64, f32)>,
    /// The last `look + 1` window minima and their sum, for the ramp.
    ramp: VecDeque<f32>,
    ramp_sum: f64,
    reduction: f32,
    release: f32,
}

impl Limiter {
    /// A limiter for `channels` channels at `rate` Hz that applies
    /// `gain_db`, then holds true peaks at or under `ceiling_dbtp`.
    #[must_use]
    pub fn new(rate: u32, channels: usize, gain_db: f64, ceiling_dbtp: f64) -> Self {
        let channels = channels.max(1);
        let look = (LOOK_AHEAD * f64::from(rate)).ceil().max(1.0) as usize;
        Self {
            channels,
            gain: crate::from_db(gain_db) as f32,
            ceiling: crate::from_db(ceiling_dbtp) as f32,
            look,
            peak: TruePeak::new(rate, channels),
            delay: VecDeque::new(),
            fed: 0,
            arrived: 0,
            emitted: 0,
            min_window: VecDeque::new(),
            ramp: VecDeque::new(),
            ramp_sum: 0.0,
            reduction: 1.0,
            release: (1.0 - (-1.0 / (RELEASE * f64::from(rate))).exp()) as f32,
        }
    }

    /// Feeds interleaved frames and returns the frames that are ready:
    /// as many as came in, once the look-ahead has filled.
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(samples.len());
        let mut frame = vec![0.0f32; self.channels];
        for input in samples.chunks_exact(self.channels) {
            for (f, &s) in frame.iter_mut().zip(input) {
                *f = s * self.gain;
            }
            self.arrived += 1;
            self.feed(&frame, &mut out);
        }
        out
    }

    /// Lets out the frames still held back. The limiter is spent after.
    pub fn finish(&mut self) -> Vec<f32> {
        let mut out = Vec::new();
        let silence = vec![0.0f32; self.channels];
        while self.emitted < self.arrived {
            self.feed(&silence, &mut out);
        }
        out
    }

    /// One frame in, and one out once enough are held.
    fn feed(&mut self, frame: &[f32], out: &mut Vec<f32>) {
        self.fed += 1;
        self.delay.extend(frame.iter().copied());
        // The peak reported now belongs to the frame the interpolator's
        // delay back.
        let p = self.peak.push(frame);
        let index = (self.fed - 1).saturating_sub(self.peak.delay() as u64);
        let required = if p > self.ceiling {
            self.ceiling / p
        } else {
            1.0
        };
        while self.min_window.back().is_some_and(|(_, g)| *g >= required) {
            self.min_window.pop_back();
        }
        self.min_window.push_back((index, required));
        // The next frame out is the one whose whole look-ahead has been
        // peaked.
        let front = self.emitted;
        if index < front + self.look as u64 {
            return;
        }
        while self.min_window.front().is_some_and(|(i, _)| *i < front) {
            self.min_window.pop_front();
        }
        let window_min = self.min_window.front().map_or(1.0, |(_, g)| *g);
        if self.ramp.len() > self.look {
            self.ramp_sum -= f64::from(self.ramp.pop_front().unwrap_or(0.0));
        }
        self.ramp.push_back(window_min);
        self.ramp_sum += f64::from(window_min);
        let target = (self.ramp_sum / self.ramp.len() as f64) as f32;
        self.reduction = if target < self.reduction {
            target
        } else {
            self.reduction + (target - self.reduction) * self.release
        };
        for _ in 0..self.channels {
            let s = self.delay.pop_front().unwrap_or(0.0);
            out.push(s * self.reduction);
        }
        self.emitted += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        rate: u32,
        channels: usize,
        gain_db: f64,
        ceiling_dbtp: f64,
        input: &[f32],
        block: usize,
    ) -> Vec<f32> {
        let mut l = Limiter::new(rate, channels, gain_db, ceiling_dbtp);
        let mut out = Vec::new();
        for chunk in input.chunks(block * channels) {
            out.extend(l.push(chunk));
        }
        out.extend(l.finish());
        out
    }

    #[test]
    fn as_many_frames_come_out_as_went_in_whatever_the_blocks() {
        let input: Vec<f32> = (0..9_601 * 2)
            .map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5)
            .collect();
        let a = run(48_000, 2, 0.0, -1.0, &input, 48_000);
        let b = run(48_000, 2, 0.0, -1.0, &input, 37);
        assert_eq!(a.len(), input.len());
        assert_eq!(a, b);
    }

    #[test]
    fn quiet_material_is_only_gained() {
        let input: Vec<f32> = (0..48_000).map(|i| 0.1 * (i as f32 * 0.05).sin()).collect();
        let out = run(48_000, 1, 6.0, -1.0, &input, 1024);
        for (o, i) in out.iter().zip(&input) {
            assert!((o - i * crate::from_db(6.0) as f32).abs() < 1e-5);
        }
    }

    #[test]
    fn peaks_are_held_at_the_ceiling() {
        let rate = 48_000;
        let mut input: Vec<f32> = (0..rate as usize)
            .map(|i| 0.2 * (i as f32 * 0.07).sin())
            .collect();
        // A burst well over full scale in the middle.
        for s in &mut input[20_000..22_000] {
            *s *= 8.0;
        }
        let out = run(rate, 1, 0.0, -1.0, &input, 4096);
        let ceiling = crate::from_db(-1.0) as f32;
        let tp = crate::true_peak(rate, 1, &out);
        assert!(
            tp <= ceiling * 1.005,
            "true peak {tp} over ceiling {ceiling}"
        );
        // Before the burst the signal is untouched.
        for (o, i) in out[..19_000].iter().zip(&input[..19_000]) {
            assert!((o - i).abs() < 1e-6);
        }
    }
}
