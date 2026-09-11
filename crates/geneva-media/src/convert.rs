//! Conversions between coded pixels and the compositing format.
//!
//! Decoded Y'CbCr comes in as 16-bit 4:4:4 planes (the demuxing layer
//! upsamples chroma and widens bit depth so this module sees one layout),
//! and leaves as 8-bit 4:2:0 for the encoder. Both directions go through
//! `geneva-color`: range, matrix and transfer are applied explicitly rather
//! than delegated to a scaler with implicit defaults.

use std::sync::{Mutex, OnceLock};

use geneva_color::{LinearRgba, Matrix, Range, ResolvedTags, Transfer, matrix};
use geneva_render::{Frame, Image};
use rayon::prelude::*;

/// Number of entries in the 16-bit code lookup tables.
const LUT_SIZE: usize = 1 << 16;

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
    };
    ycbcr16_into(planes, width, height, tags, &mut image);
    image
}

/// Like [`ycbcr16_to_image`], writing into `out` and keeping its buffer
/// when the size has not changed.
pub fn ycbcr16_into(
    planes: &Planes16,
    width: u32,
    height: u32,
    tags: ResolvedTags,
    out: &mut Image,
) {
    let lut = to_linear_lut(tags.transfer);
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
                *px = LinearRgba {
                    r: encode(r),
                    g: encode(g),
                    b: encode(b),
                    a: 1.0,
                };
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
            Self::Yuv422p10 | Self::Yuv444p10 => 10,
        }
    }

    /// Bytes per sample in memory.
    pub fn bytes_per_sample(self) -> usize {
        if self.bits() > 8 { 2 } else { 1 }
    }

    /// Chroma subsampling as the horizontal and vertical divisors.
    pub fn chroma_divisors(self) -> (usize, usize) {
        match self {
            Self::Yuv420p8 => (2, 2),
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

/// Converts a rendered frame to encoder planes with the output tags.
///
/// Y'CbCr layouts composite alpha over black; chroma is the average of
/// each block of full-resolution chroma samples. RGBA keeps alpha and
/// encodes with the sRGB curve. Values are rounded to nearest with no
/// dithering, so the output is deterministic.
pub fn frame_to_planes(frame: &Frame, tags: ResolvedTags, format: PlaneFormat) -> Planes {
    let width = frame.width();
    let height = frame.height();
    let mut out = Planes::new(format, width, height);
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
        return out;
    }

    let lut = from_linear_lut(tags.transfer);
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
    let enc = |v: f32| lut[lut_index(v)];
    let to_ycc = |p: &LinearRgba| {
        // Premultiplied over opaque black is just the premultiplied value.
        let (r, g, b) = (enc(p.r), enc(p.g), enc(p.b));
        let y = kr as f32 * r + kg as f32 * g + kb as f32 * b;
        let cb = (b - y) / (2.0 * (1.0 - kb as f32));
        let cr = (r - y) / (2.0 * (1.0 - kr as f32));
        (y, cb, cr)
    };
    let code = |v: f32, scale: f32, off: f32| (v * scale + off + 0.5).clamp(0.0, max) as u16;

    let [y_plane, cb_plane, cr_plane] = &mut out.planes[..] else {
        unreachable!("Y'CbCr layouts have three planes");
    };
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
    out
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
}
