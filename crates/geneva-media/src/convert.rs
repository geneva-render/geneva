//! Conversions between coded pixels and the compositing format.
//!
//! Decoded Y'CbCr comes in as 16-bit 4:4:4 planes (the demuxing layer
//! upsamples chroma and widens bit depth so this module sees one layout),
//! and leaves as 8-bit 4:2:0 for the encoder. Both directions go through
//! `geneva-color`: range, matrix and transfer are applied explicitly rather
//! than delegated to a scaler with implicit defaults.

use std::sync::{Mutex, OnceLock};

use geneva_color::hdr::HdrToSdr;
use geneva_color::{
    LinearRgba, Matrix, Primaries, Range, ResolvedTags, Transfer, matrix, primaries,
};
use geneva_render::{Frame, Image};
use rayon::prelude::*;

/// Number of entries in the 16-bit code lookup tables.
pub const LUT_SIZE: usize = 1 << 16;

/// A lookup table over 16-bit codes.
type Lut = [f32; LUT_SIZE];

/// Lazily built lookup tables, one per transfer function.
type LutCache = OnceLock<Mutex<Vec<(Transfer, &'static Lut)>>>;

fn build_lut(cache: &'static LutCache, transfer: Transfer, f: impl Fn(f64) -> f32) -> &'static Lut {
    let cache = cache.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = cache.lock().expect("lut cache");
    if let Some((_, lut)) = guard.iter().find(|(t, _)| *t == transfer) {
        return lut;
    }
    let values: Vec<f32> = (0..LUT_SIZE)
        .map(|i| f(i as f64 / (LUT_SIZE - 1) as f64))
        .collect();
    let boxed: Box<Lut> = values.into_boxed_slice().try_into().expect("table size");
    let lut: &'static Lut = Box::leak(boxed);
    guard.push((transfer, lut));
    lut
}

/// Per-transfer lookup from a 16-bit non-linear code to linear light.
fn to_linear_lut(transfer: Transfer) -> &'static Lut {
    static LUTS: LutCache = OnceLock::new();
    build_lut(&LUTS, transfer, |x| transfer.to_linear(x) as f32)
}

/// Per-transfer lookup from linear light (quantized to 16 bits over
/// `[0, 1]`) to the non-linear value in `[0, 1]`.
fn from_linear_lut(transfer: Transfer) -> &'static Lut {
    static LUTS: LutCache = OnceLock::new();
    build_lut(&LUTS, transfer, |x| transfer.from_linear(x) as f32)
}

/// A value in `transfer` to the sRGB-encoded value of the same light,
/// and back: markup is laid on in sRGB-encoded values, as the compositor
/// does after decoding the video to linear light, so the direct path
/// goes through the same curve.
fn srgb_recoding(transfer: Transfer) -> (impl Fn(f32) -> f32, impl Fn(f32) -> f32) {
    let same = transfer == Transfer::Srgb;
    let (to_linear, from_linear) = (to_linear_lut(transfer), from_linear_lut(transfer));
    let (srgb_in, srgb_out) = (
        to_linear_lut(Transfer::Srgb),
        from_linear_lut(Transfer::Srgb),
    );
    let to_srgb = move |v: f32| {
        if same {
            v
        } else {
            srgb_out[lut_index(to_linear[lut_index(v)])]
        }
    };
    let from_srgb = move |v: f32| {
        if same {
            v
        } else {
            from_linear[lut_index(srgb_in[lut_index(v)])]
        }
    };
    (to_srgb, from_srgb)
}

/// Per-transfer lookup from linear light over `[0, peak]`, indexed by
/// the fourth root of the fraction of the peak, to the non-linear value:
/// for HDR outputs, whose light runs far past reference white.
fn from_linear_hdr_lut(transfer: Transfer) -> &'static Lut {
    static LUTS: LutCache = OnceLock::new();
    let peak = hdr_peak(transfer);
    build_lut(&LUTS, transfer, move |t| {
        transfer.from_linear(t.powi(4) * peak) as f32
    })
}

/// The table the pack encodes light through: for an SDR output, the
/// transfer function over linear `[0, 1]`, one entry per 16-bit step of
/// the input; for an HDR output (`hdr`), the same over `[0, peak]`
/// indexed by the fourth root of the fraction of [`hdr_peak`]. A
/// renderer that packs on its own device uploads this table so that it
/// quantizes exactly as [`frame_to_planes_into`] does.
pub fn encode_table(transfer: Transfer, hdr: bool) -> &'static [f32; LUT_SIZE] {
    if hdr {
        from_linear_hdr_lut(transfer)
    } else {
        from_linear_lut(transfer)
    }
}

/// The light an HDR transfer can carry, in units of reference white.
pub fn hdr_peak(transfer: Transfer) -> f64 {
    match transfer {
        Transfer::Pq => 10_000.0 / geneva_color::hdr::REFERENCE_WHITE_NITS,
        Transfer::Hlg => 1000.0 / geneva_color::hdr::REFERENCE_WHITE_NITS,
        _ => 1.0,
    }
}

/// Table index for a value nominally in `[0, 1]`. The float-to-integer
/// cast saturates and maps NaN to zero, and the index is narrowed to the
/// table's range, so no bounds check is needed at the lookup.
#[inline]
fn lut_index(v: f32) -> usize {
    let i = (v * (LUT_SIZE - 1) as f32 + 0.5) as i32;
    i.clamp(0, (LUT_SIZE - 1) as i32) as u16 as usize
}

/// Three 16-bit little-endian planes of equal size, row-major, given as
/// bytes with a stride in bytes.
pub struct Planes16<'a> {
    /// Luma or first component.
    pub y: &'a [u8],
    /// First chroma or second component.
    pub cb: &'a [u8],
    /// Second chroma or third component.
    pub cr: &'a [u8],
    /// Bytes per row in each plane.
    pub stride: usize,
}

#[inline]
fn sample16(plane: &[u8], offset: usize) -> f32 {
    f32::from(u16::from_le_bytes([plane[offset], plane[offset + 1]]))
}

