//! Drawing a frame: the pipelines, the per-clip uniforms, the image
//! textures, the target and its readback.
//!
//! What draws here is what `composite.wgsl` transcribes from the CPU
//! renderer: solids, shapes and still images, placed through the same
//! [`Placement`] the reference uses, at the clip's opacity, blended
//! normally or additively. Everything else in the imaging model (masks,
//! the separable blend modes, transitions, nested compositions, blur,
//! text, markup and video) is refused with [`RenderError::Unsupported`]
//! until its step of the plan.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use geneva_color::{Color, LinearRgba};
use geneva_render::{AssetSource, Frame, Image, Placement, RenderError, crop_window};
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
    position: [f32; 2],
    anchor: [f32; 2],
    scale: [f32; 2],
    rotation: [f32; 2],
    size: [f32; 2],
    frame: [f32; 2],
    stroke_width: f32,
    radius: f32,
    opacity: f32,
    kind: u32,
    mode: u32,
    has_stroke: u32,
    _pad: [u32; 2],
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

/// Bytes per pixel of the working format.
const TEXEL_BYTES: u32 = 8;

/// One clip to draw, in order.
struct Draw {
    uniform: ClipUniform,
    blend: BlendMode,
    /// The image texture, by asset id; none for a solid or a shape.
    image: Option<String>,
}

/// An image asset on the device.
struct Cached {
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    content: Option<[u32; 4]>,
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
    layout: wgpu::BindGroupLayout,
    /// One slot per clip, `slot` bytes apart, bound at a dynamic offset.
    uniforms: wgpu::Buffer,
    slot: u64,
    capacity: usize,
    /// A 1x1 transparent texture bound where a draw has no image.
    blank: wgpu::TextureView,
    target: Option<Target>,
    images: HashMap<String, Cached>,
    /// Bind groups by image id (the empty id for the blank texture),
    /// dropped when the uniform buffer is replaced.
    bind_groups: HashMap<String, wgpu::BindGroup>,
}

