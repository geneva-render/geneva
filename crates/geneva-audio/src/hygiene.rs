//! Audio hygiene by arithmetic: a rumble high-pass, and notches on mains
//! hum where a track has some. Fixed curves, no knobs. Meant for
//! speech: on music the high-pass takes the bass under 80 Hz with it.

use crate::biquad::Biquad;

/// Where the high-pass turns: -3 dB here, 12 dB an octave below.
pub const HIGH_PASS_HZ: f64 = 80.0;
/// How wide each hum notch is between its -3 dB points, so mains drift
/// of a fraction of a hertz stays inside it.
pub const NOTCH_BANDWIDTH_HZ: f64 = 2.0;
/// The highest harmonic looked for.
const TOP_HZ: f64 = 480.0;
/// How far the reference probes sit from a candidate.
const ASIDE_HZ: f64 = 4.0;
/// A hum's fundamental must stand this far over its neighbours, and a
/// harmonic this far, to count.
const BASE_OVER_DB: f64 = 12.0;
const HARMONIC_OVER_DB: f64 = 8.0;
/// A hum quieter than this is not worth a notch, and a harmonic
/// quieter than this is not one.
const FLOOR_DBFS: f64 = -60.0;
const HARMONIC_FLOOR_DBFS: f64 = -70.0;

/// Mains hum found in a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Hum {
    /// 50 or 60.
    pub base_hz: f64,
    /// The frequencies that showed, the fundamental first.
    pub harmonics: Vec<f64>,
    /// Level of the fundamental, as a sine's RMS in dBFS.
    pub level_dbfs: f64,
}

/// One frequency probed by the Goertzel recurrence.
#[derive(Debug, Clone, Copy)]
struct Probe {
    coeff: f64,
    s1: f64,
    s2: f64,
}

impl Probe {
    fn at(rate: f64, hz: f64) -> Self {
        Self {
            coeff: 2.0 * (2.0 * std::f64::consts::PI * hz / rate).cos(),
            s1: 0.0,
            s2: 0.0,
        }
    }

    fn push(&mut self, x: f64) {
        let s0 = x + self.coeff * self.s1 - self.s2;
        self.s2 = self.s1;
        self.s1 = s0;
    }

    /// The power at the frequency over the block, and a reset.
    fn take(&mut self) -> f64 {
        let p = self.s1 * self.s1 + self.s2 * self.s2 - self.coeff * self.s1 * self.s2;
        self.s1 = 0.0;
        self.s2 = 0.0;
        p.max(0.0)
    }
}

/// A candidate hum frequency with its two reference probes.
#[derive(Debug, Clone)]
struct Candidate {
    hz: f64,
    base: f64,
    below: Probe,
    at: Probe,
    above: Probe,
    /// Sum over blocks of how far the frequency stood over its
    /// neighbours, in dB, and of its amplitude.
    over_db: f64,
    amplitude: f64,
}

/// Looks for mains hum at 50 and 60 Hz and their harmonics: the power at
/// each, over one-second blocks, against the power 4 Hz either side. No
/// transform; one Goertzel recurrence per probe.
#[derive(Debug, Clone)]
pub struct HumDetector {
    channels: usize,
    block: usize,
    filled: usize,
    blocks: usize,
    candidates: Vec<Candidate>,
}

impl HumDetector {
    /// A detector for interleaved audio at `rate` Hz, which it hears as
    /// the mono downmix.
    #[must_use]
    pub fn new(rate: u32, channels: usize) -> Self {
        let r = f64::from(rate);
        let mut candidates = Vec::new();
        for base in [50.0, 60.0] {
            for k in 1..=8 {
                let hz = base * f64::from(k);
                if hz > TOP_HZ || hz + ASIDE_HZ >= r / 2.0 {
                    break;
                }
                candidates.push(Candidate {
                    hz,
                    base,
                    below: Probe::at(r, hz - ASIDE_HZ),
                    at: Probe::at(r, hz),
                    above: Probe::at(r, hz + ASIDE_HZ),
                    over_db: 0.0,
                    amplitude: 0.0,
                });
            }
        }
        Self {
            channels: channels.max(1),
            block: rate.max(1) as usize,
            filled: 0,
            blocks: 0,
            candidates,
        }
    }

