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

/// 8-bit 4:2:0 planes ready for an encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420p {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Luma, `width × height`.
    pub y: Vec<u8>,
    /// Cb, `ceil(width/2) × ceil(height/2)`.
    pub cb: Vec<u8>,
    /// Cr, same size as `cb`.
    pub cr: Vec<u8>,
}

impl Yuv420p {
    /// Chroma plane width.
    pub fn chroma_width(&self) -> u32 {
        self.width.div_ceil(2)
    }

    /// Chroma plane height.
    pub fn chroma_height(&self) -> u32 {
        self.height.div_ceil(2)
    }
}

/// Converts a rendered frame to 8-bit 4:2:0 Y'CbCr with the output tags.
///
/// Alpha is composited over black. Chroma is the average of each 2×2 block
/// of full-resolution chroma samples. Values are rounded to nearest, with
/// no dithering, so the output is deterministic.
pub fn frame_to_yuv420p(frame: &Frame, tags: ResolvedTags) -> Yuv420p {
    let width = frame.width();
    let height = frame.height();
    let lut = from_linear_lut(tags.transfer);
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    let kg = 1.0 - kr - kb;
    let (y_scale, y_off, c_scale) = match tags.range {
        Range::Full => (255.0, 0.0, 255.0),
        Range::Limited => (219.0, 16.0, 224.0),
    };
    let w = width as usize;
    let h = height as usize;
    let cw = width.div_ceil(2) as usize;
    let ch = height.div_ceil(2) as usize;
    let mut y_plane = vec![0u8; w * h];
    let mut cb_plane = vec![0u8; cw * ch];
    let mut cr_plane = vec![0u8; cw * ch];
    // Premultiplied over opaque black is just the premultiplied value.
    let enc = |v: f32| lut[lut_index(v)];
    let to_ycc = |p: &LinearRgba| {
        let (r, g, b) = (enc(p.r), enc(p.g), enc(p.b));
        let y = kr as f32 * r + kg as f32 * g + kb as f32 * b;
        let cb = (b - y) / (2.0 * (1.0 - kb as f32));
        let cr = (r - y) / (2.0 * (1.0 - kr as f32));
        (y, cb, cr)
    };
    let code = |v: f32, scale: f32, off: f32| (v * scale + off + 0.5).clamp(0.0, 255.0) as u8;
    // Each task handles one chroma row: two picture rows, whose chroma is
    // averaged over 2×2 blocks (or fewer samples at a right or bottom edge).
    frame
        .pixels()
        .par_chunks(w * 2)
        .zip(y_plane.par_chunks_mut(w * 2))
        .zip(cb_plane.par_chunks_mut(cw))
        .zip(cr_plane.par_chunks_mut(cw))
        .for_each(|(((src, y_rows), cb_row), cr_row)| {
            let rows = src.len() / w;
            for cx in 0..cw {
                let (mut sum_b, mut sum_r, mut count) = (0.0f32, 0.0f32, 0.0f32);
                for dy in 0..rows {
                    for x in (cx * 2..cx * 2 + 2).filter(|&x| x < w) {
                        let (y, cb, cr) = to_ycc(&src[dy * w + x]);
                        y_rows[dy * w + x] = code(y, y_scale, y_off);
                        sum_b += cb;
                        sum_r += cr;
                        count += 1.0;
                    }
                }
                cb_row[cx] = code(sum_b / count, c_scale, 128.0);
                cr_row[cx] = code(sum_r / count, c_scale, 128.0);
            }
        });
    Yuv420p {
        width,
        height,
        y: y_plane,
        cb: cb_plane,
        cr: cr_plane,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geneva_color::Color;

    #[test]
    fn white_and_black_hit_the_limited_range_anchors() {
        let white = Frame::new(2, 2, Color::WHITE);
        let yuv = frame_to_yuv420p(&white, ResolvedTags::SDR_VIDEO);
        assert_eq!(yuv.y, [235; 4]);
        assert_eq!(yuv.cb, [128]);
        assert_eq!(yuv.cr, [128]);
        let black = Frame::new(2, 2, Color::BLACK);
        let yuv = frame_to_yuv420p(&black, ResolvedTags::SDR_VIDEO);
        assert_eq!(yuv.y, [16; 4]);
    }

    #[test]
    fn saturated_red_matches_bt709_reference_codes() {
        let red = Frame::new(2, 2, Color::from_rgba8(255, 0, 0, 255));
        let yuv = frame_to_yuv420p(&red, ResolvedTags::SDR_VIDEO);
        // Y' = 0.2126 * 219 + 16 = 62.6; Cr = 0.5 * 224 + 128 = 240; Cb = -0.1146 * 224 + 128 = 102.3
        assert_eq!(yuv.y[0], 63);
        assert_eq!(yuv.cr[0], 240);
        assert_eq!(yuv.cb[0], 102);
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
            let yuv = frame_to_yuv420p(&frame, ResolvedTags::SDR_VIDEO);
            // Widen to 16-bit 4:4:4 the way swscale does: a plain shift.
            let wide = |v: u8| (u16::from(v) << 8).to_le_bytes();
            let y: Vec<u8> = yuv.y.iter().flat_map(|&v| wide(v)).collect();
            let cb: Vec<u8> = std::iter::repeat_n(wide(yuv.cb[0]), 4).flatten().collect();
            let cr: Vec<u8> = std::iter::repeat_n(wide(yuv.cr[0]), 4).flatten().collect();
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
