//! Keyframe animation for the Geneva engine.
//!
//! Every animated value is a pure function of time: given the same track and
//! the same time, sampling returns the same value, on any machine. Nothing in
//! this crate reads a clock or keeps state between samples.
//!
//! The two building blocks are [`Easing`], a curve mapping normalized
//! progress in `[0, 1]` to an output that may overshoot, and [`Track`], a
//! sorted list of keyframes that interpolates between neighbouring values.

#![forbid(unsafe_code)]

mod easing;
mod track;

pub use easing::{Easing, NamedEasing, Spring, StepPosition};
pub use track::{Interpolate, Keyframe, Track};
