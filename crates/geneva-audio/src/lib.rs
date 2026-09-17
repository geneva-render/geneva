//! Audio measurement and gain: the loudness meter of ITU-R BS.1770-4,
//! true peak by oversampling, a limiter that holds a true-peak ceiling,
//! a rumble high-pass with hum notches, and the analysis a report is
//! made from. Pure arithmetic on interleaved `f32` frames, no I/O and no
//! clocks, so the same samples in give the same samples out on every
//! machine, block boundaries included.
//!
//! Loudness is reported in LUFS (the same scale as LKFS) and peaks in
//! dBTP, both relative to full scale.

#![forbid(unsafe_code)]

mod analysis;
mod biquad;
mod hygiene;
mod limiter;
mod loudness;
mod peak;

pub use analysis::{Analysis, Report};
pub use hygiene::{HIGH_PASS_HZ, Hum, HumDetector, Hygiene, NOTCH_BANDWIDTH_HZ};
pub use limiter::Limiter;
pub use loudness::{Meter, integrated};
pub use peak::{TruePeak, true_peak};

/// A linear amplitude as decibels relative to full scale.
#[must_use]
pub fn to_db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(f64::MIN_POSITIVE).log10()
}

/// Decibels relative to full scale as a linear amplitude.
#[must_use]
pub fn from_db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}
