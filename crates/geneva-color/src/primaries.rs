//! Conversion between sets of RGB primaries, in linear light.
//!
//! Every set here shares the D65 white point, so a conversion is one
//! 3×3 matrix: RGB → XYZ with the source primaries, XYZ → RGB with the
//! destination's. The working space is BT.709; material with other
//! primaries is brought into it at decode, where wide-gamut colors can
//! land outside `[0, 1]` and are clipped by whoever encodes them.

use crate::Primaries;

/// A 3×3 matrix applied to a column vector.
pub type Mat3 = [[f64; 3]; 3];

/// CIE xy chromaticities of the red, green and blue primaries.
fn chromaticities(p: Primaries) -> [[f64; 2]; 3] {
    match p {
        Primaries::Bt709 => [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]],
        Primaries::Bt601_625 => [[0.640, 0.330], [0.290, 0.600], [0.150, 0.060]],
        Primaries::Bt601_525 => [[0.630, 0.340], [0.310, 0.595], [0.155, 0.070]],
        Primaries::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]],
    }
}

/// D65 white in xy.
const WHITE: [f64; 2] = [0.3127, 0.3290];

/// The matrix taking linear RGB in `p` to CIE XYZ.
pub fn rgb_to_xyz(p: Primaries) -> Mat3 {
    let c = chromaticities(p);
    // Each primary as XYZ with Y = 1, then scaled so the sum is the white.
    let col = |[x, y]: [f64; 2]| [x / y, 1.0, (1.0 - x - y) / y];
    let [r, g, b] = [col(c[0]), col(c[1]), col(c[2])];
    let w = col(WHITE);
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let s = mul(&invert(&m), w);
    [
        [r[0] * s[0], g[0] * s[1], b[0] * s[2]],
        [r[1] * s[0], g[1] * s[1], b[1] * s[2]],
        [r[2] * s[0], g[2] * s[1], b[2] * s[2]],
    ]
}

/// The matrix taking linear RGB in `from` to linear RGB in `to`; `None`
/// when they are the same set.
pub fn conversion(from: Primaries, to: Primaries) -> Option<Mat3> {
    if from == to {
        return None;
    }
    Some(matmul(&invert(&rgb_to_xyz(to)), &rgb_to_xyz(from)))
}

/// The luminance coefficients of linear RGB in `p`: the Y row of its
/// RGB → XYZ matrix.
pub fn luminance(p: Primaries) -> [f64; 3] {
    rgb_to_xyz(p)[1]
}

/// Applies a matrix to a vector.
pub fn mul(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn matmul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn invert(m: &Mat3) -> Mat3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let inv = 1.0 / det;
    [
        [
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
        ],
        [
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
        ],
        [
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3], eps: f64) {
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < eps, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn bt709_luminance_matches_the_published_coefficients() {
        close(luminance(Primaries::Bt709), [0.2126, 0.7152, 0.0722], 1e-4);
        close(luminance(Primaries::Bt2020), [0.2627, 0.6780, 0.0593], 1e-4);
    }

    #[test]
    fn white_is_white_in_every_set() {
        for p in [
            Primaries::Bt709,
            Primaries::Bt601_625,
            Primaries::Bt601_525,
            Primaries::Bt2020,
        ] {
            let xyz = mul(&rgb_to_xyz(p), [1.0, 1.0, 1.0]);
            close(xyz, [0.9505, 1.0, 1.0891], 1e-3);
            for q in [Primaries::Bt709, Primaries::Bt2020] {
                if let Some(m) = conversion(p, q) {
                    close(mul(&m, [1.0, 1.0, 1.0]), [1.0, 1.0, 1.0], 1e-9);
                }
            }
        }
    }

    #[test]
    fn bt2020_to_bt709_matches_the_bt2087_matrix() {
        let m = conversion(Primaries::Bt2020, Primaries::Bt709).unwrap();
        close(m[0], [1.6605, -0.5876, -0.0728], 1e-3);
        close(m[1], [-0.1246, 1.1329, -0.0083], 1e-3);
        close(m[2], [-0.0182, -0.1006, 1.1187], 1e-3);
        // BT.2020 red is outside BT.709: it comes back with negative green and blue.
        let red = mul(&m, [1.0, 0.0, 0.0]);
        assert!(red[0] > 1.0 && red[1] < 0.0 && red[2] < 0.0, "{red:?}");
    }

    #[test]
    fn same_primaries_need_no_conversion() {
        assert!(conversion(Primaries::Bt709, Primaries::Bt709).is_none());
    }
}
