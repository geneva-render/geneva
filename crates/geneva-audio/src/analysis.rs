//! What a track measures, for a report before anything is rendered:
//! loudness, true peak, clipping, DC offset, noise floor, hum, silence
//! and a stereo file that is really mono.

use crate::hygiene::{Hum, HumDetector};
use crate::loudness::Meter;
use crate::peak::TruePeak;

/// A sample at or over this counts as clipped (full scale less than a
/// step of 16 bits).
const CLIP_LEVEL: f32 = 0.999;
/// A run of clipped samples this long or longer counts as a clip.
const CLIP_RUN: usize = 3;

/// The measurements of a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Sample rate the track was measured at.
    pub rate: u32,
    /// Channels measured.
    pub channels: usize,
    /// Length in seconds.
    pub seconds: f64,
    /// Integrated loudness in LUFS, or `None` for silence.
    pub lufs: Option<f64>,
    /// True peak in dBTP.
    pub true_peak_dbtp: f64,
    /// Runs of three or more samples at full scale.
    pub clipped_runs: usize,
    /// The longest such run, in samples.
    pub longest_clip: usize,
    /// Where the first one starts, in seconds.
    pub first_clip_secs: Option<f64>,
    /// The largest mean sample value of any channel, as a fraction of
    /// full scale.
    pub dc_offset: f64,
    /// Level of the quietest tenth of 100 ms windows, in dBFS, leaving
    /// out digital silence; `None` when there is nothing else.
    pub noise_floor_dbfs: Option<f64>,
    /// Level of the loudest tenth of windows, in dBFS.
    pub signal_dbfs: Option<f64>,
    /// `signal_dbfs` over `noise_floor_dbfs`.
    pub snr_db: Option<f64>,
    /// A two-channel track whose channels are the same signal.
    pub dual_mono: bool,
    /// Nothing over -60 dBTP.
    pub silent: bool,
    /// Mains hum, where there is any.
    pub hum: Option<Hum>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Clipping {
    run: usize,
    runs: usize,
    longest: usize,
    first: Option<u64>,
}

impl Clipping {
    fn push(&mut self, s: f32, frame: u64) {
        if s.abs() >= CLIP_LEVEL {
            self.run += 1;
            if self.run == CLIP_RUN {
                self.runs += 1;
                self.first.get_or_insert(frame + 1 - CLIP_RUN as u64);
            }
            self.longest = self.longest.max(self.run);
        } else {
            self.run = 0;
        }
    }
}

/// Measures a track fed a block at a time.
#[derive(Debug, Clone)]
pub struct Analysis {
    rate: u32,
    channels: usize,
    frames: u64,
    meter: Meter,
    peak: TruePeak,
    highest: f32,
    hum: HumDetector,
    dc: Vec<f64>,
    clipping: Vec<Clipping>,
    window: usize,
    filled: usize,
    energy: f64,
    windows: Vec<f64>,
    mid: f64,
    side: f64,
}

