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

/// Number of entries in the 16-bit code lookup tables.
const LUT_SIZE: usize = 1 << 16;

/// Lazily built lookup tables, one per transfer function.
type LutCache = OnceLock<Mutex<Vec<(Transfer, &'static [f32])>>>;

/// Per-transfer lookup from a 16-bit non-linear code to linear light.
fn to_linear_lut(transfer: Transfer) -> &'static [f32] {
    static LUTS: LutCache = OnceLock::new();
    let cache = LUTS.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = cache.lock().expect("lut cache");
    if let Some((_, lut)) = guard.iter().find(|(t, _)| *t == transfer) {
        return lut;
    }
    let lut: Vec<f32> = (0..LUT_SIZE)
        .map(|i| transfer.to_linear(i as f64 / (LUT_SIZE - 1) as f64) as f32)
        .collect();
    let lut: &'static [f32] = Box::leak(lut.into_boxed_slice());
    guard.push((transfer, lut));
    lut
}

/// Per-transfer lookup from linear light (quantized to 16 bits over
/// `[0, 1]`) to an 8-bit-scaled non-linear value in `[0, 255]`.
fn from_linear_lut(transfer: Transfer) -> &'static [f32] {
    static LUTS: LutCache = OnceLock::new();
    let cache = LUTS.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = cache.lock().expect("lut cache");
    if let Some((_, lut)) = guard.iter().find(|(t, _)| *t == transfer) {
        return lut;
    }
    let lut: Vec<f32> = (0..LUT_SIZE)
        .map(|i| (transfer.from_linear(i as f64 / (LUT_SIZE - 1) as f64) * 255.0) as f32)
        .collect();
    let lut: &'static [f32] = Box::leak(lut.into_boxed_slice());
    guard.push((transfer, lut));
    lut
}

/// Three 16-bit planes of equal size, little-endian, row-major with a
/// stride in samples.
pub struct Planes16<'a> {
    /// Luma or first component.
    pub y: &'a [u16],
    /// First chroma or second component.
    pub cb: &'a [u16],
    /// Second chroma or third component.
    pub cr: &'a [u16],
    /// Samples per row in each plane.
    pub stride: usize,
}

/// Converts 16-bit 4:4:4 Y'CbCr planes to a premultiplied linear image.
pub fn ycbcr16_to_image(planes: &Planes16, width: u32, height: u32, tags: ResolvedTags) -> Image {
    let lut = to_linear_lut(tags.transfer);
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.0, 0.0));
    let kg = 1.0 - kr - kb;
    let identity = tags.matrix == Matrix::Identity;
    // Range normalization for 16-bit codes.
    let (y_off, y_scale, c_scale) = match tags.range {
        Range::Full => (0.0, 1.0 / 65535.0, 1.0 / 65535.0),
        Range::Limited => (16.0 * 256.0, 1.0 / (219.0 * 256.0), 1.0 / (224.0 * 256.0)),
    };
    let max_index = (LUT_SIZE - 1) as f32;
    let encode = |v: f32| lut[(v.clamp(0.0, 1.0) * max_index + 0.5) as usize];
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for row in 0..height as usize {
        let base = row * planes.stride;
        for col in 0..width as usize {
            let y = f32::from(planes.y[base + col]);
            let cb = f32::from(planes.cb[base + col]);
            let cr = f32::from(planes.cr[base + col]);
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
            pixels.push(LinearRgba {
                r: encode(r),
                g: encode(g),
                b: encode(b),
                a: 1.0,
            });
        }
    }
    Image {
        width,
        height,
        pixels,
    }
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
    let lut = to_linear_lut(transfer);
    let step = (LUT_SIZE - 1) / 255;
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for row in 0..height as usize {
        let line = &data[row * stride..row * stride + width as usize * 4];
        for p in line.chunks_exact(4) {
            let a = f32::from(p[3]) / 255.0;
            pixels.push(LinearRgba {
                r: lut[p[0] as usize * step] * a,
                g: lut[p[1] as usize * step] * a,
                b: lut[p[2] as usize * step] * a,
                a,
            });
        }
    }
    Image {
        width,
        height,
        pixels,
    }
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
    let max_index = (LUT_SIZE - 1) as f32;
    let (kr, kb) = matrix::luma_coefficients(tags.matrix).unwrap_or((0.2126, 0.0722));
    let kg = 1.0 - kr - kb;
    let (y_scale, y_off, c_scale) = match tags.range {
        Range::Full => (255.0, 0.0, 255.0),
        Range::Limited => (219.0, 16.0, 224.0),
    };
    let n = width as usize * height as usize;
    let mut y_plane = vec![0u8; n];
    let mut cb_full = vec![0f32; n];
    let mut cr_full = vec![0f32; n];
    for (i, p) in frame.pixels().iter().enumerate() {
        // Premultiplied over opaque black is just the premultiplied value.
        let enc = |v: f32| lut[(v.clamp(0.0, 1.0) * max_index + 0.5) as usize] / 255.0;
        let (r, g, b) = (enc(p.r), enc(p.g), enc(p.b));
        let y = kr as f32 * r + kg as f32 * g + kb as f32 * b;
        let cb = (b - y) / (2.0 * (1.0 - kb as f32));
        let cr = (r - y) / (2.0 * (1.0 - kr as f32));
        y_plane[i] = (y * y_scale + y_off + 0.5).clamp(0.0, 255.0) as u8;
        cb_full[i] = cb;
        cr_full[i] = cr;
    }
    let cw = width.div_ceil(2) as usize;
    let ch = height.div_ceil(2) as usize;
    let mut cb_plane = vec![0u8; cw * ch];
    let mut cr_plane = vec![0u8; cw * ch];
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut sum_b, mut sum_r, mut count) = (0.0f32, 0.0f32, 0.0f32);
            for dy in 0..2 {
                for dx in 0..2 {
                    let x = cx * 2 + dx;
                    let y = cy * 2 + dy;
                    if x < width as usize && y < height as usize {
                        sum_b += cb_full[y * width as usize + x];
                        sum_r += cr_full[y * width as usize + x];
                        count += 1.0;
                    }
                }
            }
            let i = cy * cw + cx;
            cb_plane[i] = (sum_b / count * c_scale + 128.0 + 0.5).clamp(0.0, 255.0) as u8;
            cr_plane[i] = (sum_r / count * c_scale + 128.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
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
            let wide = |v: u8| u16::from(v) << 8;
            let y: Vec<u16> = yuv.y.iter().map(|&v| wide(v)).collect();
            let cb = vec![wide(yuv.cb[0]); 4];
            let cr = vec![wide(yuv.cr[0]); 4];
            let img = ycbcr16_to_image(
                &Planes16 {
                    y: &y,
                    cb: &cb,
                    cr: &cr,
                    stride: 2,
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
