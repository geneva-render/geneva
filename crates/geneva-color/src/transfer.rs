//! Transfer functions: conversions between encoded signal and linear light.
//!
//! All functions take and return values nominally in `[0, 1]`, where 1 is
//! SDR reference white. PQ and HLG return linear values relative to SDR white
//! so that HDR material can be tone-mapped rather than clipped; their peaks
//! exceed 1.

use crate::Transfer;

/// Peak luminance of the PQ curve relative to SDR reference white (100 nits).
const PQ_PEAK_RELATIVE: f64 = 10_000.0 / 100.0;

/// Nominal HLG peak relative to SDR white (1000 nits).
const HLG_PEAK_RELATIVE: f64 = 1000.0 / 100.0;

impl Transfer {
    /// Converts an encoded value to linear light.
    pub fn to_linear(self, v: f64) -> f64 {
        match self {
            Self::Srgb => srgb_to_linear(v),
            Self::Bt709 => bt709_to_linear(v),
            Self::Linear => v,
            Self::Pq => pq_to_linear(v),
            Self::Hlg => hlg_to_linear(v),
        }
    }

    /// Converts linear light to an encoded value.
    pub fn from_linear(self, l: f64) -> f64 {
        match self {
            Self::Srgb => linear_to_srgb(l),
            Self::Bt709 => linear_to_bt709(l),
            Self::Linear => l,
            Self::Pq => linear_to_pq(l),
            Self::Hlg => linear_to_hlg(l),
        }
    }
}

/// sRGB electro-optical transfer function (IEC 61966-2-1).
pub fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Inverse of [`srgb_to_linear`].
pub fn linear_to_srgb(l: f64) -> f64 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear-domain knee of the BT.709 curve, using the precise constants
/// published with BT.2020 so that both branches meet exactly.
const BT709_BETA: f64 = 0.018_053_968_510_807;
const BT709_ALPHA: f64 = 1.099_296_826_809_44;

/// Inverse of the BT.709 opto-electronic transfer function.
///
/// This is the scene-referred inverse OETF, which keeps sRGB graphics and
/// BT.709 video visually consistent when combined. Display-referred decoding
/// (BT.1886) is a distinct, steeper curve and is not what this returns.
pub fn bt709_to_linear(v: f64) -> f64 {
    if v < 4.5 * BT709_BETA {
        v / 4.5
    } else {
        ((v + (BT709_ALPHA - 1.0)) / BT709_ALPHA).powf(1.0 / 0.45)
    }
}

/// BT.709 opto-electronic transfer function.
pub fn linear_to_bt709(l: f64) -> f64 {
    if l < BT709_BETA {
        l * 4.5
    } else {
        BT709_ALPHA * l.powf(0.45) - (BT709_ALPHA - 1.0)
    }
}

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

/// SMPTE ST 2084 EOTF, returning linear light relative to SDR white.
pub fn pq_to_linear(v: f64) -> f64 {
    let v = v.max(0.0);
    let p = v.powf(1.0 / PQ_M2);
    let num = (p - PQ_C1).max(0.0);
    let den = PQ_C2 - PQ_C3 * p;
    (num / den).powf(1.0 / PQ_M1) * PQ_PEAK_RELATIVE
}

/// Inverse of [`pq_to_linear`].
pub fn linear_to_pq(l: f64) -> f64 {
    let y = (l / PQ_PEAK_RELATIVE).clamp(0.0, 1.0);
    let ym = y.powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * ym) / (1.0 + PQ_C3 * ym)).powf(PQ_M2)
}

const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 0.284_668_92;
const HLG_C: f64 = 0.559_910_73;

/// ARIB STD-B67 inverse OETF, returning scene light relative to SDR white.
///
/// The system gamma of the HLG OOTF is not applied; the value is scene
/// linear scaled so that the nominal peak maps to 10× SDR white.
pub fn hlg_to_linear(v: f64) -> f64 {
    let v = v.max(0.0);
    let scene = if v <= 0.5 {
        v * v / 3.0
    } else {
        (((v - HLG_C) / HLG_A).exp() + HLG_B) / 12.0
    };
    scene * HLG_PEAK_RELATIVE
}

/// Inverse of [`hlg_to_linear`].
pub fn linear_to_hlg(l: f64) -> f64 {
    let e = (l / HLG_PEAK_RELATIVE).clamp(0.0, 1.0);
    if e <= 1.0 / 12.0 {
        (3.0 * e).sqrt()
    } else {
        HLG_A * (12.0 * e - HLG_B).ln() + HLG_C
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(a: f64, b: f64, eps: f64) {
        assert!((a - b).abs() < eps, "{a} != {b} (eps {eps})");
    }

    #[test]
    fn srgb_reference_points() {
        assert_close(srgb_to_linear(0.0), 0.0, 1e-12);
        assert_close(srgb_to_linear(1.0), 1.0, 1e-12);
        assert_close(srgb_to_linear(0.5), 0.214_04, 1e-5);
        assert_close(srgb_to_linear(128.0 / 255.0), 0.215_86, 1e-5);
        assert_close(linear_to_srgb(0.5), 0.735_36, 1e-5);
    }

    #[test]
    fn bt709_reference_points() {
        assert_close(bt709_to_linear(0.0), 0.0, 1e-12);
        assert_close(bt709_to_linear(1.0), 1.0, 1e-6);
        assert_close(linear_to_bt709(BT709_BETA), 4.5 * BT709_BETA, 1e-12);
        // The two branches meet at the knee without a discontinuity.
        let below = bt709_to_linear(4.5 * BT709_BETA - 1e-9);
        let above = bt709_to_linear(4.5 * BT709_BETA + 1e-9);
        assert_close(below, above, 1e-8);
        assert_close(bt709_to_linear(0.5), 0.259_6, 1e-3);
    }

    #[test]
    fn pq_reference_points() {
        // PQ code 0.5 corresponds to roughly 92 nits.
        assert_close(pq_to_linear(0.5) * 100.0, 92.24, 0.1);
        assert_close(pq_to_linear(1.0), PQ_PEAK_RELATIVE, 1e-6);
        assert_close(pq_to_linear(0.0), 0.0, 1e-12);
    }

    #[test]
    fn hlg_reference_points() {
        assert_close(hlg_to_linear(0.5) / HLG_PEAK_RELATIVE, 1.0 / 12.0, 1e-9);
        assert_close(hlg_to_linear(1.0), HLG_PEAK_RELATIVE, 1e-6);
    }

    #[test]
    fn all_curves_round_trip() {
        for t in [
            Transfer::Srgb,
            Transfer::Bt709,
            Transfer::Linear,
            Transfer::Pq,
            Transfer::Hlg,
        ] {
            for i in 0..=100 {
                let v = f64::from(i) / 100.0;
                let back = t.from_linear(t.to_linear(v));
                assert_close(back, v, 1e-6);
            }
        }
    }
}
