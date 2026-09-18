//! Drawing a frame: the pipelines, the per-clip uniforms, the textures
//! (pictures the painter keeps, pictures made for one frame, a video
//! frame's planes, nested compositions, the backdrop copy a separable
//! blend reads), the target and its readback.
//!
//! What draws here is what `composite.wgsl` and `blur.wgsl` transcribe
//! from the CPU renderer and the media crate: every source. Solids
//! and shapes are described to the shader; image assets, text and
//! markup are painted by the shared [`Painter`] and uploaded as
//! textures, kept under the painter's key while a byte budget allows;
//! an 8-bit 4:2:0 video frame is uploaded as its three planes and
//! converted to linear light as its texels are read, any other video
//! frame as the picture the decoder converts; a nested composition is
//! drawn into a pooled texture first. Clips are placed through the same
//! [`Placement`] the reference uses, through shape and luma masks, at
//! the clip's opacity with its transitions, blended by any of the
//! format's modes, with a fade's veil over the result. A blurred clip is
//! drawn onto a transparent layer sized and downscaled as the CPU sizes
//! it, blurred by the same three box blurs as row and column passes
//! (`blur.wgsl`), and laid back with one bilinear sample per pixel.
//!
//! A frame is a list of passes, nested compositions and blur layers
//! first: each composition renders its layers into a texture (a pooled
//! one for a nested composition, the frame's own for the top). Normal
//! and add blending are fixed-function; a separable mode ends the pass,
//! copies the target to a backdrop texture, and draws with a pipeline
//! that replaces the pixel with what the shader composites from the
//! copy.
//!
//! The frame leaves the device through a ring of three staging buffers:
//! [`Compositor::submit`] records the frame, the pack of it into each
//! output's planes (`pack.wgsl`, one pass per plane into an integer
//! texture) and the copies into one slot's buffer, and returns a
//! [`Job`]; [`Compositor::finish`] waits for that submission alone, maps
//! the buffer and copies the rows out. With a frame in flight while the
//! next is planned, the painting of one overlaps the drawing of the
//! other.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use geneva_color::{Color, LinearRgba, Primaries, ResolvedTags, Transfer, matrix, primaries};
use geneva_media::PlaneTarget;
use geneva_media::convert::{PlaneFormat, encode_table, hdr_peak};
use geneva_render::{
    AssetSource, Frame, Image, Paint, Painter, Placement, RenderError, VideoPlanes, box_radii,
    crop_window, fade_veil, transition_gain,
};
use geneva_timeline::schema::{BlendMode, ShapeKind};
use geneva_timeline::{Composition, Ratio, ResolvedEffect, ResolvedLayer, ResolvedSource};
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
    yuv: [f32; 4],
    coef: [f32; 4],
    m0: [f32; 4],
    m1: [f32; 4],
    m2: [f32; 4],
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
    source: u32,
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
/// axis-aligned, magnified picture, which the CPU renderer resamples
/// span by span with one bilinear sample at the pixel center.
const MODE_SPANS: u32 = 2;

const MASK_NONE: u32 = 0;
const MASK_RECT: u32 = 1;
const MASK_ELLIPSE: u32 = 2;
const MASK_LUMA: u32 = 3;

/// A picture's texels are converted from 8-bit 4:2:0 planes rather
/// than read from the `image` texture (source 0).
const SOURCE_PLANES: u32 = 1;

/// Bytes per pixel of the working format.
const TEXEL_BYTES: u32 = 8;

/// What the kept pictures may take on the device, after which the least
/// recently drawn go first: the markup group cache's rule.
const CACHE_BUDGET: u64 = 96 << 20;

/// Entries of a transfer function's table, one per 16-bit code.
const LUT_SIZE: usize = 1 << 16;

/// A working-format texture a draw reads.
#[derive(Clone)]
enum Tex {
    /// The 1x1 transparent texture, where the draw has none.
    Blank,
    /// A picture kept under the painter's key.
    Cached(u64),
    /// A pooled texture of this frame: a picture made for it, a nested
    /// composition's frame, or a pass's backdrop copy.
    Pooled(usize),
    /// A blur layer of this frame, at f32.
    Layer(usize),
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
    /// A video frame's planes in the plane pool: Y, Cb, Cr.
    planes: Option<[usize; 3]>,
    /// The transfer function whose table the shader reads.
    transfer: Transfer,
}

/// One pass of the frame.
enum Pass {
    /// The layers of one composition drawn into one texture.
    Composite {
        /// The frame's own target, or a pooled texture.
        target: Option<usize>,
        width: u32,
        height: u32,
        background: Color,
        draws: Vec<Draw>,
    },
    /// A clip drawn onto a transparent blur layer.
    Layer { target: usize, draw: Box<Draw> },
    /// One box blur of a layer into another, along the rows or down the
    /// columns.
    Blur {
        from: usize,
        to: usize,
        radius: u32,
        vertical: bool,
    },
}

/// What one box blur pass tells `blur.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BoxUniform {
    radius: i32,
    vertical: u32,
    _pad: [u32; 2],
}

/// The format of blur layers: the CPU blurs in f32 and so does this,
/// rather than rounding to halves between six passes.
const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// A picture kept on the device.
struct Kept {
    view: wgpu::TextureView,
    bytes: u64,
    /// The frame it was last drawn in.
    used: u64,
}

/// A texture the frame draws into or copies to, kept for the next
/// frame that needs one of its size.
struct Pooled {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    /// Taken by this frame.
    busy: bool,
}

/// What one pack pass tells `pack.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct PackUniform {
    yuv: [f32; 4],
    coef: [f32; 4],
    m0: [f32; 4],
    m1: [f32; 4],
    m2: [f32; 4],
    max: f32,
    hdr_peak: f32,
    hdr: u32,
    mode: u32,
    block: [u32; 2],
    size: [u32; 2],
}

/// Where one plane, or the frame, lands in a slot's buffer.
#[derive(Debug, Clone, Copy)]
struct Region {
    offset: u64,
    /// Bytes per row in the buffer, a multiple of 256.
    padded_row: u32,
    /// Texels per row.
    width: u32,
    height: u32,
    bytes_per_texel: u32,
}

