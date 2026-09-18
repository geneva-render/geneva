//! Drawing a frame: the pipelines, the per-clip uniforms, the textures
//! (image assets, luma masks, nested compositions, the backdrop copy a
//! separable blend reads), the target and its readback.
//!
//! What draws here is what `composite.wgsl` transcribes from the CPU
//! renderer: solids, shapes, still images and nested compositions,
//! placed through the same [`Placement`] the reference uses, through
//! shape and luma masks, at the clip's opacity with its transitions,
//! blended by any of the format's modes, with a fade's veil over the
//! result. Blur, text, markup and video are refused with
//! [`RenderError::Unsupported`] until their step of the plan.
//!
//! A frame is a list of passes, nested compositions first: each renders
//! its layers into a texture (a pooled one for a nested composition, the
//! frame's own for the top), and a nested composition's texture is then
//! drawn into its parent like an image. Normal and add blending are
//! fixed-function; a separable mode ends the pass, copies the target to
//! a backdrop texture, and draws with a pipeline that replaces the pixel
//! with what the shader composites from the copy.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use geneva_color::{Color, LinearRgba};
use geneva_render::{
    AssetSource, Frame, Image, Placement, RenderError, crop_window, fade_veil, transition_gain,
};
use geneva_timeline::schema::{BlendMode, ShapeKind};
use geneva_timeline::{Composition, Ratio, ResolvedClip, ResolvedLayer, ResolvedSource};
use half::f16;

use crate::device::{Gpu, WORKING_FORMAT};

/// What one draw tells the shader. Mirrors `struct Clip` in
/// `composite.wgsl`, field for field, in its layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ClipUniform {
    bounds: [f32; 4],
    window: [f32; 4],
    interior: [f32; 4],
    fill: [f32; 4],
    stroke: [f32; 4],
    mask_rect: [f32; 4],
    position: [f32; 2],
    anchor: [f32; 2],
    scale: [f32; 2],
    rotation: [f32; 2],
    size: [f32; 2],
    frame: [f32; 2],
    stroke_width: f32,
    radius: f32,
    opacity: f32,
    mask_radius: f32,
    mask_feather: f32,
    kind: u32,
    mode: u32,
    has_stroke: u32,
    blend: u32,
    mask_kind: u32,
    mask_invert: u32,
    _pad: u32,
}

const KIND_SOLID: u32 = 0;
const KIND_RECT: u32 = 1;
const KIND_ELLIPSE: u32 = 2;
const KIND_IMAGE: u32 = 3;

/// Four subsamples for every pixel.
const MODE_SUBSAMPLES: u32 = 0;
/// One center sample for every pixel: the placement is pixel-aligned.
const MODE_CENTER: u32 = 1;
/// One center sample inside the interior, four subsamples elsewhere: an
/// axis-aligned, magnified image, which the CPU renderer resamples span
/// by span with one bilinear sample at the pixel center.
const MODE_SPANS: u32 = 2;

const MASK_NONE: u32 = 0;
const MASK_RECT: u32 = 1;
const MASK_ELLIPSE: u32 = 2;
const MASK_LUMA: u32 = 3;

/// Bytes per pixel of the working format.
const TEXEL_BYTES: u32 = 8;

/// A texture a draw reads.
#[derive(Clone)]
enum Tex {
    /// The 1x1 transparent texture, where the draw has none.
    Blank,
    /// An uploaded asset, by id.
    Asset(String),
    /// A pooled texture of this frame: a nested composition's picture,
    /// or a pass's backdrop copy.
    Pooled(usize),
}

/// How a draw lands on the target.
#[derive(Clone, Copy)]
enum Blending {
    /// Premultiplied "over", fixed-function.
    Normal,
    /// Colors summed, fixed-function.
    Add,
    /// One of the seven separable modes, composited in the shader from
    /// a copy of the target.
    Separable,
    /// A fade's veil: the target blended toward the fill by this
    /// fraction, fixed-function with a blend constant.
    Veil(f32),
}

/// One clip to draw, in order.
struct Draw {
    uniform: ClipUniform,
    blending: Blending,
    image: Tex,
    mask: Tex,
}

