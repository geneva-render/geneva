//! The GPU renderer of the Geneva engine.
//!
//! [`GpuRenderer`] is a second implementation of [`Renderer`] with the
//! same contract as the CPU reference: a pure function of the
//! composition, the time and the assets. It composites on a device
//! reached through `wgpu` (Vulkan on Linux, Metal on macOS, DirectX 12 on
//! Windows), in premultiplied linear light held as 16-bit floats, and is
//! checked against the CPU renderer by the golden harness under its
//! tolerance rather than byte for byte: the same device gives the same
//! frame every run, and two devices agree to the tolerance.
//!
//! What draws so far: everything but the blur effect. Solids, shapes,
//! image assets, text, markup, video and nested compositions, placed
//! through the same [`geneva_render::Placement`] as the reference and
//! sampled by the same rules, through shape and luma masks, at the
//! clip's opacity with its transitions, in every blend mode of the
//! format, with a fade's veil over the result, and the frame read back
//! into a [`Frame`]. Text and markup are painted on the CPU by the
//! shared [`geneva_render::Painter`] and uploaded; an 8-bit 4:2:0 video
//! frame is uploaded as its planes and converted on the device. A
//! blurred clip is refused with [`RenderError::Unsupported`] until its
//! step of the plan. [`Gpu::probe`] picks the adapter, checks what the
//! compositor needs of it, and describes it in a [`Report`].

#![forbid(unsafe_code)]

mod compositor;
mod device;

use geneva_render::{AssetSource, Frame, Painter, RenderError, Renderer};
use geneva_timeline::{Composition, Ratio};

pub use device::{Gpu, GpuError, Preference, Report};

/// Renders frames on a device.
pub struct GpuRenderer<A: AssetSource> {
    gpu: Gpu,
    painter: Painter<A>,
    compositor: compositor::Compositor,
}

impl<A: AssetSource> GpuRenderer<A> {
    /// A renderer on `gpu`, reading assets from `assets`.
    pub fn new(gpu: Gpu, assets: A) -> Self {
        let compositor = compositor::Compositor::new(&gpu);
        Self {
            gpu,
            painter: Painter::new(assets),
            compositor,
        }
    }

    /// The device this renderer draws on.
    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The asset source, for registering fonts or media.
    pub fn assets_mut(&mut self) -> &mut A {
        self.painter.assets_mut()
    }
}

impl<A: AssetSource> std::fmt::Debug for GpuRenderer<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuRenderer")
            .field("device", &self.gpu.report().name)
            .finish_non_exhaustive()
    }
}

impl<A: AssetSource> Renderer for GpuRenderer<A> {
    fn render_into(
        &mut self,
        comp: &Composition,
        t: Ratio,
        frame: &mut Frame,
    ) -> Result<(), RenderError> {
        if t < Ratio::ZERO || t >= comp.duration {
            return Err(RenderError::OutOfRange {
                time: t,
                duration: comp.duration,
            });
        }
        self.compositor
            .render(&self.gpu, &mut self.painter, comp, t, frame)
    }
}
