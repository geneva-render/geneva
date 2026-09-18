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
mod blur;
mod cpu;
mod fill;
mod frame;
mod html;
mod painter;
mod placement;
mod text;
mod transitions;

use geneva_color::Color;
use geneva_timeline::{Composition, Ratio};
use thiserror::Error;

pub use assets::{AssetSource, FileAssets, Image, NoAssets, VideoPlanes};
pub use blur::{box_radii, gaussian_blur};
pub use cpu::CpuRenderer;
pub use frame::Frame;
pub use painter::{Paint, Painted, Painter};
pub use placement::{Placement, SUBSAMPLES, crop_window};
pub use text::TextEngine;
pub use transitions::{fade_veil, transition_gain};

/// Errors that prevent a frame from being rendered.
#[derive(Debug, Error)]
pub enum RenderError {
    /// A source kind, or a feature, this renderer does not implement
    /// yet. The CPU renderer implements everything; the GPU renderer
    /// refuses what it does not draw yet, by name.
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
    /// The renderer's device failed, or cannot do what the frame needs:
    /// a GPU that was lost, or ran out of memory, or a renderer that
    /// does not draw yet. The CPU renderer never returns it.
    #[error("the renderer's device failed: {reason}")]
    Backend {
        /// What went wrong.
        reason: String,
    },
}

impl RenderError {
    /// The diagnostic code matching this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "E500",
            Self::Asset { .. } => "E501",
            Self::OutOfRange { .. } => "E502",
            Self::Backend { .. } => "E503",
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