/// Converts 16-bit 4:4:4 Y'CbCr planes to a premultiplied linear image.
pub fn ycbcr16_to_image(planes: &Planes16, width: u32, height: u32, tags: ResolvedTags) -> Image {
    let mut image = Image {
        width,
        height,
        pixels: Vec::new(),
        content: None,
    };
    let hdr = HdrToSdr::new(tags, None);
    ycbcr16_into(planes, width, height, tags, hdr.as_ref(), &mut image);
    image
}

/// The matrix taking linear light in `tags`' primaries into the working
/// space, when they differ.
fn to_working_primaries(tags: ResolvedTags) -> Option<[[f32; 3]; 3]> {
    primaries::conversion(tags.primaries, Primaries::Bt709)
        .map(|m| m.map(|row| row.map(|v| v as f32)))
}

#[inline]
fn apply3(m: &[[f32; 3]; 3], [r, g, b]: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * r + m[0][1] * g + m[0][2] * b,
        m[1][0] * r + m[1][1] * g + m[1][2] * b,
        m[2][0] * r + m[2][1] * g + m[2][2] * b,
    ]
}

/// Like [`ycbcr16_to_image`], writing into `out` and keeping its buffer
/// when the size has not changed. HDR material needs the conversion
/// built for its tags and peak ([`HdrToSdr::new`]); with `None`, HDR
/// tags are treated per channel and clip at white.
pub fn ycbcr16_into(
    planes: &Planes16,
    width: u32,
    height: u32,
    tags: ResolvedTags,
    hdr: Option<&HdrToSdr>,
    out: &mut Image,
) {
    let lut = to_linear_lut(tags.transfer);
    let to_working = if hdr.is_some() {
        None
    } else {
        to_working_primaries(tags)
    };
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.0, 0.0));
    let kg = 1.0 - kr - kb;
    let identity = tags.matrix == Matrix::Identity;
    // Range normalization for 16-bit codes.
    let (y_off, y_scale, c_scale) = match tags.range {
        Range::Full => (0.0, 1.0 / 65535.0, 1.0 / 65535.0),
        Range::Limited => (16.0 * 256.0, 1.0 / (219.0 * 256.0), 1.0 / (224.0 * 256.0)),
    };
    let encode = |v: f32| lut[lut_index(v)];
    let w = width as usize;
    out.width = width;
    out.height = height;
    out.pixels
        .resize(w * height as usize, LinearRgba::TRANSPARENT);
    out.pixels
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(row, out)| {
            let base = row * planes.stride;
            for (col, px) in out.iter_mut().enumerate() {
                let at = base + col * 2;
                let y = sample16(planes.y, at);
                let cb = sample16(planes.cb, at);
                let cr = sample16(planes.cr, at);
                let (r, g, b) = if identity {
                    (
                        (y - y_off) * y_scale,
                        (cb - y_off) * y_scale,
                        (cr - y_off) * y_scale,
                    )
                } else {
                    let yn = (y - y_off) * y_scale;
                    let cbn = (cb - 32768.0) * c_scale;
                    let crn = (cr - 32768.0) * c_scale;
                    let r = yn + 2.0 * (1.0 - kr as f32) * crn;
                    let b = yn + 2.0 * (1.0 - kb as f32) * cbn;
                    let g = (yn - kr as f32 * r - kb as f32 * b) / kg as f32;
                    (r, g, b)
                };
                let [r, g, b] = match (hdr, &to_working) {
                    (Some(h), _) => h.convert([r, g, b]),
                    (None, Some(m)) => apply3(m, [encode(r), encode(g), encode(b)]),
                    (None, None) => [encode(r), encode(g), encode(b)],
                };
                *px = LinearRgba { r, g, b, a: 1.0 };
            }
        });
}

/// Three 8-bit planes with chroma subsampled 2×2 (4:2:0), each row
/// `stride` bytes apart; the chroma planes hold `ceil(width / 2)` ×
/// `ceil(height / 2)` samples.
pub struct Planes420<'a> {
    /// Luma.
    pub y: &'a [u8],
    /// Cb.
    pub cb: &'a [u8],
    /// Cr.
    pub cr: &'a [u8],
    /// Bytes per luma row.
    pub y_stride: usize,
    /// Bytes per chroma row.
    pub c_stride: usize,
}

/// Converts an 8-bit 4:2:0 Y'CbCr frame straight to a premultiplied
/// linear image, without the pass through 16-bit 4:4:4 that other
/// layouts take. Chroma is upsampled bilinearly at the usual siting
/// (co-sited with the even luma columns, centered between luma rows).
pub fn yuv420p8_into(
    planes: &Planes420,
    width: u32,
    height: u32,
    tags: ResolvedTags,
    out: &mut Image,
) {
    let lut = to_linear_lut(tags.transfer);
    let to_working = to_working_primaries(tags);
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    let kg = 1.0 - kr - kb;
    let (y_off, y_scale, c_scale) = match tags.range {
        Range::Full => (0.0f32, 1.0 / 255.0, 1.0 / 255.0),
        Range::Limited => (16.0, 1.0 / 219.0, 1.0 / 224.0),
    };
    let (cr_r, cb_b) = (2.0 * (1.0 - kr as f32), 2.0 * (1.0 - kb as f32));
    let (kr, kb, kg) = (kr as f32, kb as f32, kg as f32);
    let w = width as usize;
    let h = height as usize;
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    out.width = width;
    out.height = height;
    out.pixels.resize(w * h, LinearRgba::TRANSPARENT);
    out.pixels
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(row, out)| {
            let y_row = &planes.y[row * planes.y_stride..][..w];
            // The chroma row this luma row belongs to, and the neighbour
            // it is interpolated with: the one above for an even row, the
            // one below for an odd row, at a quarter of the weight.
            let cy = row / 2;
            let other = if row.is_multiple_of(2) {
                cy.saturating_sub(1)
            } else {
                (cy + 1).min(ch - 1)
            };
            let cb0 = &planes.cb[cy * planes.c_stride..][..cw];
            let cb1 = &planes.cb[other * planes.c_stride..][..cw];
            let cr0 = &planes.cr[cy * planes.c_stride..][..cw];
            let cr1 = &planes.cr[other * planes.c_stride..][..cw];
            // The chroma row at this luma row's height, normalized; the
            // odd columns are then the mean of their two neighbours.
            let mut chroma: Vec<(f32, f32)> = Vec::with_capacity(cw);
            for cx in 0..cw {
                let mix = |a: &[u8], b: &[u8]| {
                    (0.75 * f32::from(a[cx]) + 0.25 * f32::from(b[cx]) - 128.0) * c_scale
                };
                chroma.push((mix(cb0, cb1), mix(cr0, cr1)));
            }
            for (x, px) in out.iter_mut().enumerate() {
                let cx = x / 2;
                let (cbn, crn) = if x.is_multiple_of(2) {
                    chroma[cx]
                } else {
                    let (a, b) = (chroma[cx], chroma[(cx + 1).min(cw - 1)]);
                    (f32::midpoint(a.0, b.0), f32::midpoint(a.1, b.1))
                };
                let yn = (f32::from(y_row[x]) - y_off) * y_scale;
                let r = yn + cr_r * crn;
                let b = yn + cb_b * cbn;
                let g = (yn - kr * r - kb * b) / kg;
                let rgb = [lut[lut_index(r)], lut[lut_index(g)], lut[lut_index(b)]];
                let [r, g, b] = match &to_working {
                    Some(m) => apply3(m, rgb),
                    None => rgb,
                };
                *px = LinearRgba { r, g, b, a: 1.0 };
            }
        });
}