/// The layers of one composition drawn into one texture.
struct Pass {
    /// The frame's own target, or a pooled texture.
    target: Option<usize>,
    width: u32,
    height: u32,
    background: Color,
    draws: Vec<Draw>,
}

/// An image asset on the device.
struct Cached {
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    content: Option<[u32; 4]>,
}

/// A texture the frame draws into or copies to, kept for the next
/// frame that needs one of its size.
struct Pooled {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    /// Taken by this frame.
    busy: bool,
}

/// The frame texture and the buffer it is read back through.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    staging: wgpu::Buffer,
    width: u32,
    height: u32,
    /// Bytes per row of the staging buffer, a multiple of 256.
    padded_row: u32,
}

/// Everything a frame is drawn with, kept between frames.
pub(crate) struct Compositor {
    normal: wgpu::RenderPipeline,
    add: wgpu::RenderPipeline,
    separable: wgpu::RenderPipeline,
    veil: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// One slot per draw, `slot` bytes apart, bound at a dynamic offset.
    uniforms: wgpu::Buffer,
    slot: u64,
    capacity: usize,
    /// A 1x1 transparent texture bound where a draw has no image.
    blank: wgpu::TextureView,
    target: Option<Target>,
    images: HashMap<String, Cached>,
    pool: Vec<Pooled>,
    /// The passes of the frame being planned, nested compositions first.
    passes: Vec<Pass>,
}

