//! Y'CbCr ↔ R'G'B' conversion with range handling.
//!
//! Conversions operate on normalized non-linear values: R'G'B' in `[0, 1]`,
//! Y' in `[0, 1]` and Cb/Cr in `[-0.5, 0.5]`. Integer code values are
//! normalized with [`decode_ycbcr`] and [`encode_ycbcr`], which handle the
//! limited/full range scaling for any bit depth.

use crate::{Matrix, Range};

/// Luma coefficients `(Kr, Kb)` for a matrix; `Kg = 1 - Kr - Kb`.
pub fn luma_coefficients(matrix: Matrix) -> Option<(f64, f64)> {
    match matrix {
        Matrix::Bt709 => Some((0.2126, 0.0722)),
        Matrix::Bt601 => Some((0.299, 0.114)),
        Matrix::Bt2020Ncl => Some((0.2627, 0.0593)),
        Matrix::Identity => None,
    }
}

/// Converts normalized R'G'B' to normalized Y'CbCr.
pub fn rgb_to_ycbcr(matrix: Matrix, [r, g, b]: [f64; 3]) -> [f64; 3] {
    let Some((kr, kb)) = luma_coefficients(matrix) else {
        return [r, g, b];
    };
    let kg = 1.0 - kr - kb;
    let y = kr * r + kg * g + kb * b;
    let cb = (b - y) / (2.0 * (1.0 - kb));
    let cr = (r - y) / (2.0 * (1.0 - kr));
    [y, cb, cr]
}

/// Converts normalized Y'CbCr to normalized R'G'B'.
pub fn ycbcr_to_rgb(matrix: Matrix, [y, cb, cr]: [f64; 3]) -> [f64; 3] {
    let Some((kr, kb)) = luma_coefficients(matrix) else {
        return [y, cb, cr];
    };
    let kg = 1.0 - kr - kb;
    let r = y + 2.0 * (1.0 - kr) * cr;
    let b = y + 2.0 * (1.0 - kb) * cb;
    let g = (y - kr * r - kb * b) / kg;
    [r, g, b]
}

/// Normalizes integer Y'CbCr codes at `bits` depth to `[0, 1]` / `[-0.5, 0.5]`.
///
/// Limited range maps luma codes `16..=235` (scaled by `2^(bits-8)`) to
/// `[0, 1]` and chroma codes `16..=240` to `[-0.5, 0.5]`; values outside the
/// nominal range are preserved proportionally rather than clipped.
pub fn decode_ycbcr(range: Range, bits: u32, [y, cb, cr]: [f64; 3]) -> [f64; 3] {
    let scale = f64::from(1u32 << (bits - 8));
    let max = f64::from((1u32 << bits) - 1);
    let half = f64::from(1u32 << (bits - 1));
    match range {
        Range::Full => [y / max, (cb - half) / max, (cr - half) / max],
        Range::Limited => [
            (y - 16.0 * scale) / (219.0 * scale),
            (cb - 128.0 * scale) / (224.0 * scale),
            (cr - 128.0 * scale) / (224.0 * scale),
        ],
    }
}

/// Inverse of [`decode_ycbcr`]; the result is not rounded or clipped.
pub fn encode_ycbcr(range: Range, bits: u32, [y, cb, cr]: [f64; 3]) -> [f64; 3] {
    let scale = f64::from(1u32 << (bits - 8));
    let max = f64::from((1u32 << bits) - 1);
    let half = f64::from(1u32 << (bits - 1));
    match range {
        Range::Full => [y * max, cb * max + half, cr * max + half],
        Range::Limited => [
            y * 219.0 * scale + 16.0 * scale,
            cb * 224.0 * scale + 128.0 * scale,
            cr * 224.0 * scale + 128.0 * scale,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: [f64; 3], b: [f64; 3], eps: f64) {
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y).abs() < eps, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn white_and_black_have_no_chroma() {
        for m in [Matrix::Bt709, Matrix::Bt601, Matrix::Bt2020Ncl] {
            assert_close(rgb_to_ycbcr(m, [1.0, 1.0, 1.0]), [1.0, 0.0, 0.0], 1e-12);
            assert_close(rgb_to_ycbcr(m, [0.0, 0.0, 0.0]), [0.0, 0.0, 0.0], 1e-12);
        }
    }

    #[test]
    fn primaries_hit_the_chroma_extremes() {
        let [y, cb, cr] = rgb_to_ycbcr(Matrix::Bt709, [1.0, 0.0, 0.0]);
        assert!((y - 0.2126).abs() < 1e-12);
        assert!((cr - 0.5).abs() < 1e-12);
        assert!(cb < 0.0);
        let [_, cb, _] = rgb_to_ycbcr(Matrix::Bt601, [0.0, 0.0, 1.0]);
        assert!((cb - 0.5).abs() < 1e-12);
    }

    #[test]
    fn round_trips_every_matrix() {
        let samples = [
            [0.2, 0.7, 0.1],
            [0.9, 0.9, 0.1],
            [0.05, 0.05, 0.4],
            [1.0, 0.0, 1.0],
        ];
        for m in [
            Matrix::Bt709,
            Matrix::Bt601,
            Matrix::Bt2020Ncl,
            Matrix::Identity,
        ] {
            for rgb in samples {
                assert_close(ycbcr_to_rgb(m, rgb_to_ycbcr(m, rgb)), rgb, 1e-12);
            }
        }
    }

    #[test]
    fn limited_range_8bit_anchors() {
        assert_close(
            decode_ycbcr(Range::Limited, 8, [16.0, 128.0, 128.0]),
            [0.0, 0.0, 0.0],
            1e-12,
        );
        assert_close(
            decode_ycbcr(Range::Limited, 8, [235.0, 16.0, 240.0]),
            [1.0, -0.5, 0.5],
            1e-12,
        );
        assert_close(
            encode_ycbcr(Range::Limited, 8, [1.0, 0.5, -0.5]),
            [235.0, 240.0, 16.0],
            1e-12,
        );
    }

    #[test]
    fn limited_range_10bit_anchors() {
        assert_close(
            decode_ycbcr(Range::Limited, 10, [64.0, 512.0, 512.0]),
            [0.0, 0.0, 0.0],
            1e-12,
        );
        assert_close(
            decode_ycbcr(Range::Limited, 10, [940.0, 512.0, 512.0]),
            [1.0, 0.0, 0.0],
            1e-12,
        );
    }

    #[test]
    fn full_range_anchors() {
        assert_close(
            decode_ycbcr(Range::Full, 8, [255.0, 128.0, 128.0]),
            [1.0, 0.0, 0.0],
            1e-12,
        );
        assert_close(
            decode_ycbcr(Range::Full, 8, [0.0, 0.0, 255.0]),
            [0.0, -0.501_96, 0.498_04],
            1e-5,
        );
    }

    #[test]
    fn bt601_and_bt709_disagree_on_saturated_colors() {
        // The classic wrong-matrix symptom: identical codes decode differently.
        let codes = decode_ycbcr(Range::Limited, 8, [100.0, 90.0, 200.0]);
        let a = ycbcr_to_rgb(Matrix::Bt601, codes);
        let b = ycbcr_to_rgb(Matrix::Bt709, codes);
        assert!((a[0] - b[0]).abs() > 0.01 || (a[1] - b[1]).abs() > 0.01);
    }
}