/// Converts 8-bit straight-alpha RGBA rows with `stride` bytes per row to a
/// premultiplied linear image using the given transfer function.
pub fn rgba8_to_image(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    transfer: Transfer,
) -> Image {
    let mut image = Image {
        width,
        height,
        pixels: Vec::new(),
        content: None,
    };
    rgba8_into(data, stride, width, height, transfer, &mut image);
    image
}

/// Like [`rgba8_to_image`], writing into `out` and keeping its buffer when
/// the size has not changed.
pub fn rgba8_into(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    transfer: Transfer,
    out: &mut Image,
) {
    let lut = to_linear_lut(transfer);
    let step = (LUT_SIZE - 1) / 255;
    let w = width as usize;
    out.width = width;
    out.height = height;
    out.pixels
        .resize(w * height as usize, LinearRgba::TRANSPARENT);
    out.pixels
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(row, px)| {
            let line = &data[row * stride..row * stride + w * 4];
            for (p, out) in line.chunks_exact(4).zip(px.iter_mut()) {
                let a = f32::from(p[3]) / 255.0;
                *out = LinearRgba {
                    r: lut[p[0] as usize * step] * a,
                    g: lut[p[1] as usize * step] * a,
                    b: lut[p[2] as usize * step] * a,
                    a,
                };
            }
        });
}

/// Planar sample layouts the encoders take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneFormat {
    /// 8-bit Y'CbCr with chroma halved both ways.
    Yuv420p8,
    /// 10-bit Y'CbCr with chroma halved both ways, little-endian words.
    Yuv420p10,
    /// 8-bit Y'CbCr with chroma halved horizontally.
    Yuv422p8,
    /// 10-bit Y'CbCr with chroma halved horizontally, little-endian words.
    Yuv422p10,
    /// 10-bit Y'CbCr with full chroma, little-endian words.
    Yuv444p10,
    /// 8-bit straight-alpha RGBA, one interleaved plane.
    Rgba8,
}

impl PlaneFormat {
    /// Bits per sample.
    pub fn bits(self) -> u32 {
        match self {
            Self::Yuv420p8 | Self::Yuv422p8 | Self::Rgba8 => 8,
            Self::Yuv420p10 | Self::Yuv422p10 | Self::Yuv444p10 => 10,
        }
    }

    /// Bytes per sample in memory.
    pub fn bytes_per_sample(self) -> usize {
        if self.bits() > 8 { 2 } else { 1 }
    }

    /// Chroma subsampling as the horizontal and vertical divisors.
    pub fn chroma_divisors(self) -> (usize, usize) {
        match self {
            Self::Yuv420p8 | Self::Yuv420p10 => (2, 2),
            Self::Yuv422p8 | Self::Yuv422p10 => (2, 1),
            Self::Yuv444p10 | Self::Rgba8 => (1, 1),
        }
    }

    /// Whether the samples are RGB rather than Y'CbCr.
    pub fn is_rgb(self) -> bool {
        matches!(self, Self::Rgba8)
    }

    /// The layout's name as the media libraries spell it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Yuv420p8 => "yuv420p",
            Self::Yuv420p10 => "yuv420p10le",
            Self::Yuv422p8 => "yuv422p",
            Self::Yuv422p10 => "yuv422p10le",
            Self::Yuv444p10 => "yuv444p10le",
            Self::Rgba8 => "rgba",
        }
    }

    /// Size in samples of plane `index` for a picture of `width × height`.
    pub fn plane_size(self, index: usize, width: u32, height: u32) -> (usize, usize) {
        let (w, h) = (width as usize, height as usize);
        if self.is_rgb() {
            return (w * 4, h);
        }
        if index == 0 {
            return (w, h);
        }
        let (dx, dy) = self.chroma_divisors();
        (w.div_ceil(dx), h.div_ceil(dy))
    }

    /// Number of planes.
    pub fn plane_count(self) -> usize {
        if self.is_rgb() { 1 } else { 3 }
    }
}

/// One plane of samples, row-major, rows packed without padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    /// Samples, little-endian when wider than a byte.
    pub data: Vec<u8>,
    /// Samples per row.
    pub width: usize,
    /// Rows.
    pub height: usize,
    /// Bytes per row.
    pub stride: usize,
}

/// A picture as planes ready for an encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planes {
    /// Sample layout.
    pub format: PlaneFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The planes in the layout's order.
    pub planes: Vec<Plane>,
}

