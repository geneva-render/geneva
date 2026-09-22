//! Rendering straight to encoder planes.
//!
//! `geneva render` wants the encoder's planes, not an f32 frame:
//! [`PlaneRenderer`] renders a frame and packs it in one call, so a
//! renderer with a device of its own can do the pack there and read
//! back three bytes a pixel rather than sixteen. [`FramePacker`] is the
//! implementation over any [`Renderer`]: it renders into a frame it keeps
//! and packs on the CPU with [`frame_to_planes_into`].

use geneva_color::{Color, ResolvedTags};
use geneva_render::{Frame, RenderError, Renderer};
use geneva_timeline::{Composition, Ratio};

use crate::convert::{PlaneFormat, Planes, frame_to_planes_into};

/// One set of planes to fill: the output's tags and layout, and the
/// buffer laid out for them at the composition's size.
pub struct PlaneTarget<'a> {
    /// The output's color tags.
    pub tags: ResolvedTags,
    /// The output's sample layout.
    pub format: PlaneFormat,
    /// Where the frame goes, laid out for `format` at the frame's size.
    pub planes: &'a mut Planes,
}

/// Renders frames and packs them for encoders.
pub trait PlaneRenderer: Renderer {
    /// Renders the frame at `t` into every target, each packed for its
    /// own tags and layout, and into `frame` too when one is given (a
    /// picture wants the frame whole).
    fn render_planes(
        &mut self,
        comp: &Composition,
        t: Ratio,
        targets: &mut [PlaneTarget<'_>],
        frame: Option<&mut Frame>,
    ) -> Result<(), RenderError>;

    /// A word that [`render_planes`](Self::render_planes) will be asked
    /// for this frame next, with these tags and layouts, so a renderer
    /// that can may start it now and have it ready. Nothing by default;
    /// a renderer that ignores it is only slower.
    fn prepare_planes(
        &mut self,
        comp: &Composition,
        t: Ratio,
        outputs: &[(ResolvedTags, PlaneFormat)],
    ) {
        let _ = (comp, t, outputs);
    }
}

/// Any renderer, packed on the CPU: the frame is rendered into a buffer
/// kept between frames and converted with [`frame_to_planes_into`].
pub struct FramePacker<R: Renderer> {
    renderer: R,
    frame: Frame,
}

impl<R: Renderer> FramePacker<R> {
    /// Packs the frames `renderer` draws.
    pub fn new(renderer: R) -> Self {
        Self {
            renderer,
            frame: Frame::new(0, 0, Color::BLACK),
        }
    }

    /// The renderer inside.
    pub fn renderer_mut(&mut self) -> &mut R {
        &mut self.renderer
    }
}

impl<R: Renderer> std::fmt::Debug for FramePacker<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FramePacker").finish_non_exhaustive()
    }
}

impl<R: Renderer> Renderer for FramePacker<R> {
    fn take_warnings(&mut self) -> Vec<String> {
        // A wrapper that swallowed these would make the renderer inside
        // it silent, which is the failure this reports in the first place.
        self.renderer.take_warnings()
    }

    fn render_into(
        &mut self,
        comp: &Composition,
        t: Ratio,
        frame: &mut Frame,
    ) -> Result<(), RenderError> {
        self.renderer.render_into(comp, t, frame)
    }
}

impl<R: Renderer> PlaneRenderer for FramePacker<R> {
    fn render_planes(
        &mut self,
        comp: &Composition,
        t: Ratio,
        targets: &mut [PlaneTarget<'_>],
        frame: Option<&mut Frame>,
    ) -> Result<(), RenderError> {
        // The caller's frame is drawn into directly when there is one;
        // the kept one otherwise.
        let mut own = std::mem::replace(&mut self.frame, Frame::new(0, 0, Color::BLACK));
        let target: &mut Frame = match frame {
            Some(f) => f,
            None => &mut own,
        };
        let drawn = self.renderer.render_into(comp, t, target);
        if drawn.is_ok() {
            for t in targets.iter_mut() {
                frame_to_planes_into(target, t.tags, t.format, t.planes);
            }
        }
        self.frame = own;
        drawn
    }
}
