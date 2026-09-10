//! Frame rendering for the Geneva engine.
//!
//! A [`Renderer`] turns a resolved [`Composition`] and a time into a
//! [`Frame`]: a premultiplied, linear-light RGBA buffer. Rendering is a pure
//! function of the composition, the time and the assets; the same inputs
//! always produce the same frame.
//!
//! [`CpuRenderer`] is the reference implementation. It is written for
//! clarity and exactness rather than speed and serves as the baseline that
//! faster backends are compared against.

#![forbid(unsafe_code)]

mod assets;
mod cpu;
mod frame;
mod text;

use geneva_color::Color;
use geneva_timeline::{Composition, Ratio};
use thiserror::Error;

pub use assets::{AssetSource, FileAssets, Image, NoAssets};
pub use cpu::CpuRenderer;
pub use frame::Frame;
pub use text::TextEngine;

/// Errors that prevent a frame from being rendered.
#[derive(Debug, Error)]
pub enum RenderError {
    /// A source kind this renderer does not implement yet.
    #[error("{what} is not supported by this renderer (clip at {path})")]
    Unsupported {
        /// Description of the unsupported feature.
        what: String,
        /// JSON pointer of the clip.
        path: String,
    },
    /// An asset could not be read.
    #[error("asset {id:?} could not be loaded: {reason}")]
    Asset {
        /// Asset id.
        id: String,
        /// What went wrong.
        reason: String,
    },
    /// The requested time is outside the composition.
    #[error("time {time}s is outside the composition (0s to {duration}s)")]
    OutOfRange {
        /// Requested time.
        time: Ratio,
        /// Composition duration.
        duration: Ratio,
    },
}

impl RenderError {
    /// The diagnostic code matching this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "E500",
            Self::Asset { .. } => "E501",
            Self::OutOfRange { .. } => "E502",
        }
    }
}

/// Renders single frames of a composition.
pub trait Renderer {
    /// Renders the frame shown at time `t` into `frame`, which is resized
    /// and cleared first; passing the same frame back keeps its buffer.
    fn render_into(
        &mut self,
        comp: &Composition,
        t: Ratio,
        frame: &mut Frame,
    ) -> Result<(), RenderError>;

    /// Renders the frame shown at time `t`.
    fn render_frame(&mut self, comp: &Composition, t: Ratio) -> Result<Frame, RenderError> {
        let mut frame = Frame::new(0, 0, Color::BLACK);
        self.render_into(comp, t, &mut frame)?;
        Ok(frame)
    }
}