/// One slot of the staging ring.
struct Slot {
    buffer: Option<wgpu::Buffer>,
    size: u64,
    /// Submitted and not yet finished.
    busy: bool,
}

/// How many frames may be in flight: the one being finished, the one
/// prepared behind it, and one more for a frame asked for whole.
const RING: usize = 3;

/// A frame submitted to the device and not yet read back.
pub(crate) struct Job {
    slot: usize,
    submission: wgpu::SubmissionIndex,
    width: u32,
    height: u32,
    background: Color,
    /// The frame whole, when it was asked for.
    frame: Option<Region>,
    /// The planes of each output, in the outputs' order.
    planes: Vec<Vec<Region>>,
}

/// The frame texture.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

/// Everything a frame is drawn with, kept between frames.
pub(crate) struct Compositor {
    normal: wgpu::RenderPipeline,
    add: wgpu::RenderPipeline,
    separable: wgpu::RenderPipeline,
    veil: wgpu::RenderPipeline,
    /// The composite onto a transparent blur layer, at f32.
    layer: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    blur_layout: wgpu::BindGroupLayout,
    /// One slot per draw, `slot` bytes apart, bound at a dynamic offset.
    uniforms: wgpu::Buffer,
    slot: u64,
    capacity: usize,
    /// One slot per blur pass.
    blur_uniforms: wgpu::Buffer,
    blur_slot: u64,
    blur_capacity: usize,
    /// The pack into 8-bit planes, 16-bit planes, and RGBA bytes.
    pack_r8: wgpu::RenderPipeline,
    pack_r16: wgpu::RenderPipeline,
    pack_rgba8: wgpu::RenderPipeline,
    pack_layout: wgpu::BindGroupLayout,
    /// One slot per pack pass.
    pack_uniforms: wgpu::Buffer,
    pack_slot: u64,
    pack_capacity: usize,
    /// The encode tables uploaded so far, by transfer and HDR.
    encode_tables: HashMap<(Transfer, bool), wgpu::Buffer>,
    /// Plane output textures by format and size.
    outputs: Vec<Pooled>,
    /// The staging ring.
    slots: Vec<Slot>,
    /// A 1x1 transparent texture bound where a draw has no picture.
    blank: wgpu::TextureView,
    /// A 1x1 texture of the planes' format, bound where a draw has none.
    blank_plane: wgpu::TextureView,
    target: Option<Target>,
    /// Pictures kept under the painter's keys, within the budget.
    kept: HashMap<u64, Kept>,
    kept_bytes: u64,
    /// The frame being drawn, for the least-recently-used order.
    tick: u64,
    /// Working-format textures by size.
    pool: Vec<Pooled>,
    /// Plane textures by size.
    planes: Vec<Pooled>,
    /// Blur layers by size, at f32.
    layers: Vec<Pooled>,
    /// The transfer tables uploaded so far.
    luts: HashMap<Transfer, wgpu::Buffer>,
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
        let texture_entry =
            |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            };
        let float = wgpu::TextureSampleType::Float { filterable: false };
        let uint = wgpu::TextureSampleType::Uint;
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
                texture_entry(1, float),
                texture_entry(2, float),
                texture_entry(3, float),
                texture_entry(4, uint),
                texture_entry(5, uint),
                texture_entry(6, uint),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new((LUT_SIZE * 4) as u64),
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
        let pipeline =
            |label: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
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
                            format,
                            blend,
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
            WORKING_FORMAT,
            Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        );
        // Add: the colors sum; the alpha is sa + da - sa * da, which is
        // "over" on the alpha channel.
        let add = pipeline(
            "geneva composite add",
            WORKING_FORMAT,
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
        );
        // The shader composites from the backdrop copy and its result
        // replaces the pixel.
        let separable = pipeline(
            "geneva composite separable",
            WORKING_FORMAT,
            Some(wgpu::BlendState::REPLACE),
        );
        // dst * (1 - a) + fill * a on every channel, `a` the blend
        // constant: Frame::veil.
        let constant = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let veil = pipeline(
            "geneva composite veil",
            WORKING_FORMAT,
            Some(wgpu::BlendState {
                color: constant,
                alpha: constant,
            }),
        );
        // Onto a transparent layer, writing the sample as it is, which is
        // "over" onto transparency; f32 targets cannot blend without a
        // feature and need not here.
        let layer = pipeline("geneva composite layer", LAYER_FORMAT, None);
        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geneva blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()),
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geneva blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<BoxUniform>() as u64
                        ),
                    },
                    count: None,
                },
                texture_entry(1, float),
            ],
        });
        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geneva blur"),
            bind_group_layouts: &[Some(&blur_layout)],
            immediate_size: 0,
        });
        let blur = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("geneva blur"),
            layout: Some(&blur_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blur_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blur_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: LAYER_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let pack_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geneva pack"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pack.wgsl").into()),
        });
        let pack_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geneva pack"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<PackUniform>() as u64
                        ),
                    },
                    count: None,
                },
                texture_entry(1, float),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new((LUT_SIZE * 4) as u64),
                    },
                    count: None,
                },
            ],
        });
        let pack_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geneva pack"),
            bind_group_layouts: &[Some(&pack_layout)],
            immediate_size: 0,
        });
        let pack = |label: &str, entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pack_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &pack_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &pack_shader,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pack_r8 = pack("geneva pack 8-bit", "fs_plane", wgpu::TextureFormat::R8Uint);
        let pack_r16 = pack(
            "geneva pack 16-bit",
            "fs_plane",
            wgpu::TextureFormat::R16Uint,
        );
        let pack_rgba8 = pack(
            "geneva pack rgba",
            "fs_rgba",
            wgpu::TextureFormat::Rgba8Uint,
        );
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let slot = uniform_size.div_ceil(align) * align;
        let capacity = 64;
        let uniforms = uniform_buffer(device, slot, capacity);
        let blur_slot = (std::mem::size_of::<BoxUniform>() as u64).div_ceil(align) * align;
        let blur_capacity = 16;
        let blur_uniforms = uniform_buffer(device, blur_slot, blur_capacity);
        let pack_slot = (std::mem::size_of::<PackUniform>() as u64).div_ceil(align) * align;
        let pack_capacity = 8;
        let pack_uniforms = uniform_buffer(device, pack_slot, pack_capacity);
        let blank = make_texture(device, "geneva blank", WORKING_FORMAT, 1, 1, false)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let blank_plane = make_texture(
            device,
            "geneva blank plane",
            wgpu::TextureFormat::R8Uint,
            1,
            1,
            false,
        )
        .create_view(&wgpu::TextureViewDescriptor::default());
        let mut this = Self {
            normal,
            add,
            separable,
            veil,
            layer,
            blur,
            layout,
            blur_layout,
            uniforms,
            slot,
            capacity,
            blur_uniforms,
            blur_slot,
            blur_capacity,
            pack_r8,
            pack_r16,
            pack_rgba8,
            pack_layout,
            pack_uniforms,
            pack_slot,
            pack_capacity,
            encode_tables: HashMap::new(),
            outputs: Vec::new(),
            slots: (0..RING)
                .map(|_| Slot {
                    buffer: None,
                    size: 0,
                    busy: false,
                })
                .collect(),
            blank,
            blank_plane,
            target: None,
            kept: HashMap::new(),
            kept_bytes: 0,
            tick: 0,
            pool: Vec::new(),
            planes: Vec::new(),
            layers: Vec::new(),
            luts: HashMap::new(),
            passes: Vec::new(),
        };
        // The table bound where a draw reads none.
        this.lut(gpu, Transfer::Srgb);
        this
    }

    /// Draws the frame of `comp` at `t` into `frame`, painting sources
    /// with `painter`: a submit and a finish, one after the other.
    pub(crate) fn render<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        painter: &mut Painter<A>,
        comp: &Composition,
        t: Ratio,
        frame: &mut Frame,
    ) -> Result<(), RenderError> {
        let job = self.submit(gpu, painter, comp, t, &[], true)?;
        self.finish(gpu, job, &mut [], Some(frame));
        Ok(())
    }

    /// Records the frame of `comp` at `t`, its pack into each output's
    /// planes, and the copies into a free slot of the ring, and submits
    /// them; with `with_frame`, the frame whole as well. A slot must be
    /// free: at most [`RING`] jobs are in flight.
    pub(crate) fn submit<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        painter: &mut Painter<A>,
        comp: &Composition,
        t: Ratio,
        outputs: &[(ResolvedTags, PlaneFormat)],
        with_frame: bool,
    ) -> Result<Job, RenderError> {
        let (width, height) = (comp.width, comp.height);
        let device = gpu.device();
        if self
            .target
            .as_ref()
            .is_none_or(|t| t.width != width || t.height != height)
        {
            self.target = Some(make_target(device, width.max(1), height.max(1)));
        }
        self.tick += 1;
        for p in self
            .pool
            .iter_mut()
            .chain(self.planes.iter_mut())
            .chain(self.layers.iter_mut())
            .chain(self.outputs.iter_mut())
        {
            p.busy = false;
        }
        self.passes.clear();
        if width > 0 && height > 0 {
            let planned = self.plan(
                gpu,
                painter,
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
        }
        self.evict();
        // Where everything lands in the slot's buffer.
        let mut size = 0u64;
        let mut region = |texels: u32, rows: u32, bytes_per_texel: u32| {
            let align = u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let padded_row =
                (u64::from(texels) * u64::from(bytes_per_texel)).div_ceil(align) * align;
            let r = Region {
                offset: size,
                padded_row: padded_row as u32,
                width: texels,
                height: rows,
                bytes_per_texel,
            };
            size += padded_row * u64::from(rows);
            r
        };
        let frame = with_frame.then(|| region(width, height, TEXEL_BYTES));
        let planes: Vec<Vec<Region>> = outputs
            .iter()
            .map(|(_, format)| {
                (0..format.plane_count())
                    .map(|k| {
                        let (w, h) = format.plane_size(k, width, height);
                        if format.is_rgb() {
                            region((w / 4) as u32, h as u32, 4)
                        } else {
                            region(w as u32, h as u32, format.bytes_per_sample() as u32)
                        }
                    })
                    .collect()
            })
            .collect();
        let slot = self
            .slots
            .iter()
            .position(|s| !s.busy)
            .expect("a slot of the ring is free");
        if self.slots[slot].buffer.is_none() || self.slots[slot].size < size {
            self.slots[slot].buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("geneva readback"),
                size: size.max(wgpu::MAP_ALIGNMENT),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.slots[slot].size = size.max(wgpu::MAP_ALIGNMENT);
        }
        self.slots[slot].busy = true;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("geneva frame"),
        });
        if width > 0 && height > 0 {
            self.execute(gpu, &mut encoder);
        }
        // Handles of their own, so the copies borrow nothing of `self`
        // while the pack passes take it mutably.
        let target = self.target.as_ref().expect("made above").texture.clone();
        let buffer = self.slots[slot].buffer.clone().expect("made above");
        let copy = |encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, r: &Region| {
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: r.offset,
                        bytes_per_row: Some(r.padded_row),
                        rows_per_image: Some(r.height),
                    },
                },
                wgpu::Extent3d {
                    width: r.width,
                    height: r.height,
                    depth_or_array_layers: 1,
                },
            );
        };
        if let Some(r) = &frame {
            if width > 0 && height > 0 {
                copy(&mut encoder, &target, r);
            }
        }
        if width > 0 && height > 0 && !outputs.is_empty() {
            self.pack(gpu, &mut encoder, outputs, &planes, &copy);
        }
        let submission = gpu.queue().submit([encoder.finish()]);
        Ok(Job {
            slot,
            submission,
            width,
            height,
            background: comp.background,
            frame,
            planes,
        })
    }

    /// Records the pack passes of the frame into each output's planes,
    /// and their copies into the slot's buffer.
    fn pack(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        outputs: &[(ResolvedTags, PlaneFormat)],
        regions: &[Vec<Region>],
        copy: &dyn Fn(&mut wgpu::CommandEncoder, &wgpu::Texture, &Region),
    ) {
        let device = gpu.device();
        let target = self.target.as_ref().expect("made for the frame");
        let (width, height) = (target.width, target.height);
        // The uniforms of every pack pass go up in one write, and every
        // table they read exists before the bind groups are made.
        let mut uniforms = Vec::new();
        for (tags, format) in outputs {
            for k in 0..format.plane_count() {
                uniforms.push(pack_uniform(*tags, *format, k, width, height));
            }
            self.encode_table(gpu, tags.transfer, tags.is_hdr());
        }
        if uniforms.len() > self.pack_capacity {
            let mut capacity = self.pack_capacity;
            while capacity < uniforms.len() {
                capacity *= 2;
            }
            self.pack_uniforms = uniform_buffer(device, self.pack_slot, capacity);
            self.pack_capacity = capacity;
        }
        let mut bytes = vec![0u8; self.pack_slot as usize * uniforms.len()];
        for (i, u) in uniforms.iter().enumerate() {
            let at = i * self.pack_slot as usize;
            bytes[at..at + std::mem::size_of::<PackUniform>()]
                .copy_from_slice(bytemuck::bytes_of(u));
        }
        gpu.queue().write_buffer(&self.pack_uniforms, 0, &bytes);
        // The plane textures, taken before any borrow of the pools.
        let mut textures: Vec<Vec<usize>> = Vec::new();
        for ((_, format), planes) in outputs.iter().zip(regions) {
            let texture_format = if format.is_rgb() {
                wgpu::TextureFormat::Rgba8Uint
            } else if format.bytes_per_sample() == 2 {
                wgpu::TextureFormat::R16Uint
            } else {
                wgpu::TextureFormat::R8Uint
            };
            textures.push(
                planes
                    .iter()
                    .map(|r| {
                        acquire_in(
                            &mut self.outputs,
                            gpu,
                            texture_format,
                            r.width,
                            r.height,
                            true,
                        )
                    })
                    .collect(),
            );
        }
        let target = self.target.as_ref().expect("made for the frame");
        let mut pass_index = 0u64;
        for ((tags, format), (planes, textures)) in
            outputs.iter().zip(regions.iter().zip(&textures))
        {
            let table = &self.encode_tables[&(tags.transfer, tags.is_hdr())];
            let pipeline = if format.is_rgb() {
                &self.pack_rgba8
            } else if format.bytes_per_sample() == 2 {
                &self.pack_r16
            } else {
                &self.pack_r8
            };
            for (r, &texture) in planes.iter().zip(textures) {
                let out = &self.outputs[texture];
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("geneva pack"),
                    layout: &self.pack_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.pack_uniforms,
                                offset: 0,
                                size: wgpu::BufferSize::new(
                                    std::mem::size_of::<PackUniform>() as u64
                                ),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&target.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: table.as_entire_binding(),
                        },
                    ],
                });
                {
                    let mut rpass =
                        begin(encoder, &out.view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                    rpass.set_pipeline(pipeline);
                    let offset = (pass_index * self.pack_slot) as wgpu::DynamicOffset;
                    rpass.set_bind_group(0, &bind_group, &[offset]);
                    rpass.draw(0..3, 0..1);
                }
                pass_index += 1;
                copy(encoder, &out.texture, r);
            }
        }
    }

    /// The output's encode table on the device, uploaded on first use.
    fn encode_table(&mut self, gpu: &Gpu, transfer: Transfer, hdr: bool) -> &wgpu::Buffer {
        self.encode_tables
            .entry((transfer, hdr))
            .or_insert_with(|| {
                let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
                    label: Some("geneva encode"),
                    size: (LUT_SIZE * 4) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                gpu.queue().write_buffer(
                    &buffer,
                    0,
                    bytemuck::cast_slice(encode_table(transfer, hdr).as_slice()),
                );
                buffer
            })
    }

    /// Waits for `job`'s submission alone, and copies its planes into
    /// `targets` (in the outputs' order it was submitted with) and its
    /// frame into `frame`, when it carried one. The slot is free again
    /// after; the job is taken by value so that it is finished once.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn finish(
        &mut self,
        gpu: &Gpu,
        job: Job,
        targets: &mut [PlaneTarget<'_>],
        frame: Option<&mut Frame>,
    ) {
        let slot = &self.slots[job.slot];
        let buffer = slot.buffer.as_ref().expect("submitted with a buffer");
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        gpu.device()
            .poll(wgpu::PollType::Wait {
                submission_index: Some(job.submission.clone()),
                timeout: None,
            })
            .expect("the device answers");
        rx.recv()
            .expect("the map callback runs")
            .expect("the staging buffer maps");
        {
            let data = slice.get_mapped_range().expect("mapped above");
            let rows = |r: &Region| {
                let row_bytes = r.width as usize * r.bytes_per_texel as usize;
                data[r.offset as usize..]
                    .chunks_exact(r.padded_row as usize)
                    .take(r.height as usize)
                    .map(move |row| &row[..row_bytes])
            };
            if let Some(frame) = frame {
                frame.reset(job.width, job.height, job.background);
                if let Some(r) = &job.frame {
                    let pixels = frame.pixels_mut();
                    for (y, row) in rows(r).enumerate() {
                        let out = &mut pixels[y * r.width as usize..][..r.width as usize];
                        for (p, texel) in out.iter_mut().zip(row.chunks_exact(8)) {
                            let half = |i: usize| {
                                f16::from_bits(u16::from_le_bytes([texel[i], texel[i + 1]]))
                                    .to_f32()
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
            }
            for (regions, target) in job.planes.iter().zip(targets.iter_mut()) {
                for (r, plane) in regions.iter().zip(target.planes.planes.iter_mut()) {
                    for (y, row) in rows(r).enumerate() {
                        let at = y * plane.stride;
                        plane.data[at..at + row.len()].copy_from_slice(row);
                    }
                }
            }
        }
        buffer.unmap();
        self.slots[job.slot].busy = false;
    }

    /// Gives a job's slot up without reading it; the job is taken by
    /// value so that it is released once.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn release(&mut self, job: Job) {
        self.slots[job.slot].busy = false;
    }

    /// Plans the pass that draws `layers` into `target` (the frame's own
    /// when `None`), after the passes of the nested compositions it
    /// shows; pictures and planes are uploaded on the way.
    #[allow(clippy::too_many_arguments)]
    fn plan<A: AssetSource>(
        &mut self,
        gpu: &Gpu,
        painter: &mut Painter<A>,
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
                // The blurs on the clip, combined into one deviation.
                let sigma = clip
                    .effects
                    .iter()
                    .map(|e| match e {
                        ResolvedEffect::Blur(radius) => radius.sample(local).max(0.0),
                    })
                    .fold(0.0f64, |acc, s| (acc * acc + s * s).sqrt());
                let mut uniform = ClipUniform {
                    frame: frame_size,
                    opacity: opacity as f32,
                    ..ClipUniform::zeroed()
                };
                let mut image = Tex::Blank;
                let mut planes = None;
                let mut transfer = Transfer::Srgb;
                let mut picture: Option<(f64, f64, Option<[u32; 4]>)> = None;
                match &clip.source {
                    ResolvedSource::Composition(nested) => {
                        let texture = self.acquire(gpu, nested.width, nested.height);
                        self.plan(
                            gpu,
                            painter,
                            comp,
                            &nested.layers,
                            nested.width,
                            nested.height,
                            nested.background,
                            (t - clip.start) * clip.speed,
                            Some(texture),
                        )?;
                        image = Tex::Pooled(texture);
                        uniform.kind = KIND_IMAGE;
                        picture = Some((f64::from(nested.width), f64::from(nested.height), None));
                    }
                    ResolvedSource::Video { asset, in_, .. } => {
                        // The decoder's own planes when it holds them in
                        // the one layout the shader converts; its
                        // picture otherwise, through the painter.
                        let source_time = *in_ + (t - clip.start) * clip.speed;
                        let held = painter
                            .assets_mut()
                            .video_planes(comp, asset, source_time)?;
                        if let Some(p) = held {
                            let (w, h) = (p.width, p.height);
                            planes = Some(self.upload_planes(gpu, &p));
                            transfer = p.tags.transfer;
                            set_conversion(&mut uniform, &p);
                            uniform.kind = KIND_IMAGE;
                            uniform.source = SOURCE_PLANES;
                            picture = Some((f64::from(w), f64::from(h), None));
                        }
                    }
                    _ => {}
                }
                if picture.is_none() {
                    let painted = painter.paint(comp, clip, t, local)?;
                    match painted.paint {
                        Paint::Solid {
                            color,
                            width,
                            height,
                        } => {
                            uniform.kind = KIND_SOLID;
                            uniform.fill = channels(color);
                            picture = Some((width, height, None));
                        }
                        Paint::Shape {
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
                            uniform.fill = channels(fill);
                            if let Some((color, w)) = stroke {
                                uniform.stroke = channels(color);
                                uniform.stroke_width = w as f32;
                                uniform.has_stroke = 1;
                            }
                            uniform.radius = radius as f32;
                            picture = Some((width, height, None));
                        }
                        Paint::Image(img) => {
                            let (w, h, content) = (img.width, img.height, img.content);
                            image = match painted.key {
                                Some(key) => self.keep(gpu, key, &img),
                                None => self.upload_once(gpu, &img),
                            };
                            uniform.kind = KIND_IMAGE;
                            picture = Some((f64::from(w), f64::from(h), content));
                        }
                    }
                }
                let Some((w, h, content)) = picture else {
                    continue;
                };
                let size = (w, h);
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
                            let key = image_key(id);
                            let img = painter.assets_mut().image(comp, id)?;
                            mask = self.keep(gpu, key, img);
                            MASK_LUMA
                        }
                        None => match m.shape {
                            ShapeKind::Rect => MASK_RECT,
                            ShapeKind::Ellipse => MASK_ELLIPSE,
                        },
                    };
                }
                set_placement(&mut uniform, &place, size);
                let (blending, code) = blending(clip.blend);
                uniform.blend = code;
                let draw = Draw {
                    uniform,
                    blending,
                    image,
                    mask,
                    planes,
                    transfer,
                };
                if sigma > 0.0 {
                    self.plan_blur(gpu, &mut draws, draw, &place, sigma, width, height);
                } else {
                    draws.push(draw);
                }
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
                    planes: None,
                    transfer: Transfer::Srgb,
                });
            }
        }
        self.passes.push(Pass::Composite {
            target,
            width,
            height,
            background,
            draws,
        });
        Ok(())
    }

    /// Plans a blurred clip as the CPU renderer draws one: the clip goes
    /// onto a transparent layer covering everything its blur can reach
    /// in the frame, `k` times smaller for a wide blur, the layer is
    /// blurred by three box blurs each way, and the result is laid onto
    /// the frame at the clip's opacity and blend mode, one bilinear
    /// sample per pixel.
    #[allow(clippy::too_many_arguments)]
    fn plan_blur(
        &mut self,
        gpu: &Gpu,
        draws: &mut Vec<Draw>,
        draw: Draw,
        place: &Placement,
        sigma: f64,
        width: u32,
        height: u32,
    ) {
        let reach = (3.0 * sigma).ceil();
        let (fw, fh) = (f64::from(width), f64::from(height));
        let x0 = (place.extent[0] - reach).max(-reach).floor();
        let y0 = (place.extent[1] - reach).max(-reach).floor();
        let x1 = (place.extent[2] + reach).min(fw + reach).ceil();
        let y1 = (place.extent[3] + reach).min(fh + reach).ceil();
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let k = if sigma >= 4.0 {
            (sigma / 2.0).floor().min(8.0)
        } else {
            1.0
        };
        let lw = ((x1 - x0) / k).ceil().max(1.0) as u32;
        let lh = ((y1 - y0) / k).ceil().max(1.0) as u32;
        // Nothing of the clip lands on the layer: the blurred layer is
        // transparent and lays nothing.
        let Some(moved) = place.moved([-x0, -y0], 1.0 / k, lw, lh) else {
            return;
        };
        let size = (
            f64::from(draw.uniform.size[0]),
            f64::from(draw.uniform.size[1]),
        );
        let mut on_layer = draw.uniform;
        set_placement(&mut on_layer, &moved, size);
        on_layer.frame = [lw as f32, lh as f32];
        on_layer.opacity = 1.0;
        on_layer.blend = 0;
        let first = self.acquire_layer(gpu, lw, lh);
        self.passes.push(Pass::Layer {
            target: first,
            draw: Box::new(Draw {
                uniform: on_layer,
                blending: Blending::Normal,
                image: draw.image,
                mask: draw.mask,
                planes: draw.planes,
                transfer: draw.transfer,
            }),
        });
        let (mut from, mut to) = (first, self.acquire_layer(gpu, lw, lh));
        for radius in box_radii(sigma / k) {
            if radius == 0 {
                continue;
            }
            for vertical in [false, true] {
                self.passes.push(Pass::Blur {
                    from,
                    to,
                    radius: radius as u32,
                    vertical,
                });
                std::mem::swap(&mut from, &mut to);
            }
        }
        // The layer laid back (composite_layer): its pixel (0, 0) at the
        // layer's origin, each pixel `k` frame pixels wide, one sample at
        // each frame pixel's center.
        let bx0 = x0.max(0.0) as u32;
        let by0 = y0.max(0.0) as u32;
        let bx1 = ((x0 + f64::from(lw) * k).ceil().max(0.0) as u32).min(width);
        let by1 = ((y0 + f64::from(lh) * k).ceil().max(0.0) as u32).min(height);
        if bx0 >= bx1 || by0 >= by1 {
            return;
        }
        let laid = ClipUniform {
            bounds: [bx0 as f32, by0 as f32, bx1 as f32, by1 as f32],
            window: [0.0, 0.0, lw as f32, lh as f32],
            position: [x0 as f32, y0 as f32],
            anchor: [0.0, 0.0],
            scale: [k as f32, k as f32],
            rotation: [1.0, 0.0],
            size: [lw as f32, lh as f32],
            frame: draw.uniform.frame,
            opacity: draw.uniform.opacity,
            kind: KIND_IMAGE,
            mode: MODE_CENTER,
            blend: draw.uniform.blend,
            ..ClipUniform::zeroed()
        };
        draws.push(Draw {
            uniform: laid,
            blending: draw.blending,
            image: Tex::Layer(from),
            mask: Tex::Blank,
            planes: None,
            transfer: Transfer::Srgb,
        });
    }

    /// The picture under `key`, uploaded on first use and kept while
    /// the budget allows.
    fn keep(&mut self, gpu: &Gpu, key: u64, img: &Image) -> Tex {
        if let Some(k) = self.kept.get_mut(&key) {
            k.used = self.tick;
            return Tex::Cached(key);
        }
        let (width, height) = (img.width.max(1), img.height.max(1));
        let texture = make_texture(
            gpu.device(),
            "geneva picture",
            WORKING_FORMAT,
            width,
            height,
            false,
        );
        write_image(gpu, &texture, img);
        let bytes = u64::from(width) * u64::from(height) * u64::from(TEXEL_BYTES);
        self.kept.insert(
            key,
            Kept {
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                bytes,
                used: self.tick,
            },
        );
        self.kept_bytes += bytes;
        Tex::Cached(key)
    }

    /// Drops kept pictures not drawn this frame, least recently drawn
    /// first, until the rest fit the budget.
    fn evict(&mut self) {
        while self.kept_bytes > CACHE_BUDGET {
            let Some((&key, _)) = self
                .kept
                .iter()
                .filter(|(_, k)| k.used < self.tick)
                .min_by_key(|(_, k)| k.used)
            else {
                break;
            };
            if let Some(k) = self.kept.remove(&key) {
                self.kept_bytes -= k.bytes;
            }
        }
    }

    /// A picture made for this frame, in a pooled texture.
    fn upload_once(&mut self, gpu: &Gpu, img: &Image) -> Tex {
        let i = self.acquire(gpu, img.width, img.height);
        write_image(gpu, &self.pool[i].texture, img);
        Tex::Pooled(i)
    }

    /// A video frame's planes in pooled plane textures: Y, Cb, Cr.
    fn upload_planes(&mut self, gpu: &Gpu, p: &VideoPlanes<'_>) -> [usize; 3] {
        let (cw, ch) = (p.width.div_ceil(2), p.height.div_ceil(2));
        let y = self.acquire_plane(gpu, p.width, p.height);
        let cb = self.acquire_plane(gpu, cw, ch);
        let cr = self.acquire_plane(gpu, cw, ch);
        let write = |texture: &wgpu::Texture, data: &[u8], stride: usize, w: u32, h: u32| {
            gpu.queue().write_texture(
                texture.as_image_copy(),
                &data[..stride * (h as usize - 1) + w as usize],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride as u32),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        };
        write(&self.planes[y].texture, p.y, p.y_stride, p.width, p.height);
        write(&self.planes[cb].texture, p.cb, p.c_stride, cw, ch);
        write(&self.planes[cr].texture, p.cr, p.c_stride, cw, ch);
        [y, cb, cr]
    }

    /// The table of `transfer`, one entry per 16-bit code, uploaded on
    /// first use: the same table the CPU conversion reads.
    fn lut(&mut self, gpu: &Gpu, transfer: Transfer) -> &wgpu::Buffer {
        self.luts.entry(transfer).or_insert_with(|| {
            let values: Vec<f32> = (0..LUT_SIZE)
                .map(|i| transfer.to_linear(i as f64 / (LUT_SIZE - 1) as f64) as f32)
                .collect();
            let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("geneva transfer"),
                size: (LUT_SIZE * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.queue()
                .write_buffer(&buffer, 0, bytemuck::cast_slice(&values));
            buffer
        })
    }

    /// A pooled working-format texture of this size for the frame, made
    /// when none is free.
    fn acquire(&mut self, gpu: &Gpu, width: u32, height: u32) -> usize {
        acquire_in(&mut self.pool, gpu, WORKING_FORMAT, width, height, true)
    }

    /// A pooled blur layer of this size for the frame.
    fn acquire_layer(&mut self, gpu: &Gpu, width: u32, height: u32) -> usize {
        acquire_in(&mut self.layers, gpu, LAYER_FORMAT, width, height, true)
    }

    /// A pooled plane texture of this size for the frame.
    fn acquire_plane(&mut self, gpu: &Gpu, width: u32, height: u32) -> usize {
        acquire_in(
            &mut self.planes,
            gpu,
            wgpu::TextureFormat::R8Uint,
            width,
            height,
            false,
        )
    }

    fn view(&self, tex: &Tex) -> &wgpu::TextureView {
        match tex {
            Tex::Blank => &self.blank,
            Tex::Cached(key) => &self.kept[key].view,
            Tex::Pooled(i) => &self.pool[*i].view,
            Tex::Layer(i) => &self.layers[*i].view,
        }
    }

    /// The bindings of one composite draw.
    fn bind_group(
        &self,
        device: &wgpu::Device,
        d: &Draw,
        backdrop: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let plane = |k: usize| match d.planes {
            Some(p) => &self.planes[p[k]].view,
            None => &self.blank_plane,
        };
        let uniform = wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: &self.uniforms,
            offset: 0,
            size: wgpu::BufferSize::new(std::mem::size_of::<ClipUniform>() as u64),
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("geneva clip"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform,
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
                    resource: wgpu::BindingResource::TextureView(backdrop),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(plane(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(plane(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(plane(2)),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: self.luts[&d.transfer].as_entire_binding(),
                },
            ],
        })
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

    /// Records the planned passes into `encoder`.
    fn execute(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder) {
        let device = gpu.device();
        let passes = std::mem::take(&mut self.passes);
        // Every composite draw's uniforms go up in one write, each at
        // its slot, in the order the passes draw them; the blur passes'
        // in another.
        let draws: Vec<&Draw> = passes
            .iter()
            .flat_map(|p| match p {
                Pass::Composite { draws, .. } => draws.iter().collect::<Vec<_>>(),
                Pass::Layer { draw, .. } => vec![draw.as_ref()],
                Pass::Blur { .. } => Vec::new(),
            })
            .collect();
        if draws.len() > self.capacity {
            let mut capacity = self.capacity;
            while capacity < draws.len() {
                capacity *= 2;
            }
            self.uniforms = uniform_buffer(device, self.slot, capacity);
            self.capacity = capacity;
        }
        let mut bytes = vec![0u8; self.slot as usize * draws.len().max(1)];
        for (i, d) in draws.iter().enumerate() {
            let at = i * self.slot as usize;
            bytes[at..at + std::mem::size_of::<ClipUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&d.uniform));
        }
        gpu.queue().write_buffer(&self.uniforms, 0, &bytes);
        let boxes: Vec<BoxUniform> = passes
            .iter()
            .filter_map(|p| match p {
                Pass::Blur {
                    radius, vertical, ..
                } => Some(BoxUniform {
                    radius: *radius as i32,
                    vertical: u32::from(*vertical),
                    _pad: [0; 2],
                }),
                _ => None,
            })
            .collect();
        if boxes.len() > self.blur_capacity {
            let mut capacity = self.blur_capacity;
            while capacity < boxes.len() {
                capacity *= 2;
            }
            self.blur_uniforms = uniform_buffer(device, self.blur_slot, capacity);
            self.blur_capacity = capacity;
        }
        let mut bytes = vec![0u8; self.blur_slot as usize * boxes.len().max(1)];
        for (i, b) in boxes.iter().enumerate() {
            let at = i * self.blur_slot as usize;
            bytes[at..at + std::mem::size_of::<BoxUniform>()]
                .copy_from_slice(bytemuck::bytes_of(b));
        }
        gpu.queue().write_buffer(&self.blur_uniforms, 0, &bytes);
        // Every transfer table a draw reads exists before the bind groups
        // are made, and a pass with a separable blend reads a copy of its
        // target.
        for d in &draws {
            self.lut(gpu, d.transfer);
        }
        let backdrops: Vec<Option<usize>> = passes
            .iter()
            .map(|p| match p {
                Pass::Composite {
                    draws,
                    width,
                    height,
                    ..
                } => draws
                    .iter()
                    .any(|d| matches!(d.blending, Blending::Separable))
                    .then(|| self.acquire(gpu, *width, *height)),
                _ => None,
            })
            .collect();
        let mut next_slot = 0u64;
        let mut next_box = 0u64;
        let transparent = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
        for (pass, backdrop) in passes.iter().zip(&backdrops) {
            match pass {
                Pass::Composite {
                    target,
                    width,
                    height,
                    background,
                    draws,
                } => {
                    let (target, view) = self.texture_of(*target);
                    let backdrop_view = backdrop.map_or(&self.blank, |i| &self.pool[i].view);
                    let bind_groups: Vec<wgpu::BindGroup> = draws
                        .iter()
                        .map(|d| self.bind_group(device, d, backdrop_view))
                        .collect();
                    let clear = background.to_linear();
                    let mut rpass = begin(
                        encoder,
                        view,
                        wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear.r),
                            g: f64::from(clear.g),
                            b: f64::from(clear.b),
                            a: f64::from(clear.a),
                        }),
                    );
                    for (d, bind_group) in draws.iter().zip(&bind_groups) {
                        if matches!(d.blending, Blending::Separable) {
                            // The shader reads what the earlier draws
                            // left, so the pass ends and the target is
                            // copied first.
                            drop(rpass);
                            let backdrop = &self.pool[backdrop.expect("acquired for the pass")];
                            encoder.copy_texture_to_texture(
                                target.as_image_copy(),
                                backdrop.texture.as_image_copy(),
                                wgpu::Extent3d {
                                    width: *width,
                                    height: *height,
                                    depth_or_array_layers: 1,
                                },
                            );
                            rpass = begin(encoder, view, wgpu::LoadOp::Load);
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
                Pass::Layer { target, draw } => {
                    let bind_group = self.bind_group(device, draw, &self.blank);
                    let mut rpass = begin(encoder, &self.layers[*target].view, transparent);
                    rpass.set_pipeline(&self.layer);
                    let offset = (next_slot * self.slot) as wgpu::DynamicOffset;
                    next_slot += 1;
                    rpass.set_bind_group(0, &bind_group, &[offset]);
                    rpass.draw(0..6, 0..1);
                }
                Pass::Blur { from, to, .. } => {
                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("geneva blur"),
                        layout: &self.blur_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &self.blur_uniforms,
                                    offset: 0,
                                    size: wgpu::BufferSize::new(
                                        std::mem::size_of::<BoxUniform>() as u64
                                    ),
                                }),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(
                                    &self.layers[*from].view,
                                ),
                            },
                        ],
                    });
                    let mut rpass = begin(encoder, &self.layers[*to].view, transparent);
                    rpass.set_pipeline(&self.blur);
                    let offset = (next_box * self.blur_slot) as wgpu::DynamicOffset;
                    next_box += 1;
                    rpass.set_bind_group(0, &bind_group, &[offset]);
                    rpass.draw(0..3, 0..1);
                }
            }
        }
        self.passes = passes;
        self.passes.clear();
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