impl Compositor {
    pub(crate) fn new(gpu: &Gpu) -> Self {
        let device = gpu.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geneva composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let uniform_size = std::mem::size_of::<ClipUniform>() as u64;
        let texture_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geneva clip"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(uniform_size),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geneva composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORKING_FORMAT,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Premultiplied "over": src + dst * (1 - src.a), every channel.
        let normal = pipeline(
            "geneva composite normal",
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        );
        // Add: the colors sum; the alpha is sa + da - sa * da, which is
        // "over" on the alpha channel.
        let add = pipeline(
            "geneva composite add",
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            },
        );
        // The shader composites from the backdrop copy and its result
        // replaces the pixel.
        let separable = pipeline("geneva composite separable", wgpu::BlendState::REPLACE);
        // dst * (1 - a) + fill * a on every channel, `a` the blend
        // constant: Frame::veil.
        let constant = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let veil = pipeline(
            "geneva composite veil",
            wgpu::BlendState {
                color: constant,
                alpha: constant,
            },
        );
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let slot = uniform_size.div_ceil(align) * align;
        let capacity = 64;
        let uniforms = uniform_buffer(device, slot, capacity);
        let blank = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("geneva blank"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORKING_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            normal,
            add,
            separable,
            veil,
            layout,
            uniforms,
            slot,
            capacity,
            blank,
            target: None,
            images: HashMap::new(),
            pool: Vec::new(),
            passes: Vec::new(),
        }
    }

    /// Draws the frame of `comp` at `t` into `frame`.
    pub(crate) fn render<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        assets: &mut A,
        comp: &Composition,
        t: Ratio,
        frame: &mut Frame,
    ) -> Result<(), RenderError> {
        let (width, height) = (comp.width, comp.height);
        frame.reset(width, height, comp.background);
        if width == 0 || height == 0 {
            return Ok(());
        }
        let device = gpu.device();
        if self
            .target
            .as_ref()
            .is_none_or(|t| t.width != width || t.height != height)
        {
            self.target = Some(make_target(device, width, height));
        }
        for p in &mut self.pool {
            p.busy = false;
        }
        self.passes.clear();
        let planned = self.plan(
            gpu,
            assets,
            comp,
            &comp.layers,
            width,
            height,
            comp.background,
            t,
            None,
        );
        if let Err(e) = planned {
            self.passes.clear();
            return Err(e);
        }
        self.execute(gpu);
        self.read_back(gpu, frame);
        Ok(())
    }

    /// Plans the pass that draws `layers` into `target` (the frame's own
    /// when `None`), after the passes of the nested compositions it
    /// shows; images and luma masks are uploaded on the way.
    #[allow(clippy::too_many_arguments)]
    fn plan<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        assets: &mut A,
        comp: &Composition,
        layers: &[ResolvedLayer],
        width: u32,
        height: u32,
        background: Color,
        t: Ratio,
        target: Option<usize>,
    ) -> Result<(), RenderError> {
        let mut draws = Vec::new();
        let frame_size = [width as f32, height as f32];
        for layer in layers {
            for (i, clip) in layer.clips.iter().enumerate() {
                if !(clip.start <= t && t < clip.end) {
                    continue;
                }
                let local = (t - clip.start).to_f64();
                let mut opacity = clip.opacity.sample(local).clamp(0.0, 1.0);
                opacity *= transition_gain(layer, i, t);
                if opacity <= 0.0 {
                    continue;
                }
                if let Some(what) = unsupported(clip) {
                    return Err(RenderError::Unsupported {
                        what: what.to_owned(),
                        path: clip.path.clone(),
                    });
                }
                let mut uniform = ClipUniform {
                    frame: frame_size,
                    opacity: opacity as f32,
                    ..ClipUniform::zeroed()
                };
                let mut image = Tex::Blank;
                let (size, content) = match &clip.source {
                    ResolvedSource::Solid { color } => {
                        uniform.kind = KIND_SOLID;
                        uniform.fill = channels(color.sample(local));
                        ((f64::from(comp.width), f64::from(comp.height)), None)
                    }
                    ResolvedSource::Shape {
                        kind,
                        width,
                        height,
                        fill,
                        stroke,
                        radius,
                    } => {
                        uniform.kind = match kind {
                            ShapeKind::Rect => KIND_RECT,
                            ShapeKind::Ellipse => KIND_ELLIPSE,
                        };
                        uniform.fill = channels(fill.sample(local));
                        if let Some((color, w)) = stroke {
                            uniform.stroke = channels(*color);
                            uniform.stroke_width = *w as f32;
                            uniform.has_stroke = 1;
                        }
                        uniform.radius = *radius as f32;
                        ((*width, *height), None)
                    }
                    ResolvedSource::Image { asset } => {
                        uniform.kind = KIND_IMAGE;
                        let (w, h, content) = self.upload(gpu, assets, comp, asset)?;
                        image = Tex::Asset(asset.clone());
                        ((f64::from(w), f64::from(h)), content)
                    }
                    ResolvedSource::Composition(nested) => {
                        uniform.kind = KIND_IMAGE;
                        let texture = self.acquire(gpu, nested.width, nested.height);
                        self.plan(
                            gpu,
                            assets,
                            comp,
                            &nested.layers,
                            nested.width,
                            nested.height,
                            nested.background,
                            (t - clip.start) * clip.speed,
                            Some(texture),
                        )?;
                        image = Tex::Pooled(texture);
                        ((f64::from(nested.width), f64::from(nested.height)), None)
                    }
                    _ => unreachable!("refused above"),
                };
                let Some(window) = crop_window(clip, size) else {
                    continue;
                };
                let content = content
                    .map(|[x, y, w, h]| [f64::from(x), f64::from(y), f64::from(w), f64::from(h)]);
                let Some(place) = Placement::new(width, height, clip, local, window, content, None)
                else {
                    continue;
                };
                let mut mask = Tex::Blank;
                if let Some(m) = &clip.mask {
                    // The mask's box is in the clip's box, which is the window.
                    let [cx, cy, w, h] = window;
                    let [mx, my, mw, mh] = m.rect_px(w, h);
                    uniform.mask_rect = [cx + mx, cy + my, mw, mh].map(|v| v as f32);
                    uniform.mask_radius = m.radius as f32;
                    uniform.mask_feather = m.feather as f32;
                    uniform.mask_invert = u32::from(m.invert);
                    uniform.mask_kind = match &m.asset {
                        Some(id) => {
                            self.upload(gpu, assets, comp, id)?;
                            mask = Tex::Asset(id.clone());
                            MASK_LUMA
                        }
                        None => match m.shape {
                            ShapeKind::Rect => MASK_RECT,
                            ShapeKind::Ellipse => MASK_ELLIPSE,
                        },
                    };
                }
                uniform.bounds = place.bounds.map(|v| v as f32);
                uniform.window = place.window.map(|v| v as f32);
                uniform.position = place.position.map(|v| v as f32);
                uniform.anchor = place.anchor.map(|v| v as f32);
                uniform.scale = place.scale.map(|v| v as f32);
                uniform.rotation = [place.cos as f32, place.sin as f32];
                uniform.size = [size.0 as f32, size.1 as f32];
                uniform.mode = if place.pixel_aligned {
                    MODE_CENTER
                } else if uniform.kind == KIND_IMAGE
                    && place.axis_aligned()
                    && uniform.mask_kind == MASK_NONE
                    && place.scale[0].abs() >= 1.0
                    && place.scale[1].abs() >= 1.0
                {
                    let interior = place.interior(size.0 as u32, size.1 as u32);
                    uniform.interior = interior.map(|v| v as f32);
                    MODE_SPANS
                } else {
                    MODE_SUBSAMPLES
                };
                let (blending, code) = blending(clip.blend);
                uniform.blend = code;
                draws.push(Draw {
                    uniform,
                    blending,
                    image,
                    mask,
                });
            }
        }
        // A fade dips the picture through a color, so the veil goes over
        // everything the layers drew.
        if let Some((color, alpha)) = fade_veil(layers, t) {
            let alpha = alpha.clamp(0.0, 1.0) as f32;
            if alpha > 0.0 {
                draws.push(Draw {
                    uniform: ClipUniform {
                        bounds: [0.0, 0.0, frame_size[0], frame_size[1]],
                        window: [0.0, 0.0, frame_size[0], frame_size[1]],
                        fill: channels(color),
                        scale: [1.0, 1.0],
                        rotation: [1.0, 0.0],
                        size: frame_size,
                        frame: frame_size,
                        opacity: 1.0,
                        kind: KIND_SOLID,
                        mode: MODE_CENTER,
                        ..ClipUniform::zeroed()
                    },
                    blending: Blending::Veil(alpha),
                    image: Tex::Blank,
                    mask: Tex::Blank,
                });
            }
        }
        self.passes.push(Pass {
            target,
            width,
            height,
            background,
            draws,
        });
        Ok(())
    }

    /// The image asset `id` on the device, uploaded on first use: its
    /// size and content box.
    fn upload<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        assets: &mut A,
        comp: &Composition,
        id: &str,
    ) -> Result<(u32, u32, Option<[u32; 4]>), RenderError> {
        if !self.images.contains_key(id) {
            let img = assets.image(comp, id)?;
            let cached = upload_image(gpu, id, img);
            self.images.insert(id.to_owned(), cached);
        }
        let c = &self.images[id];
        Ok((c.width, c.height, c.content))
    }

    /// A pooled texture of this size for the frame, made when none is free.
    fn acquire(&mut self, gpu: &Gpu, width: u32, height: u32) -> usize {
        let (width, height) = (width.max(1), height.max(1));
        if let Some(i) = self
            .pool
            .iter()
            .position(|p| !p.busy && p.width == width && p.height == height)
        {
            self.pool[i].busy = true;
            return i;
        }
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("geneva layer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.pool.push(Pooled {
            texture,
            view,
            width,
            height,
            busy: true,
        });
        self.pool.len() - 1
    }

    fn view(&self, tex: &Tex) -> &wgpu::TextureView {
        match tex {
            Tex::Blank => &self.blank,
            Tex::Asset(id) => &self.images[id].view,
            Tex::Pooled(i) => &self.pool[*i].view,
        }
    }

    fn texture_of(&self, target: Option<usize>) -> (&wgpu::Texture, &wgpu::TextureView) {
        match target {
            None => {
                let t = self.target.as_ref().expect("made for the frame");
                (&t.texture, &t.view)
            }
            Some(i) => (&self.pool[i].texture, &self.pool[i].view),
        }
    }

    /// Records the planned passes and submits them, with the frame's own
    /// target copied out to the staging buffer at the end.
    fn execute(&mut self, gpu: &Gpu) {
        let device = gpu.device();
        let passes = std::mem::take(&mut self.passes);
        let total: usize = passes.iter().map(|p| p.draws.len()).sum();
        if total > self.capacity {
            let mut capacity = self.capacity;
            while capacity < total {
                capacity *= 2;
            }
            self.uniforms = uniform_buffer(device, self.slot, capacity);
            self.capacity = capacity;
        }
        // Every draw's uniforms go up in one write, each at its slot.
        let mut bytes = vec![0u8; self.slot as usize * total.max(1)];
        for (i, d) in passes.iter().flat_map(|p| &p.draws).enumerate() {
            let at = i * self.slot as usize;
            bytes[at..at + std::mem::size_of::<ClipUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&d.uniform));
        }
        gpu.queue().write_buffer(&self.uniforms, 0, &bytes);
        // A pass with a separable blend reads a copy of its target.
        let backdrops: Vec<Option<usize>> = passes
            .iter()
            .map(|p| {
                p.draws
                    .iter()
                    .any(|d| matches!(d.blending, Blending::Separable))
                    .then(|| self.acquire(gpu, p.width, p.height))
            })
            .collect();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("geneva frame"),
        });
        let mut next_slot = 0u64;
        for (pass, backdrop) in passes.iter().zip(&backdrops) {
            let (target, view) = self.texture_of(pass.target);
            let backdrop_view = backdrop.map_or(&self.blank, |i| &self.pool[i].view);
            let bind_groups: Vec<wgpu::BindGroup> = pass
                .draws
                .iter()
                .map(|d| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("geneva clip"),
                        layout: &self.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &self.uniforms,
                                    offset: 0,
                                    size: wgpu::BufferSize::new(
                                        std::mem::size_of::<ClipUniform>() as u64
                                    ),
                                }),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(self.view(&d.image)),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(self.view(&d.mask)),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(backdrop_view),
                            },
                        ],
                    })
                })
                .collect();
            let clear = pass.background.to_linear();
            let mut rpass = begin(
                &mut encoder,
                view,
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: f64::from(clear.r),
                    g: f64::from(clear.g),
                    b: f64::from(clear.b),
                    a: f64::from(clear.a),
                }),
            );
            for (d, bind_group) in pass.draws.iter().zip(&bind_groups) {
                if matches!(d.blending, Blending::Separable) {
                    // The shader reads what the earlier draws left, so
                    // the pass ends and the target is copied first.
                    drop(rpass);
                    let backdrop = &self.pool[backdrop.expect("acquired for the pass")];
                    encoder.copy_texture_to_texture(
                        target.as_image_copy(),
                        backdrop.texture.as_image_copy(),
                        wgpu::Extent3d {
                            width: pass.width,
                            height: pass.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    rpass = begin(&mut encoder, view, wgpu::LoadOp::Load);
                }
                rpass.set_pipeline(match d.blending {
                    Blending::Normal => &self.normal,
                    Blending::Add => &self.add,
                    Blending::Separable => &self.separable,
                    Blending::Veil(_) => &self.veil,
                });
                if let Blending::Veil(a) = d.blending {
                    let a = f64::from(a);
                    rpass.set_blend_constant(wgpu::Color {
                        r: a,
                        g: a,
                        b: a,
                        a,
                    });
                }
                let offset = (next_slot * self.slot) as wgpu::DynamicOffset;
                next_slot += 1;
                rpass.set_bind_group(0, bind_group, &[offset]);
                rpass.draw(0..6, 0..1);
            }
            drop(rpass);
        }
        let target = self.target.as_ref().expect("made for the frame");
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &target.staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.padded_row),
                    rows_per_image: Some(target.height),
                },
            },
            wgpu::Extent3d {
                width: target.width,
                height: target.height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue().submit([encoder.finish()]);
        self.passes = passes;
        self.passes.clear();
    }

    /// Waits for the frame and copies it out of the staging buffer.
    fn read_back(&self, gpu: &Gpu, frame: &mut Frame) {
        let target = self.target.as_ref().expect("drawn");
        let slice = target.staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the device answers");
        rx.recv()
            .expect("the map callback runs")
            .expect("the staging buffer maps");
        {
            let data = slice.get_mapped_range().expect("mapped above");
            let row_bytes = target.width as usize * TEXEL_BYTES as usize;
            let pixels = frame.pixels_mut();
            for (y, row) in data
                .chunks_exact(target.padded_row as usize)
                .take(target.height as usize)
                .enumerate()
            {
                let out = &mut pixels[y * target.width as usize..][..target.width as usize];
                for (p, texel) in out.iter_mut().zip(row[..row_bytes].chunks_exact(8)) {
                    let half = |i: usize| {
                        f16::from_bits(u16::from_le_bytes([texel[i], texel[i + 1]])).to_f32()
                    };
                    *p = LinearRgba {
                        r: half(0),
                        g: half(2),
                        b: half(4),
                        a: half(6),
                    };
                }
            }
        }
        target.staging.unmap();
    }
}

