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
//! What exists so far is the device: [`Gpu::probe`] picks an adapter,
//! checks what the compositor needs of it, and describes it in a
//! [`Report`]. Nothing draws yet, and [`Renderer::render_into`] says so.

#![forbid(unsafe_code)]

mod device;

use geneva_render::{AssetSource, Frame, RenderError, Renderer};
use geneva_timeline::{Composition, Ratio};

pub use device::{Gpu, GpuError, Preference, Report};

/// Renders frames on a device.
pub struct GpuRenderer<A: AssetSource> {
    gpu: Gpu,
    assets: A,
}

impl<A: AssetSource> GpuRenderer<A> {
    /// A renderer on `gpu`, reading assets from `assets`.
    pub fn new(gpu: Gpu, assets: A) -> Self {
        Self { gpu, assets }
    }

    /// The device this renderer draws on.
    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The asset source, for registering fonts or media.
    pub fn assets_mut(&mut self) -> &mut A {
        &mut self.assets
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
        _frame: &mut Frame,
    ) -> Result<(), RenderError> {
        if t < Ratio::ZERO || t >= comp.duration {
            return Err(RenderError::OutOfRange {
                time: t,
                duration: comp.duration,
            });
        }
        Err(RenderError::Backend {
            reason: format!(
                "the GPU renderer draws nothing yet (device: {})",
                self.gpu.report().name
            ),
        })
    }
}