impl Analysis {
    /// An analysis of interleaved audio at `rate` Hz.
    #[must_use]
    pub fn new(rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            rate,
            channels,
            frames: 0,
            meter: Meter::new(rate, channels),
            peak: TruePeak::new(rate, channels),
            highest: 0.0,
            hum: HumDetector::new(rate, channels),
            dc: vec![0.0; channels],
            clipping: vec![Clipping::default(); channels],
            window: (rate as usize / 10).max(1),
            filled: 0,
            energy: 0.0,
            windows: Vec::new(),
            mid: 0.0,
            side: 0.0,
        }
    }

    /// Feeds interleaved frames.
    pub fn push(&mut self, samples: &[f32]) {
        self.meter.push(samples);
        self.hum.push(samples);
        for frame in samples.chunks_exact(self.channels) {
            self.highest = self.highest.max(self.peak.push(frame));
            let mut mono = 0.0f64;
            for (c, &s) in frame.iter().enumerate() {
                self.dc[c] += f64::from(s);
                self.clipping[c].push(s, self.frames);
                mono += f64::from(s);
            }
            mono /= self.channels as f64;
            self.energy += mono * mono;
            if self.channels == 2 {
                let (l, r) = (f64::from(frame[0]), f64::from(frame[1]));
                self.mid += (l + r) * (l + r);
                self.side += (l - r) * (l - r);
            }
            self.filled += 1;
            if self.filled == self.window {
                self.windows.push(self.energy / self.window as f64);
                self.energy = 0.0;
                self.filled = 0;
            }
            self.frames += 1;
        }
    }

    /// The measurements of everything fed so far.
    #[must_use]
    pub fn report(&self) -> Report {
        let rate = f64::from(self.rate);
        let dbfs = |mean_square: f64| 10.0 * mean_square.max(f64::MIN_POSITIVE).log10();
        let mut sounding: Vec<f64> = self.windows.iter().copied().filter(|e| *e > 0.0).collect();
        sounding.sort_by(f64::total_cmp);
        let at = |fraction: f64| {
            sounding
                .get(
                    ((sounding.len() as f64 * fraction) as usize)
                        .min(sounding.len().saturating_sub(1)),
                )
                .copied()
                .map(dbfs)
        };
        let (noise_floor_dbfs, signal_dbfs) = if sounding.is_empty() {
            (None, None)
        } else {
            (at(0.1), at(0.9))
        };
        let clipped_runs = self.clipping.iter().map(|c| c.runs).sum();
        let first_clip = self.clipping.iter().filter_map(|c| c.first).min();
        Report {
            rate: self.rate,
            channels: self.channels,
            seconds: self.frames as f64 / rate,
            lufs: self.meter.integrated(),
            true_peak_dbtp: crate::to_db(f64::from(self.highest)),
            clipped_runs,
            longest_clip: self.clipping.iter().map(|c| c.longest).max().unwrap_or(0),
            first_clip_secs: first_clip.map(|f| f as f64 / rate),
            dc_offset: self
                .dc
                .iter()
                .map(|sum| (sum / self.frames.max(1) as f64).abs())
                .fold(0.0, f64::max),
            noise_floor_dbfs,
            signal_dbfs,
            snr_db: signal_dbfs.zip(noise_floor_dbfs).map(|(s, n)| s - n),
            dual_mono: self.channels == 2 && self.mid > 0.0 && self.side / self.mid < 1e-4,
            silent: self.highest < 1e-3,
            hum: self.hum.hum(),
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

    #[test]
    fn clipping_dc_and_dual_mono_are_reported() {
        let rate = 16_000;
        let mono = sine(rate, 2.0, 440.0, 0.5);
        let mut stereo: Vec<f32> = mono.iter().flat_map(|s| [*s + 0.02, *s + 0.02]).collect();
        // Two clipped runs on the left channel.
        for i in [8_000usize, 12_000] {
            for k in 0..10 {
                stereo[(i + k) * 2] = 1.0;
            }
        }
        let mut a = Analysis::new(rate, 2);
        a.push(&stereo);
        let r = a.report();
        assert_eq!(r.clipped_runs, 2);
        assert_eq!(r.longest_clip, 10);
        assert!((r.first_clip_secs.unwrap() - 0.5).abs() < 1e-6);
        assert!((r.dc_offset - 0.02).abs() < 1e-3, "{}", r.dc_offset);
        // The clipped samples on one side are a difference between the
        // channels; without them the pair is one signal twice.
        assert!(!r.dual_mono);
        assert!(!r.silent);
        assert!((r.seconds - 2.0).abs() < 1e-9);
        assert!(r.hum.is_none());
        let same: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        let mut a = Analysis::new(rate, 2);
        a.push(&same);
        assert!(a.report().dual_mono);
    }

    #[test]
    fn floor_and_signal_come_from_the_quiet_and_loud_windows() {
        let rate = 16_000;
        // A second of tone at -20 dBFS, a second at -50, and a second of
        // digital silence that does not count.
        let mut s = sine(rate, 1.0, 440.0, 0.1);
        s.extend(sine(rate, 1.0, 440.0, 0.003_16));
        s.extend(vec![0.0; rate as usize]);
        let mut a = Analysis::new(rate, 1);
        a.push(&s);
        let r = a.report();
        assert!(
            (r.signal_dbfs.unwrap() - -23.0).abs() < 0.5,
            "{:?}",
            r.signal_dbfs
        );
        assert!(
            (r.noise_floor_dbfs.unwrap() - -53.0).abs() < 0.5,
            "{:?}",
            r.noise_floor_dbfs
        );
        assert!((r.snr_db.unwrap() - 30.0).abs() < 1.0);
        assert!(!r.dual_mono);
    }

    #[test]
    fn silence_is_silent() {
        let mut a = Analysis::new(48_000, 2);
        a.push(&vec![0.0; 96_000]);
        let r = a.report();
        assert!(r.silent);
        assert_eq!(r.lufs, None);
        assert_eq!(r.noise_floor_dbfs, None);
    }
}