impl Compositor {
    pub(crate) fn new(gpu: &Gpu) -> Self {
        let device = gpu.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geneva composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });
        let uniform_size = std::mem::size_of::<ClipUniform>() as u64;
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
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
            layout,
            uniforms,
            slot,
            capacity,
            blank,
            target: None,
            images: HashMap::new(),
            bind_groups: HashMap::new(),
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
        let draws = self.plan(gpu, assets, comp, &comp.layers, width, height, t)?;
        self.draw(gpu, width, height, comp.background, &draws);
        self.read_back(gpu, frame);
        Ok(())
    }

    /// Decides what each visible clip draws, uploading images on the way.
    #[allow(clippy::too_many_arguments)]
    fn plan<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        assets: &mut A,
        comp: &Composition,
        layers: &[ResolvedLayer],
        width: u32,
        height: u32,
        t: Ratio,
    ) -> Result<Vec<Draw>, RenderError> {
        let mut draws = Vec::new();
        let frame_size = [width as f32, height as f32];
        for layer in layers {
            for (i, clip) in layer.clips.iter().enumerate() {
                if !(clip.start <= t && t < clip.end) {
                    continue;
                }
                let local = (t - clip.start).to_f64();
                let opacity = clip.opacity.sample(local).clamp(0.0, 1.0);
                if opacity <= 0.0 {
                    continue;
                }
                if let Some(what) = unsupported(layer, i) {
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
                let mut image = None;
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
                        let cached = self.upload(gpu, assets, comp, asset)?;
                        image = Some(asset.clone());
                        (
                            (f64::from(cached.width), f64::from(cached.height)),
                            cached.content,
                        )
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
                    && place.scale[0].abs() >= 1.0
                    && place.scale[1].abs() >= 1.0
                {
                    let interior = place.interior(size.0 as u32, size.1 as u32);
                    uniform.interior = interior.map(|v| v as f32);
                    MODE_SPANS
                } else {
                    MODE_SUBSAMPLES
                };
                draws.push(Draw {
                    uniform,
                    blend: clip.blend,
                    image,
                });
            }
        }
        Ok(draws)
    }

    /// The image asset `id` on the device, uploaded on first use.
    fn upload<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        assets: &mut A,
        comp: &Composition,
        id: &str,
    ) -> Result<&Cached, RenderError> {
        if !self.images.contains_key(id) {
            let img = assets.image(comp, id)?;
            let cached = upload_image(gpu, id, img);
            self.images.insert(id.to_owned(), cached);
        }
        Ok(&self.images[id])
    }

    /// Records and submits the frame.
    fn draw(&mut self, gpu: &Gpu, width: u32, height: u32, background: Color, draws: &[Draw]) {
        let device = gpu.device();
        if self
            .target
            .as_ref()
            .is_none_or(|t| t.width != width || t.height != height)
        {
            self.target = Some(make_target(device, width, height));
        }
        if draws.len() > self.capacity {
            let mut capacity = self.capacity;
            while capacity < draws.len() {
                capacity *= 2;
            }
            self.uniforms = uniform_buffer(device, self.slot, capacity);
            self.capacity = capacity;
            self.bind_groups.clear();
        }
        // Every clip's uniforms go up in one write, each at its slot.
        let mut bytes = vec![0u8; self.slot as usize * draws.len().max(1)];
        for (i, d) in draws.iter().enumerate() {
            let at = i * self.slot as usize;
            bytes[at..at + std::mem::size_of::<ClipUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&d.uniform));
        }
        gpu.queue().write_buffer(&self.uniforms, 0, &bytes);
        for d in draws {
            let key = d.image.clone().unwrap_or_default();
            if !self.bind_groups.contains_key(&key) {
                let view = match &d.image {
                    Some(id) => &self.images[id].view,
                    None => &self.blank,
                };
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                    ],
                });
                self.bind_groups.insert(key, bind_group);
            }
        }
        let target = self.target.as_ref().expect("made above");
        let clear = background.to_linear();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("geneva frame"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("geneva composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear.r),
                            g: f64::from(clear.g),
                            b: f64::from(clear.b),
                            a: f64::from(clear.a),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for (i, d) in draws.iter().enumerate() {
                pass.set_pipeline(match d.blend {
                    BlendMode::Add => &self.add,
                    _ => &self.normal,
                });
                let key = d.image.clone().unwrap_or_default();
                let offset = (i as u64 * self.slot) as wgpu::DynamicOffset;
                pass.set_bind_group(0, &self.bind_groups[&key], &[offset]);
                pass.draw(0..6, 0..1);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &target.staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(target.padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue().submit([encoder.finish()]);
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

/// Why a visible clip cannot be drawn yet, if it cannot.
fn unsupported(layer: &ResolvedLayer, i: usize) -> Option<&'static str> {
    let clip: &ResolvedClip = &layer.clips[i];
    if clip.mask.is_some() {
        return Some("a mask");
    }
    if !clip.effects.is_empty() {
        return Some("the blur effect");
    }
    let mode = match clip.blend {
        BlendMode::Normal | BlendMode::Add => None,
        BlendMode::Multiply => Some("the multiply blend mode"),
        BlendMode::Screen => Some("the screen blend mode"),
        BlendMode::Overlay => Some("the overlay blend mode"),
        BlendMode::Darken => Some("the darken blend mode"),
        BlendMode::Lighten => Some("the lighten blend mode"),
        BlendMode::Difference => Some("the difference blend mode"),
        BlendMode::SoftLight => Some("the soft-light blend mode"),
    };
    if mode.is_some() {
        return mode;
    }
    // The clip after this one takes it out through its own transition.
    let next_in = layer
        .clips
        .get(i + 1)
        .is_some_and(|next| next.transition_in.is_some());
    if clip.transition_in.is_some() || clip.transition_out.is_some() || next_in {
        return Some("a transition");
    }
    match clip.source {
        ResolvedSource::Solid { .. }
        | ResolvedSource::Shape { .. }
        | ResolvedSource::Image { .. } => None,
        ResolvedSource::Composition(_) => Some("a nested composition"),
        ResolvedSource::Video { .. } => Some("a video source"),
        ResolvedSource::Html(_) => Some("a markup source"),
        ResolvedSource::Text(_) => Some("a text source"),
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
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
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