/// Begins a pass onto `view`, loading what is there or clearing it.
fn begin<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("geneva composite"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Why a visible clip cannot be drawn yet, if it cannot.
fn unsupported(clip: &ResolvedClip) -> Option<&'static str> {
    if !clip.effects.is_empty() {
        return Some("the blur effect");
    }
    match clip.source {
        ResolvedSource::Solid { .. }
        | ResolvedSource::Shape { .. }
        | ResolvedSource::Image { .. }
        | ResolvedSource::Composition(_) => None,
        ResolvedSource::Video { .. } => Some("a video source"),
        ResolvedSource::Html(_) => Some("a markup source"),
        ResolvedSource::Text(_) => Some("a text source"),
    }
}

/// How a blend mode is drawn, and its code for the shader.
fn blending(mode: BlendMode) -> (Blending, u32) {
    match mode {
        BlendMode::Normal => (Blending::Normal, 0),
        BlendMode::Multiply => (Blending::Separable, 1),
        BlendMode::Screen => (Blending::Separable, 2),
        BlendMode::Overlay => (Blending::Separable, 3),
        BlendMode::Darken => (Blending::Separable, 4),
        BlendMode::Lighten => (Blending::Separable, 5),
        BlendMode::Difference => (Blending::Separable, 6),
        BlendMode::SoftLight => (Blending::Separable, 7),
        BlendMode::Add => (Blending::Add, 8),
    }
}