/// Where the draw lands and how it samples: the placement's map, its
/// bounds, and the sampling rule (one center sample when pixel-aligned,
/// one center sample inside the interior of an axis-aligned magnified
/// picture without a mask, four subsamples otherwise).
fn set_placement(uniform: &mut ClipUniform, place: &Placement, size: (f64, f64)) {
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
}

/// The conversion of a video frame's planes, as `yuv420p8_into` sets it
/// up: the range's offset and scales, the matrix's coefficients, and
/// the primaries brought into the working space when they differ.
fn set_conversion(uniform: &mut ClipUniform, p: &VideoPlanes<'_>) {
    let tags = p.tags;
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    let kg = 1.0 - kr - kb;
    let (y_off, y_scale, c_scale) = match tags.range {
        geneva_color::Range::Full => (0.0f32, 1.0 / 255.0, 1.0 / 255.0),
        geneva_color::Range::Limited => (16.0, 1.0 / 219.0, 1.0 / 224.0),
    };
    uniform.yuv = [y_off, y_scale, c_scale, 0.0];
    let to_working = primaries::conversion(tags.primaries, Primaries::Bt709);
    uniform.coef = [
        kr as f32,
        kb as f32,
        kg as f32,
        if to_working.is_some() { 1.0 } else { 0.0 },
    ];
    if let Some(m) = to_working {
        let row = |r: [f64; 3]| [r[0] as f32, r[1] as f32, r[2] as f32, 0.0];
        uniform.m0 = row(m[0]);
        uniform.m1 = row(m[1]);
        uniform.m2 = row(m[2]);
    }
}