impl Planes {
    /// Allocates zeroed planes for `format`.
    pub fn new(format: PlaneFormat, width: u32, height: u32) -> Self {
        let bytes = format.bytes_per_sample();
        let planes = (0..format.plane_count())
            .map(|i| {
                let (w, h) = format.plane_size(i, width, height);
                Plane {
                    data: vec![0; w * h * bytes],
                    width: w,
                    height: h,
                    stride: w * bytes,
                }
            })
            .collect();
        Self {
            format,
            width,
            height,
            planes,
        }
    }
}

/// Stores a sample at index `i` of a plane as one byte or a little-endian
/// word.
#[inline]
fn store(data: &mut [u8], i: usize, value: u16, wide: bool) {
    if wide {
        data[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
    } else {
        data[i] = value as u8;
    }
}

/// Spare frame buffers. A run hands each finished picture back here
/// and takes the next one from the pool, so it does not allocate, zero
/// and page in a fresh frame per picture.
#[derive(Debug, Default)]
pub struct PlanePool {
    spare: Vec<Planes>,
}

impl PlanePool {
    /// At most this many spare frames are kept.
    const CAP: usize = 16;

    /// A buffer laid out for `format` at `width`×`height`: a spare one
    /// when there is one, else newly allocated. Its contents are stale;
    /// the caller writes every sample.
    pub fn take(&mut self, format: PlaneFormat, width: u32, height: u32) -> Planes {
        let fits = |p: &Planes| p.format == format && p.width == width && p.height == height;
        match self.spare.iter().position(fits) {
            Some(i) => self.spare.swap_remove(i),
            None => Planes::new(format, width, height),
        }
    }

    /// Returns a buffer for reuse.
    pub fn give(&mut self, planes: Planes) {
        if self.spare.len() < Self::CAP {
            self.spare.push(planes);
        }
    }
}

/// Converts a rendered frame to encoder planes with the output tags.
///
/// Y'CbCr layouts composite alpha over black; chroma is the average of
/// each block of full-resolution chroma samples. RGBA keeps alpha and
/// encodes with the sRGB curve. Values are rounded to nearest with no
/// dithering, so the output is deterministic.
pub fn frame_to_planes(frame: &Frame, tags: ResolvedTags, format: PlaneFormat) -> Planes {
    let mut out = Planes::new(format, frame.width(), frame.height());
    frame_to_planes_into(frame, tags, format, &mut out);
    out
}

/// [`frame_to_planes`] into `out`, which is laid out for `format` at the
/// frame's size; every sample is written.
pub fn frame_to_planes_into(
    frame: &Frame,
    tags: ResolvedTags,
    format: PlaneFormat,
    out: &mut Planes,
) {
    debug_assert!(
        out.format == format && out.width == frame.width() && out.height == frame.height(),
        "planes are laid out for the frame"
    );
    let width = frame.width();
    let height = frame.height();
    let w = width as usize;
    if format.is_rgb() {
        let plane = &mut out.planes[0];
        plane
            .data
            .par_chunks_mut(w * 4)
            .zip(frame.pixels().par_chunks(w))
            .for_each(|(row, src)| {
                for (px, p) in row.chunks_exact_mut(4).zip(src) {
                    px.copy_from_slice(&p.to_srgb8());
                }
            });
        return;
    }

    let lut = from_linear_lut(tags.transfer);
    // HDR outputs encode light past reference white; the working space
    // is BT.709, so wide-gamut outputs get their primaries on the way out.
    let hdr_lut = tags.is_hdr().then(|| from_linear_hdr_lut(tags.transfer));
    let hdr_peak = hdr_peak(tags.transfer) as f32;
    let to_output = primaries::conversion(Primaries::Bt709, tags.primaries)
        .map(|m| m.map(|row| row.map(|v| v as f32)));
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    let kg = 1.0 - kr - kb;
    let bits = format.bits();
    let shift = bits - 8;
    let max = ((1u32 << bits) - 1) as f32;
    let (y_scale, y_off, c_scale) = match tags.range {
        Range::Full => (max, 0.0, max),
        Range::Limited => (
            (219u32 << shift) as f32,
            (16u32 << shift) as f32,
            (224u32 << shift) as f32,
        ),
    };
    let c_off = (1u32 << (bits - 1)) as f32;
    let wide = format.bytes_per_sample() == 2;
    let (dx, dy) = format.chroma_divisors();
    let cw = w.div_ceil(dx);
    let enc = |v: f32| match hdr_lut {
        Some(h) => h[lut_index((v.max(0.0) / hdr_peak).sqrt().sqrt())],
        None => lut[lut_index(v)],
    };
    let to_ycc = |p: &LinearRgba| {
        // Premultiplied over opaque black is just the premultiplied value.
        let [r, g, b] = match &to_output {
            Some(m) => apply3(m, [p.r, p.g, p.b]),
            None => [p.r, p.g, p.b],
        };
        let (r, g, b) = (enc(r), enc(g), enc(b));
        let y = kr as f32 * r + kg as f32 * g + kb as f32 * b;
        let cb = (b - y) / (2.0 * (1.0 - kb as f32));
        let cr = (r - y) / (2.0 * (1.0 - kr as f32));
        (y, cb, cr)
    };
    let code = |v: f32, scale: f32, off: f32| (v * scale + off + 0.5).clamp(0.0, max) as u16;

    let [y_plane, cb_plane, cr_plane] = &mut out.planes[..] else {
        unreachable!("Y'CbCr layouts have three planes");
    };
    // The common layout, 8-bit 4:2:0 at an even size, has its own loop:
    // one pass per pair of rows over 2×2 blocks with no per-sample
    // branching, which is what the generic loop below costs most on.
    if format == PlaneFormat::Yuv420p8
        && w.is_multiple_of(2)
        && height.is_multiple_of(2)
        && hdr_lut.is_none()
        && to_output.is_none()
    {
        let (ys, yo, cs) = (y_scale, y_off, c_scale);
        let code8 = |v: f32, scale: f32, off: f32| (v * scale + off + 0.5).clamp(0.0, 255.0) as u8;
        frame
            .pixels()
            .par_chunks(w * 2)
            .zip(y_plane.data.par_chunks_mut(y_plane.stride * 2))
            .zip(cb_plane.data.par_chunks_mut(cb_plane.stride))
            .zip(cr_plane.data.par_chunks_mut(cr_plane.stride))
            .for_each(|(((src, y_rows), cb_row), cr_row)| {
                let (top, bottom) = src.split_at(w);
                let (y_top, y_bottom) = y_rows.split_at_mut(y_plane.stride);
                for cx in 0..w / 2 {
                    let x = cx * 2;
                    let (y00, b00, r00) = to_ycc(&top[x]);
                    let (y01, b01, r01) = to_ycc(&top[x + 1]);
                    let (y10, b10, r10) = to_ycc(&bottom[x]);
                    let (y11, b11, r11) = to_ycc(&bottom[x + 1]);
                    y_top[x] = code8(y00, ys, yo);
                    y_top[x + 1] = code8(y01, ys, yo);
                    y_bottom[x] = code8(y10, ys, yo);
                    y_bottom[x + 1] = code8(y11, ys, yo);
                    cb_row[cx] = code8((b00 + b01 + b10 + b11) * 0.25, cs, c_off);
                    cr_row[cx] = code8((r00 + r01 + r10 + r11) * 0.25, cs, c_off);
                }
            });
        return;
    }
    // Each task handles one chroma row: `dy` picture rows, whose chroma is
    // averaged over `dx × dy` blocks (fewer samples at a right or bottom
    // edge).
    frame
        .pixels()
        .par_chunks(w * dy)
        .zip(y_plane.data.par_chunks_mut(y_plane.stride * dy))
        .zip(cb_plane.data.par_chunks_mut(cb_plane.stride))
        .zip(cr_plane.data.par_chunks_mut(cr_plane.stride))
        .for_each(|(((src, y_rows), cb_row), cr_row)| {
            let rows = src.len() / w;
            for cx in 0..cw {
                let (mut sum_b, mut sum_r, mut count) = (0.0f32, 0.0f32, 0.0f32);
                for row in 0..rows {
                    for x in (cx * dx..cx * dx + dx).filter(|&x| x < w) {
                        let (y, cb, cr) = to_ycc(&src[row * w + x]);
                        store(y_rows, row * w + x, code(y, y_scale, y_off), wide);
                        sum_b += cb;
                        sum_r += cr;
                        count += 1.0;
                    }
                }
                store(cb_row, cx, code(sum_b / count, c_scale, c_off), wide);
                store(cr_row, cx, code(sum_r / count, c_scale, c_off), wide);
            }
        });
}

/// Lays one step of the clips above a direct-path base onto its decoded
/// planes: a picture blended over them (in linear light, or in
/// sRGB-encoded values for markup), or a box's backdrop filter done to
/// them.
pub fn lay_overlay(planes: &mut Planes, overlay: &geneva_render::Overlay, tags: ResolvedTags) {
    match overlay {
        geneva_render::Overlay::Picture(frame, rect, false) => {
            blend_overlay(planes, frame, *rect, tags);
        }
        geneva_render::Overlay::Picture(frame, rect, true) => {
            blend_overlay_encoded(planes, frame, *rect, tags);
        }
        geneva_render::Overlay::Backdrop(placed) => backdrop_planes(planes, placed, tags),
    }
}

/// Lays `overlay` (premultiplied linear RGBA drawn over transparency,
/// covering `rect` = `[x0, y0, x1, y1]` of the picture) onto packed
/// planes in place, through the same conversions as [`frame_to_planes`].
/// Only pixels the overlay covers change; for subsampled chroma, the
/// blocks they belong to are recomputed from every pixel in the block.
pub fn blend_overlay(planes: &mut Planes, overlay: &Frame, rect: [u32; 4], tags: ResolvedTags) {
    blend_overlay_in(planes, overlay, rect, tags, false);
}

/// [`blend_overlay`] for an overlay of sRGB-encoded values (markup), laid
/// on in sRGB-encoded R'G'B' as a browser lays a page over a video,
/// rather than in linear light.
pub fn blend_overlay_encoded(
    planes: &mut Planes,
    overlay: &Frame,
    rect: [u32; 4],
    tags: ResolvedTags,
) {
    blend_overlay_in(planes, overlay, rect, tags, true);
}

fn blend_overlay_in(
    planes: &mut Planes,
    overlay: &Frame,
    rect: [u32; 4],
    tags: ResolvedTags,
    encoded: bool,
) {
    let [x0, y0, x1, y1] = rect;
    let x1 = x1.min(planes.width).min(x0 + overlay.width());
    let y1 = y1.min(planes.height).min(y0 + overlay.height());
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let ow = overlay.width() as usize;
    let src_at = |x: u32, y: u32| -> LinearRgba {
        if x < x0 || x >= x1 || y < y0 || y >= y1 {
            return LinearRgba::TRANSPARENT;
        }
        overlay.pixels()[(y - y0) as usize * ow + (x - x0) as usize]
    };
    let w = planes.width as usize;
    let format = planes.format;
    if format.is_rgb() {
        let plane = &mut planes.planes[0];
        let stride = plane.stride;
        let rows = &mut plane.data[y0 as usize * stride..y1 as usize * stride];
        rows.par_chunks_mut(stride)
            .enumerate()
            .for_each(|(i, row)| {
                let y = y0 + i as u32;
                for x in x0..x1 {
                    let src = src_at(x, y);
                    if src.a <= 0.0 {
                        continue;
                    }
                    let px = &mut row[x as usize * 4..x as usize * 4 + 4];
                    if encoded {
                        let base = LinearRgba {
                            r: f32::from(px[0]) / 255.0,
                            g: f32::from(px[1]) / 255.0,
                            b: f32::from(px[2]) / 255.0,
                            a: 1.0,
                        };
                        let out = src.over(base);
                        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        px.copy_from_slice(&[byte(out.r), byte(out.g), byte(out.b), 255]);
                        continue;
                    }
                    let base =
                        geneva_color::Color::from_rgba8(px[0], px[1], px[2], 255).to_linear();
                    let out = src.over(base).to_srgb8();
                    px.copy_from_slice(&out);
                }
            });
        return;
    }

    let to_linear = to_linear_lut(tags.transfer);
    let from_linear = from_linear_lut(tags.transfer);
    let (to_srgb, from_srgb) = srgb_recoding(tags.transfer);
    let bits = format.bits();
    let max = f64::from((1u32 << bits) - 1);
    let wide = format.bytes_per_sample() == 2;
    let (dx, dy) = format.chroma_divisors();
    let load = |data: &[u8], i: usize| -> f64 {
        if wide {
            f64::from(u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]))
        } else {
            f64::from(data[i])
        }
    };
    let (bx0, bx1) = (x0 as usize / dx, (x1 as usize).div_ceil(dx));
    let (by0, by1) = (y0 as usize / dy, (y1 as usize).div_ceil(dy));
    let height = planes.height as usize;
    let [y_plane, cb_plane, cr_plane] = &mut planes.planes[..] else {
        unreachable!("Y'CbCr layouts have three planes");
    };
    let y_stride = y_plane.stride;
    let y_rows = &mut y_plane.data[by0 * dy * y_stride..(by1 * dy).min(height) * y_stride];
    let cb_rows = &mut cb_plane.data[by0 * cb_plane.stride..by1 * cb_plane.stride];
    let cr_rows = &mut cr_plane.data[by0 * cr_plane.stride..by1 * cr_plane.stride];
    let (cb_stride, cr_stride) = (cb_plane.stride, cr_plane.stride);
    y_rows
        .par_chunks_mut(y_stride * dy)
        .zip(cb_rows.par_chunks_mut(cb_stride))
        .zip(cr_rows.par_chunks_mut(cr_stride))
        .enumerate()
        .for_each(|(bi, ((y_block_rows, cb_row), cr_row))| {
            let by = by0 + bi;
            let rows = y_block_rows.len() / y_stride;
            for bx in bx0..bx1 {
                // A range, not a collected list: most blocks of a wide
                // overlay are transparent, and this runs for every one.
                let xs = bx * dx..(bx * dx + dx).min(w);
                let covered = (0..rows).any(|r| {
                    xs.clone()
                        .any(|x| src_at(x as u32, (by * dy + r) as u32).a > 0.0)
                });
                if !covered {
                    continue;
                }
                let cb = load(cb_row, bx);
                let cr = load(cr_row, bx);
                let (mut sum_b, mut sum_r, mut count) = (0.0f64, 0.0f64, 0.0f64);
                for r in 0..rows {
                    for x in xs.clone() {
                        let yi = r * y_stride / format.bytes_per_sample() + x;
                        let y_code = load(y_block_rows, yi);
                        let [yn, cbn, crn] =
                            matrix::decode_ycbcr(tags.range, bits, [y_code, cb, cr]);
                        let rgb = matrix::ycbcr_to_rgb(tags.matrix, [yn, cbn, crn]);
                        let level = |v: f64| v.clamp(0.0, 1.0) as f32;
                        let base = if encoded {
                            LinearRgba {
                                r: to_srgb(level(rgb[0])),
                                g: to_srgb(level(rgb[1])),
                                b: to_srgb(level(rgb[2])),
                                a: 1.0,
                            }
                        } else {
                            LinearRgba {
                                r: to_linear[lut_index(level(rgb[0]))],
                                g: to_linear[lut_index(level(rgb[1]))],
                                b: to_linear[lut_index(level(rgb[2]))],
                                a: 1.0,
                            }
                        };
                        let src = src_at(x as u32, (by * dy + r) as u32);
                        let out = if src.a > 0.0 { src.over(base) } else { base };
                        let enc = if encoded {
                            [out.r, out.g, out.b].map(|v| f64::from(from_srgb(v)))
                        } else {
                            [
                                f64::from(from_linear[lut_index(out.r)]),
                                f64::from(from_linear[lut_index(out.g)]),
                                f64::from(from_linear[lut_index(out.b)]),
                            ]
                        };
                        let ycc = matrix::rgb_to_ycbcr(tags.matrix, enc);
                        let [yc, cbc, crc] = matrix::encode_ycbcr(tags.range, bits, ycc);
                        if src.a > 0.0 {
                            store(y_block_rows, yi, (yc + 0.5).clamp(0.0, max) as u16, wide);
                        }
                        sum_b += cbc;
                        sum_r += crc;
                        count += 1.0;
                    }
                }
                store(
                    cb_row,
                    bx,
                    (sum_b / count + 0.5).clamp(0.0, max) as u16,
                    wide,
                );
                store(
                    cr_row,
                    bx,
                    (sum_r / count + 0.5).clamp(0.0, max) as u16,
                    wide,
                );
            }
        });
}