fn channels(c: LinearRgba) -> [f32; 4] {
    [c.r, c.g, c.b, c.a]
}

fn uniform_buffer(device: &wgpu::Device, slot: u64, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("geneva clips"),
        size: slot * capacity as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_target(device: &wgpu::Device, width: u32, height: u32) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("geneva frame"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: WORKING_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_row = (width * TEXEL_BYTES).div_ceil(align) * align;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("geneva readback"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Target {
        texture,
        view,
        staging,
        width,
        height,
        padded_row,
    }
}

/// Uploads a decoded image as a working-format texture, its f32 channels
/// rounded to halves.
fn upload_image(gpu: &Gpu, id: &str, img: &Image) -> Cached {
    let device = gpu.device();
    let (width, height) = (img.width.max(1), img.height.max(1));
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(id),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: WORKING_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    if img.width > 0 && img.height > 0 {
        let mut bytes = Vec::with_capacity(img.pixels.len() * TEXEL_BYTES as usize);
        for p in &img.pixels {
            for c in [p.r, p.g, p.b, p.a] {
                bytes.extend_from_slice(&f16::from_f32(c).to_bits().to_le_bytes());
            }
        }
        gpu.queue().write_texture(
            texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(img.width * TEXEL_BYTES),
                rows_per_image: Some(img.height),
            },
            wgpu::Extent3d {
                width: img.width,
                height: img.height,
                depth_or_array_layers: 1,
            },
        );
    }
    Cached {
        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
        width: img.width,
        height: img.height,
        content: img.content,
    }
}