    /// Feeds interleaved frames.
    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            let x = frame.iter().map(|s| f64::from(*s)).sum::<f64>() / self.channels as f64;
            for c in &mut self.candidates {
                c.below.push(x);
                c.at.push(x);
                c.above.push(x);
            }
            self.filled += 1;
            if self.filled == self.block {
                self.close_block();
            }
        }
    }

    fn close_block(&mut self) {
        let n = self.block as f64;
        for c in &mut self.candidates {
            let (below, at, above) = (c.below.take(), c.at.take(), c.above.take());
            let aside = f64::midpoint(below, above).max(f64::MIN_POSITIVE);
            c.over_db += 10.0 * (at.max(f64::MIN_POSITIVE) / aside).log10();
            c.amplitude += 2.0 * at.sqrt() / n;
        }
        self.filled = 0;
        self.blocks += 1;
    }

    /// The hum, if the track has one. Whole seconds only are heard, so a
    /// track shorter than one has none.
    #[must_use]
    pub fn hum(&self) -> Option<Hum> {
        if self.blocks == 0 {
            return None;
        }
        let n = self.blocks as f64;
        let mut best: Option<Hum> = None;
        let mut best_over = 0.0;
        for base in [50.0, 60.0] {
            let Some(fundamental) = self
                .candidates
                .iter()
                .find(|c| c.base == base && c.hz == base)
            else {
                continue;
            };
            let over = fundamental.over_db / n;
            let amplitude = fundamental.amplitude / n;
            let level = crate::to_db(amplitude / std::f64::consts::SQRT_2);
            if over < BASE_OVER_DB || level < FLOOR_DBFS || over <= best_over {
                continue;
            }
            let harmonics = self
                .candidates
                .iter()
                .filter(|c| {
                    c.base == base
                        && (c.hz == base
                            || (c.over_db / n >= HARMONIC_OVER_DB
                                && crate::to_db(c.amplitude / n / std::f64::consts::SQRT_2)
                                    >= HARMONIC_FLOOR_DBFS))
                })
                .map(|c| c.hz)
                .collect();
            best_over = over;
            best = Some(Hum {
                base_hz: base,
                harmonics,
                level_dbfs: level,
            });
        }
        best
    }
}

/// The filters, one chain per channel: the high-pass, then a notch on
/// each harmonic of the hum, where there is one.
#[derive(Debug, Clone)]
pub struct Hygiene {
    channels: usize,
    chains: Vec<Vec<Biquad>>,
}

impl Hygiene {
    /// Filters for interleaved audio at `rate` Hz.
    #[must_use]
    pub fn new(rate: u32, channels: usize, hum: Option<&Hum>) -> Self {
        let r = f64::from(rate);
        let mut chain = vec![Biquad::high_pass(
            r,
            HIGH_PASS_HZ,
            std::f64::consts::FRAC_1_SQRT_2,
        )];
        if let Some(hum) = hum {
            for &hz in &hum.harmonics {
                chain.push(Biquad::notch(r, hz, NOTCH_BANDWIDTH_HZ));
            }
        }
        let channels = channels.max(1);
        Self {
            channels,
            chains: vec![chain; channels],
        }
    }

    /// How many notches the chain carries.
    #[must_use]
    pub fn notches(&self) -> usize {
        self.chains.first().map_or(0, |c| c.len() - 1)
    }

    /// Filters interleaved frames in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        for frame in samples.chunks_exact_mut(self.channels) {
            for (s, chain) in frame.iter_mut().zip(&mut self.chains) {
                let mut v = f64::from(*s);
                for stage in chain.iter_mut() {
                    v = stage.run(v);
                }
                *s = v as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, seconds: f64, hz: f64, amplitude: f64) -> Vec<f32> {
        (0..(f64::from(rate) * seconds) as usize)
            .map(|i| {
                (amplitude
                    * (2.0 * std::f64::consts::PI * hz * f64::from(i as u32) / f64::from(rate))
                        .sin()) as f32
            })
            .collect()
    }

