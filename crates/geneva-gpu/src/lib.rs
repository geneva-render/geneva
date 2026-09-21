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
//! What draws: everything the CPU renderer draws. Solids, shapes,
//! image assets, text, markup, video and nested compositions, placed
//! through the same [`geneva_render::Placement`] as the reference and
//! sampled by the same rules, through shape and luma masks, at the
//! clip's opacity with its transitions, in every blend mode of the
//! format, with a fade's veil over the result, and the frame read back
//! into a [`Frame`]. Text is painted on the CPU by the shared
//! [`geneva_render::Painter`] and uploaded; a markup box's boxes are
//! painted by the same painter and its groups (opacity, blur, clips,
//! transforms) composited on the device, in the painter's encoded
//! space, then decoded to linear light there; an 8-bit 4:2:0 video
//! frame is uploaded as its planes and converted on the device; a
//! blurred clip is drawn onto a layer and blurred by the same three box
//! blurs. [`Gpu::probe`] picks the adapter, checks what the compositor
//! needs of it, and describes it in a [`Report`].

#![forbid(unsafe_code)]

mod compositor;
mod device;

use std::collections::VecDeque;

use geneva_color::ResolvedTags;
use geneva_media::convert::PlaneFormat;
use geneva_media::{PlaneRenderer, PlaneTarget};
use geneva_render::{AssetSource, Frame, Painter, RenderError, Renderer};
use geneva_timeline::{Composition, Ratio};

pub use device::{Gpu, GpuError, Preference, Report, hint};

/// Renders frames on a device.
///
/// As a [`PlaneRenderer`] it packs the frame into the encoder's planes
/// on the device too, and reads back three bytes a pixel rather than
/// sixteen. [`PlaneRenderer::prepare_planes`] submits the next frame
/// while the current one is read back, so the painting and uploading of
/// one frame overlap the drawing of the other; up to two frames are held
/// in flight this way, and a frame asked for that is not the prepared
/// one is drawn afresh.
pub struct GpuRenderer<A: AssetSource> {
    gpu: Gpu,
    painter: Painter<A>,
    compositor: compositor::Compositor,
    /// Frames submitted ahead, oldest first.
    pending: VecDeque<Pending>,
}

/// A frame in flight and what it was submitted for.
struct Pending {
    key: Key,
    job: compositor::Job,
}

/// What a prepared frame answers to.
#[derive(PartialEq)]
struct Key {
    /// The composition, by address: prepared for one document, drawn
    /// for the same one.
    comp: usize,
    width: u32,
    height: u32,
    t: Ratio,
    outputs: Vec<(ResolvedTags, PlaneFormat)>,
}

impl<A: AssetSource> GpuRenderer<A> {
    /// A renderer on `gpu`, reading assets from `assets`.
    pub fn new(gpu: Gpu, assets: A) -> Self {
        let compositor = compositor::Compositor::new(&gpu);
        Self {
            gpu,
            painter: Painter::new(assets),
            compositor,
            pending: VecDeque::new(),
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

    /// Gives up every frame prepared ahead.
    fn drop_pending(&mut self) {
        while let Some(p) = self.pending.pop_front() {
            self.compositor.release(p.job);
        }
    }

    fn key(comp: &Composition, t: Ratio, outputs: &[(ResolvedTags, PlaneFormat)]) -> Key {
        Key {
            comp: std::ptr::from_ref(comp) as usize,
            width: comp.width,
            height: comp.height,
            t,
            outputs: outputs.to_vec(),
        }
    }
}

impl<A: AssetSource> PlaneRenderer for GpuRenderer<A> {
    fn render_planes(
        &mut self,
        comp: &Composition,
        t: Ratio,
        targets: &mut [PlaneTarget<'_>],
        frame: Option<&mut Frame>,
    ) -> Result<(), RenderError> {
        if t < Ratio::ZERO || t >= comp.duration {
            return Err(RenderError::OutOfRange {
                time: t,
                duration: comp.duration,
            });
        }
        let outputs: Vec<(ResolvedTags, PlaneFormat)> =
            targets.iter().map(|p| (p.tags, p.format)).collect();
        let key = Self::key(comp, t, &outputs);
        // Frames prepared for another document, or for an earlier time,
        // are never asked for again.
        while let Some(p) = self.pending.front() {
            if p.key.comp != key.comp || p.key.t < t {
                let p = self.pending.pop_front().expect("checked above");
                self.compositor.release(p.job);
            } else {
                break;
            }
        }
        // The frame prepared ahead is this one when it was asked for
        // the same time and outputs, and without the frame whole.
        let ready = frame.is_none() && self.pending.front().is_some_and(|p| p.key == key);
        let job = if ready {
            self.pending.pop_front().expect("checked above").job
        } else {
            self.compositor.submit(
                &self.gpu,
                &mut self.painter,
                comp,
                t,
                &outputs,
                frame.is_some(),
            )?
        };
        self.compositor.finish(&self.gpu, job, targets, frame);
        Ok(())
    }

    fn prepare_planes(
        &mut self,
        comp: &Composition,
        t: Ratio,
        outputs: &[(ResolvedTags, PlaneFormat)],
    ) {
        if t < Ratio::ZERO || t >= comp.duration || self.pending.len() >= 2 {
            return;
        }
        let key = Self::key(comp, t, outputs);
        if self.pending.iter().any(|p| p.key == key) {
            return;
        }
        // An error here comes again from the render that follows, where
        // it is reported.
        if let Ok(job) =
            self.compositor
                .submit(&self.gpu, &mut self.painter, comp, t, outputs, false)
        {
            self.pending.push_back(Pending { key, job });
        }
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
        self.drop_pending();
        self.compositor
            .render(&self.gpu, &mut self.painter, comp, t, frame)
    }
}
