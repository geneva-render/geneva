//! Color handling for the Geneva engine.
//!
//! Decoders hand over coded pixels plus metadata tags that are often missing
//! or wrong. Everything from that point on is this crate's job: deciding what
//! untagged material most likely is ([`infer`]), converting between Y'CbCr
//! and R'G'B' ([`matrix`]), linearizing and re-encoding ([`transfer`]), and
//! blending in linear light ([`LinearRgba`]).
//!
//! The working space for compositing is linear light with BT.709 primaries,
//! premultiplied alpha, `f32` per channel.

#![forbid(unsafe_code)]

mod css;
mod infer;
pub mod matrix;
mod pixel;
mod tags;
pub mod transfer;

pub use css::ColorParseError;
pub use infer::{Inference, infer};
pub use pixel::{Color, LinearRgba};
pub use tags::{ColorTags, Matrix, Primaries, Range, ResolvedTags, Transfer};