    fn add(a: &mut [f32], b: &[f32]) {
        for (x, y) in a.iter_mut().zip(b) {
            *x += y;
        }
    }

    fn amplitude_at(rate: u32, hz: f64, s: &[f32]) -> f64 {
        let mut p = Probe::at(f64::from(rate), hz);
        for x in s {
            p.push(f64::from(*x));
        }
        2.0 * p.take().sqrt() / s.len() as f64
    }

    #[test]
    fn hum_under_a_tone_is_found_with_its_harmonics() {
        let rate = 48_000;
        let mut s = sine(rate, 4.0, 1000.0, 0.1);
        add(&mut s, &sine(rate, 4.0, 60.0, 0.01));
        add(&mut s, &sine(rate, 4.0, 120.0, 0.005));
        add(&mut s, &sine(rate, 4.0, 180.0, 0.002));
        let mut d = HumDetector::new(rate, 1);
        d.push(&s);
        let hum = d.hum().expect("hum");
        assert_eq!(hum.base_hz, 60.0);
        assert_eq!(hum.harmonics, vec![60.0, 120.0, 180.0]);
        assert!((hum.level_dbfs - -43.0).abs() < 0.5, "{}", hum.level_dbfs);
    }

    #[test]
    fn a_plain_tone_and_noise_have_no_hum() {
        let rate = 48_000;
        let mut d = HumDetector::new(rate, 2);
        let mono = sine(rate, 3.0, 440.0, 0.3);
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        d.push(&stereo);
        assert_eq!(d.hum(), None);
        // White-ish noise: a linear congruential generator, no hum in it.
        let mut x = 12345u32;
        let noise: Vec<f32> = (0..rate * 3)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 8) as f32 / 16_777_216.0 - 0.5
            })
            .collect();
        let mut d = HumDetector::new(rate, 1);
        d.push(&noise);
        assert_eq!(d.hum(), None);
    }

    #[test]
    fn the_notches_take_the_hum_and_leave_the_voice() {
        let rate = 48_000;
        let mut s = sine(rate, 4.0, 1000.0, 0.1);
        add(&mut s, &sine(rate, 4.0, 50.0, 0.02));
        add(&mut s, &sine(rate, 4.0, 100.0, 0.01));
        let mut d = HumDetector::new(rate, 1);
        d.push(&s);
        let hum = d.hum().expect("hum");
        let mut h = Hygiene::new(rate, 1, Some(&hum));
        assert_eq!(h.notches(), 2);
        let mut out = s.clone();
        h.process(&mut out);
        // Judged on the last two seconds, once the filters have settled.
        let tail = &out[rate as usize * 2..];
        let before = amplitude_at(rate, 50.0, &s[rate as usize * 2..]);
        let after = amplitude_at(rate, 50.0, tail);
        assert!(crate::to_db(after / before) < -30.0, "{before} {after}");
        let voice = amplitude_at(rate, 1000.0, tail);
        assert!((crate::to_db(voice / 0.1)).abs() < 0.1, "{voice}");
    }

    #[test]
    fn the_high_pass_cuts_rumble_and_passes_speech() {
        let rate = 48_000;
        let mut h = Hygiene::new(rate, 1, None);
        let mut rumble = sine(rate, 3.0, 30.0, 0.5);
        h.process(&mut rumble);
        let left = amplitude_at(rate, 30.0, &rumble[rate as usize..]);
        assert!(crate::to_db(left / 0.5) < -15.0, "{left}");
        let mut h = Hygiene::new(rate, 1, None);
        let mut voice = sine(rate, 3.0, 300.0, 0.5);
        h.process(&mut voice);
        let kept = amplitude_at(rate, 300.0, &voice[rate as usize..]);
        assert!(crate::to_db(kept / 0.5).abs() < 0.2, "{kept}");
    }
}