/// The key a luma mask's image asset is kept under: the one the
/// painter gives the same asset drawn as a picture.
fn image_key(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "image".hash(&mut hasher);
    id.hash(&mut hasher);
    hasher.finish()
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

/// A texture that is sampled and written into, and drawn into and
/// copied from when `drawable`.
fn make_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    drawable: bool,
) -> wgpu::Texture {
    let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
    if drawable {
        usage |= wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    }
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// A free pooled texture of this size, or a new one, marked busy.
fn acquire_in(
    pool: &mut Vec<Pooled>,
    gpu: &Gpu,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    drawable: bool,
) -> usize {
    let (width, height) = (width.max(1), height.max(1));
    if let Some(i) = pool
        .iter()
        .position(|p| !p.busy && p.format == format && p.width == width && p.height == height)
    {
        pool[i].busy = true;
        return i;
    }
    let texture = make_texture(
        gpu.device(),
        "geneva pooled",
        format,
        width,
        height,
        drawable,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    pool.push(Pooled {
        texture,
        view,
        format,
        width,
        height,
        busy: true,
    });
    pool.len() - 1
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Target {
        texture,
        view,
        width,
        height,
    }
}

/// What one pack pass tells the shader, as `frame_to_planes_into` sets
/// it up: the range's scales and offsets at the layout's depth, the
/// matrix's coefficients, the output's primaries when they differ from
/// the working space, the HDR table and peak, the plane's role and the
/// chroma block.
fn pack_uniform(
    tags: ResolvedTags,
    format: PlaneFormat,
    plane: usize,
    width: u32,
    height: u32,
) -> PackUniform {
    let bits = format.bits();
    let shift = bits - 8;
    let max = ((1u32 << bits) - 1) as f32;
    let (y_scale, y_off, c_scale) = match tags.range {
        geneva_color::Range::Full => (max, 0.0, max),
        geneva_color::Range::Limited => (
            (219u32 << shift) as f32,
            (16u32 << shift) as f32,
            (224u32 << shift) as f32,
        ),
    };
    let c_off = (1u32 << (bits - 1)) as f32;
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    let kg = 1.0 - kr - kb;
    let to_output = primaries::conversion(Primaries::Bt709, tags.primaries);
    let row = |r: [f64; 3]| [r[0] as f32, r[1] as f32, r[2] as f32, 0.0];
    let (dx, dy) = format.chroma_divisors();
    PackUniform {
        yuv: [y_scale, y_off, c_scale, c_off],
        coef: [
            kr as f32,
            kb as f32,
            kg as f32,
            if to_output.is_some() { 1.0 } else { 0.0 },
        ],
        m0: to_output.map_or([0.0; 4], |m| row(m[0])),
        m1: to_output.map_or([0.0; 4], |m| row(m[1])),
        m2: to_output.map_or([0.0; 4], |m| row(m[2])),
        max,
        hdr_peak: hdr_peak(tags.transfer) as f32,
        hdr: u32::from(tags.is_hdr()),
        mode: if format.is_rgb() { 3 } else { plane as u32 },
        block: [dx as u32, dy as u32],
        size: [width, height],
    }
}

/// Writes a decoded image into a working-format texture of its size,
/// its f32 channels rounded to halves.
fn write_image(gpu: &Gpu, texture: &wgpu::Texture, img: &Image) {
    if img.width == 0 || img.height == 0 {
        return;
    }
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
