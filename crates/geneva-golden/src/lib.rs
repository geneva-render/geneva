//! Golden-frame testing for the Geneva engine.
//!
//! Rendered frames are compared against reference images with perceptual
//! tolerance rather than byte equality, because the same scene rendered on
//! different hardware legitimately differs in the last bits of a channel.
//! [`compare`] reports per-channel error, PSNR and SSIM; [`Tolerance`]
//! decides what counts as a pass. [`run_case`] ties this to a renderer and a
//! directory of scenes.

#![forbid(unsafe_code)]

mod compare;
mod runner;

pub use compare::{Comparison, Rgba8Image, Tolerance, compare, diff_image};
pub use runner::{CaseOutcome, FrameOutcome, GoldenError, discover_cases, run_case};