/// Does a box's `backdrop-filter` to decoded planes, for the direct path:
/// the region under the box read as sRGB-encoded R'G'B' (through linear
/// light, as the compositor reads it), filtered and mixed in as
/// [`geneva_render::backdrop`] does for a composited frame, and written
/// back. Luma is written where the box reaches; a chroma sample takes the
/// mean of its block, as [`blend_overlay`] does.
pub fn backdrop_planes(
    planes: &mut Planes,
    placed: &geneva_render::backdrop::PlacedBackdrop,
    tags: ResolvedTags,
) {
    use geneva_render::backdrop::filter_region;
    let [bx0, by0, bx1, by1] = placed.bounds;
    let x1 = bx1.min(planes.width);
    let y1 = by1.min(planes.height);
    let format = planes.format;
    let (dx, dy) = if format.is_rgb() {
        (1, 1)
    } else {
        format.chroma_divisors()
    };
    // Whole chroma blocks, so every block written is read whole.
    let x0 = (bx0 as usize / dx * dx) as u32;
    let y0 = (by0 as usize / dy * dy) as u32;
    let x1 = ((x1 as usize).div_ceil(dx) * dx).min(planes.width as usize) as u32;
    let y1 = ((y1 as usize).div_ceil(dy) * dy).min(planes.height as usize) as u32;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut region = vec![[0.0f32; 4]; w * h];

    if format.is_rgb() {
        let plane = &mut planes.planes[0];
        let stride = plane.stride;
        for y in 0..h {
            for x in 0..w {
                let i = (y0 as usize + y) * stride + (x0 as usize + x) * 4;
                let px = &plane.data[i..i + 4];
                region[y * w + x] = [
                    f32::from(px[0]) / 255.0,
                    f32::from(px[1]) / 255.0,
                    f32::from(px[2]) / 255.0,
                    1.0,
                ];
            }
        }
        let original = region.clone();
        filter_region(&mut region, w, h, &placed.backdrop.filters, placed.scale());
        for y in 0..h {
            for x in 0..w {
                let k = placed.weight(x0 + x as u32, y0 + y as u32);
                if k <= 0.0 {
                    continue;
                }
                let (o, f) = (original[y * w + x], region[y * w + x]);
                let i = (y0 as usize + y) * stride + (x0 as usize + x) * 4;
                for c in 0..3 {
                    let v = o[c] + (f[c] - o[c]) * k;
                    plane.data[i + c] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                }
            }
        }
        return;
    }

    let bits = format.bits();
    let max = f64::from((1u32 << bits) - 1);
    let wide = format.bytes_per_sample() == 2;
    let bps = format.bytes_per_sample();
    let load = |data: &[u8], i: usize| -> f64 {
        if wide {
            f64::from(u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]))
        } else {
            f64::from(data[i])
        }
    };
    let (to_srgb, from_srgb) = srgb_recoding(tags.transfer);
    let [y_plane, cb_plane, cr_plane] = &mut planes.planes[..] else {
        unreachable!("Y'CbCr layouts have three planes");
    };
    let (ys, cbs, crs) = (
        y_plane.stride / bps,
        cb_plane.stride / bps,
        cr_plane.stride / bps,
    );
    for y in 0..h {
        let (gy, cy) = (y0 as usize + y, (y0 as usize + y) / dy);
        for x in 0..w {
            let (gx, cx) = (x0 as usize + x, (x0 as usize + x) / dx);
            let ycc = [
                load(&y_plane.data, gy * ys + gx),
                load(&cb_plane.data, cy * cbs + cx),
                load(&cr_plane.data, cy * crs + cx),
            ];
            let n = matrix::decode_ycbcr(tags.range, bits, ycc);
            let rgb = matrix::ycbcr_to_rgb(tags.matrix, n);
            let level = |v: f64| to_srgb(v.clamp(0.0, 1.0) as f32);
            region[y * w + x] = [level(rgb[0]), level(rgb[1]), level(rgb[2]), 1.0];
        }
    }
    let original = region.clone();
    filter_region(&mut region, w, h, &placed.backdrop.filters, placed.scale());
    // Mixed, back to Y'CbCr: luma per pixel, chroma summed per block.
    let (cw, ch) = (w / dx, h / dy);
    let mut chroma = vec![(0.0f64, 0.0f64, 0.0f64, false); cw * ch];
    for y in 0..h {
        for x in 0..w {
            let (gx, gy) = (x0 as usize + x, y0 as usize + y);
            let k = placed.weight(gx as u32, gy as u32);
            let (o, f) = (original[y * w + x], region[y * w + x]);
            let out = [0, 1, 2].map(|c| f64::from(from_srgb(o[c] + (f[c] - o[c]) * k)));
            let ycc =
                matrix::encode_ycbcr(tags.range, bits, matrix::rgb_to_ycbcr(tags.matrix, out));
            if k > 0.0 {
                store(
                    &mut y_plane.data,
                    gy * ys + gx,
                    (ycc[0] + 0.5).clamp(0.0, max) as u16,
                    wide,
                );
            }
            let block = &mut chroma[(y / dy) * cw + x / dx];
            block.0 += ycc[1];
            block.1 += ycc[2];
            block.2 += 1.0;
            block.3 |= k > 0.0;
        }
    }
    for by in 0..ch {
        for bx in 0..cw {
            let (sb, sr, n, touched) = chroma[by * cw + bx];
            if !touched {
                continue;
            }
            let (cx, cy) = (x0 as usize / dx + bx, y0 as usize / dy + by);
            store(
                &mut cb_plane.data,
                cy * cbs + cx,
                (sb / n + 0.5).clamp(0.0, max) as u16,
                wide,
            );
            store(
                &mut cr_plane.data,
                cy * crs + cx,
                (sr / n + 0.5).clamp(0.0, max) as u16,
                wide,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geneva_color::Color;

    #[test]
    fn white_and_black_hit_the_limited_range_anchors() {
        let white = Frame::new(2, 2, Color::WHITE);
        let yuv = frame_to_planes(&white, ResolvedTags::SDR_VIDEO, PlaneFormat::Yuv420p8);
        assert_eq!(yuv.planes[0].data, [235; 4]);
        assert_eq!(yuv.planes[1].data, [128]);
        assert_eq!(yuv.planes[2].data, [128]);
        let black = Frame::new(2, 2, Color::BLACK);
        let yuv = frame_to_planes(&black, ResolvedTags::SDR_VIDEO, PlaneFormat::Yuv420p8);
        assert_eq!(yuv.planes[0].data, [16; 4]);
    }

    #[test]
    fn saturated_red_matches_bt709_reference_codes() {
        let red = Frame::new(2, 2, Color::from_rgba8(255, 0, 0, 255));
        let yuv = frame_to_planes(&red, ResolvedTags::SDR_VIDEO, PlaneFormat::Yuv420p8);
        // Y' = 0.2126 * 219 + 16 = 62.6; Cr = 0.5 * 224 + 128 = 240; Cb = -0.1146 * 224 + 128 = 102.3
        assert_eq!(yuv.planes[0].data[0], 63);
        assert_eq!(yuv.planes[2].data[0], 240);
        assert_eq!(yuv.planes[1].data[0], 102);
    }

    #[test]
    fn decoding_the_encoded_anchors_returns_the_colors() {
        for (r, g, b) in [
            (255u8, 255u8, 255u8),
            (0, 0, 0),
            (255, 136, 0),
            (30, 90, 200),
            (128, 128, 128),
        ] {
            let frame = Frame::new(2, 2, Color::from_rgba8(r, g, b, 255));
            let yuv = frame_to_planes(&frame, ResolvedTags::SDR_VIDEO, PlaneFormat::Yuv420p8);
            // Widen to 16-bit 4:4:4 the way swscale does: a plain shift.
            let wide = |v: u8| (u16::from(v) << 8).to_le_bytes();
            let y: Vec<u8> = yuv.planes[0].data.iter().flat_map(|&v| wide(v)).collect();
            let cb: Vec<u8> = std::iter::repeat_n(wide(yuv.planes[1].data[0]), 4)
                .flatten()
                .collect();
            let cr: Vec<u8> = std::iter::repeat_n(wide(yuv.planes[2].data[0]), 4)
                .flatten()
                .collect();
            let img = ycbcr16_to_image(
                &Planes16 {
                    y: &y,
                    cb: &cb,
                    cr: &cr,
                    stride: 4,
                },
                2,
                2,
                ResolvedTags::SDR_VIDEO,
            );
            let back = img.pixels[0].to_srgb8();
            for (got, want) in back[..3].iter().zip([r, g, b]) {
                assert!(got.abs_diff(want) <= 2, "{:?} vs {:?}", back, (r, g, b));
            }
            assert_eq!(back[3], 255);
        }
    }

    #[test]
    fn eight_bit_420_converts_straight_to_the_colors() {
        for (r, g, b) in [
            (255u8, 255u8, 255u8),
            (0, 0, 0),
            (255, 136, 0),
            (30, 90, 200),
            (128, 128, 128),
        ] {
            // A 4×4 frame of one color: its 4:2:0 planes are what the
            // encoder writes, and the way back must give the color.
            let frame = Frame::new(4, 4, Color::from_rgba8(r, g, b, 255));
            let yuv = frame_to_planes(&frame, ResolvedTags::SDR_VIDEO, PlaneFormat::Yuv420p8);
            let mut img = Image {
                width: 0,
                height: 0,
                pixels: Vec::new(),
                content: None,
            };
            yuv420p8_into(
                &Planes420 {
                    y: &yuv.planes[0].data,
                    cb: &yuv.planes[1].data,
                    cr: &yuv.planes[2].data,
                    y_stride: yuv.planes[0].stride,
                    c_stride: yuv.planes[1].stride,
                },
                4,
                4,
                ResolvedTags::SDR_VIDEO,
                &mut img,
            );
            assert_eq!((img.width, img.height, img.pixels.len()), (4, 4, 16));
            for px in &img.pixels {
                let back = px.to_srgb8();
                for (got, want) in back[..3].iter().zip([r, g, b]) {
                    assert!(got.abs_diff(want) <= 2, "{:?} vs {:?}", back, (r, g, b));
                }
                assert_eq!(back[3], 255);
            }
        }
    }

    #[test]
    fn ten_bit_layouts_scale_the_anchors_and_keep_full_chroma() {
        let white = Frame::new(3, 1, Color::WHITE);
        let p = frame_to_planes(&white, ResolvedTags::SDR_VIDEO, PlaneFormat::Yuv422p10);
        let y = u16::from_le_bytes([p.planes[0].data[0], p.planes[0].data[1]]);
        assert_eq!(y, 940);
        assert_eq!(p.planes[1].width, 2);
        let cb = u16::from_le_bytes([p.planes[1].data[0], p.planes[1].data[1]]);
        assert_eq!(cb, 512);
        let p = frame_to_planes(&white, ResolvedTags::SDR_VIDEO, PlaneFormat::Yuv444p10);
        assert_eq!(p.planes[1].width, 3);
        let rgba = frame_to_planes(&white, ResolvedTags::SDR_VIDEO, PlaneFormat::Rgba8);
        assert_eq!(&rgba.planes[0].data[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn rgba_rows_honor_stride_and_premultiply() {
        let data = [255, 0, 0, 255, 9, 9, 9, 9, 0, 0, 255, 128, 9, 9, 9, 9];
        let img = rgba8_to_image(&data, 8, 1, 2, Transfer::Srgb);
        assert_eq!(img.pixels[0].to_srgb8(), [255, 0, 0, 255]);
        let p = img.pixels[1];
        assert!((p.a - 128.0 / 255.0).abs() < 1e-6);
        assert!(
            (p.b - p.a).abs() < 1e-6,
            "premultiplied blue should equal alpha"
        );
    }
    #[test]
    fn pool_reuses_a_matching_buffer_and_allocates_otherwise() {
        let mut pool = PlanePool::default();
        let mut a = pool.take(PlaneFormat::Yuv420p8, 4, 2);
        a.planes[0].data[0] = 7;
        pool.give(a);
        // The same layout comes back with its stale contents.
        let b = pool.take(PlaneFormat::Yuv420p8, 4, 2);
        assert_eq!(b.planes[0].data[0], 7);
        // A different layout is a fresh, zeroed buffer.
        let c = pool.take(PlaneFormat::Rgba8, 4, 2);
        assert_eq!(c.format, PlaneFormat::Rgba8);
        assert!(c.planes[0].data.iter().all(|&v| v == 0));
        pool.give(b);
        pool.give(c);
        assert_eq!(
            pool.take(PlaneFormat::Rgba8, 4, 2).format,
            PlaneFormat::Rgba8
        );
        assert_eq!(pool.take(PlaneFormat::Yuv420p8, 4, 2).planes[0].data[0], 7);
        // Beyond the cap, buffers are dropped.
        for _ in 0..PlanePool::CAP + 4 {
            pool.give(Planes::new(PlaneFormat::Yuv420p8, 2, 2));
        }
        assert_eq!(pool.spare.len(), PlanePool::CAP);
    }
}
