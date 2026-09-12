//! HDR material into the SDR working space: PQ and HLG signals to linear
//! BT.709 light with the highlights rolled off, not clipped.
//!
//! The pipeline per pixel:
//!
//! 1. The signal to light. PQ is display-referred: absolute nits, taken
//!    relative to reference white ([`REFERENCE_WHITE_NITS`]). HLG is
//!    scene-referred: its OOTF (system gamma 1.2 on luminance, for a
//!    1000-nit display) makes it display light first.
//! 2. A PQ source mastered above 1000 nits (per its metadata) is first
//!    brought down to 1000 with the BT.2390 EETF on luminance, so that
//!    the next step sees the range it was designed for.
//! 3. Tone mapping: ITU-R BT.2446 method A, the conversion specified for
//!    1000-nit HDR to SDR. Luminance is encoded with a gamma of 2.4,
//!    compressed through a log curve and a three-piece knee, and decoded
//!    for a 100-nit display; the chroma follows with the same scale, a
//!    touch reduced (the 1.1 factor), and a small luminance correction
//!    for saturated reds. Reference white (203 nits) lands at 0.41 of
//!    SDR white in linear light, 1000 nits on SDR white.
//! 4. Primaries: the source's (BT.2020, as a rule) to BT.709.
//! 5. What still leaves the SDR cube is pulled toward its own luminance
//!    until it fits, so a bright saturated highlight desaturates rather
//!    than shifting hue.

use crate::primaries;
use crate::transfer::{hlg_scene_linear, linear_to_pq, pq_to_linear};
use crate::{Primaries, ResolvedTags, Transfer};

/// HDR reference white: the luminance of diffuse white and of graphics
/// (ITU-R BT.2408), and what `1.0` means in the working space for HDR
/// material.
pub const REFERENCE_WHITE_NITS: f64 = 203.0;

/// The source peak assumed when a file carries no mastering metadata.
pub const DEFAULT_PEAK_NITS: f64 = 1000.0;

/// Nominal peak of an HLG display, which sets its OOTF.
const HLG_DISPLAY_NITS: f64 = 1000.0;

/// Entries of the tone curve tables.
const CURVE_SIZE: usize = 4096;

/// The luminance BT.2446 method A is specified for, in nits.
const BT2446_HDR_NITS: f64 = 1000.0;

/// The SDR display BT.2446 method A targets, in nits.
const BT2446_SDR_NITS: f64 = 100.0;

/// The gamma the method encodes luminance and channels with.
const BT2446_GAMMA: f64 = 2.4;

/// The conversion for one source: its transfer, primaries and peak.
#[derive(Debug, Clone)]
pub struct HdrToSdr {
    hlg: bool,
    /// Non-linear code (16-bit) to light: PQ in units of reference
    /// white, HLG as scene light in `[0, 1]`.
    to_light: Box<[f32]>,
    /// Luminance coefficients of the source primaries.
    luma: [f32; 3],
    /// For a source brighter than 1000 nits: its peak in units of
    /// reference white, and `eetf(y) / y` over `(y / peak)^(1/4)`,
    /// bringing it down to 1000 nits.
    reduce: Option<(f32, Box<[f32]>)>,
    /// `v^(1/2.4)` over `v^(1/4)`, for `v` in `[0, 1]`.
    encode: Box<[f32]>,
    /// `v^2.4` over `v` in `[0, 1]`.
    decode: Box<[f32]>,
    /// The method's luminance curve: encoded SDR luminance over encoded
    /// HDR luminance, both in `[0, 1]`.
    curve: Box<[f32]>,
    /// Source primaries to BT.709, when they differ.
    to_bt709: Option<[[f32; 3]; 3]>,
}

/// BT.2446 method A on encoded luminance: `yp` is `(Y / 1000 nits)^(1/2.4)`
/// and the result is `(Y_sdr / 100 nits)^(1/2.4)`.
fn bt2446a_luminance(yp: f64) -> f64 {
    let rho = 1.0 + 32.0 * (BT2446_HDR_NITS / 10_000.0).powf(1.0 / BT2446_GAMMA);
    let rho_sdr = 1.0 + 32.0 * (BT2446_SDR_NITS / 10_000.0).powf(1.0 / BT2446_GAMMA);
    let yp = yp.clamp(0.0, 1.0);
    let ypp = (1.0 + (rho - 1.0) * yp).ln() / rho.ln();
    let yc = if ypp <= 0.7399 {
        1.0770 * ypp
    } else if ypp < 0.9909 {
        -1.1510 * ypp * ypp + 2.7811 * ypp - 0.6302
    } else {
        0.5 * ypp + 0.5
    };
    ((rho_sdr.powf(yc) - 1.0) / (rho_sdr - 1.0)).clamp(0.0, 1.0)
}

impl HdrToSdr {
    /// The conversion for a source with `tags` and, when its metadata
    /// gives one, a peak luminance in nits. `None` for SDR tags.
    pub fn new(tags: ResolvedTags, peak_nits: Option<f64>) -> Option<Self> {
        let hlg = match tags.transfer {
            Transfer::Pq => false,
            Transfer::Hlg => true,
            _ => return None,
        };
        let peak_nits = if hlg {
            HLG_DISPLAY_NITS
        } else {
            peak_nits
                .filter(|p| *p > REFERENCE_WHITE_NITS)
                .unwrap_or(DEFAULT_PEAK_NITS)
        };
        let to_light: Vec<f32> = (0..=u16::MAX)
            .map(|code| {
                let v = f64::from(code) / f64::from(u16::MAX);
                let light = if hlg {
                    hlg_scene_linear(v)
                } else {
                    pq_to_linear(v)
                };
                light as f32
            })
            .collect();
        let reduce = (peak_nits > BT2446_HDR_NITS).then(|| {
            let peak = peak_nits / REFERENCE_WHITE_NITS;
            let eetf = Eetf::new(peak_nits, BT2446_HDR_NITS);
            let curve: Vec<f32> = (0..CURVE_SIZE)
                .map(|i| {
                    let t = i as f64 / (CURVE_SIZE - 1) as f64;
                    let y = t.powi(4) * peak;
                    if y <= 0.0 {
                        1.0
                    } else {
                        (eetf.map(y * REFERENCE_WHITE_NITS) / REFERENCE_WHITE_NITS / y) as f32
                    }
                })
                .collect();
            (peak as f32, curve.into_boxed_slice())
        });
        let steps = |f: fn(f64) -> f64| -> Box<[f32]> {
            (0..CURVE_SIZE)
                .map(|i| f(i as f64 / (CURVE_SIZE - 1) as f64) as f32)
                .collect()
        };
        let encode = steps(|t| t.powi(4).powf(1.0 / BT2446_GAMMA));
        let decode = steps(|v| v.powf(BT2446_GAMMA));
        let curve = steps(bt2446a_luminance);
        let luma = primaries::luminance(tags.primaries).map(|v| v as f32);
        let to_bt709 = primaries::conversion(tags.primaries, Primaries::Bt709)
            .map(|m| m.map(|row| row.map(|v| v as f32)));
        Some(Self {
            hlg,
            to_light: to_light.into_boxed_slice(),
            luma,
            reduce,
            encode,
            decode,
            curve,
            to_bt709,
        })
    }

    /// Converts one pixel from non-linear source codes in `[0, 1]` to
    /// linear BT.709 light in `[0, 1]`.
    #[inline]
    pub fn convert(&self, rgb: [f32; 3]) -> [f32; 3] {
        let light = |v: f32| {
            let i = (v * f32::from(u16::MAX) + 0.5) as i32;
            self.to_light[i.clamp(0, i32::from(u16::MAX)) as u16 as usize]
        };
        let mut rgb = [light(rgb[0]), light(rgb[1]), light(rgb[2])];
        let dot = |c: [f32; 3]| self.luma[0] * c[0] + self.luma[1] * c[1] + self.luma[2] * c[2];
        if self.hlg {
            // The OOTF: display light is scene light times the scene
            // luminance to the power of gamma minus one, at the display's
            // peak; then in units of reference white.
            let ys = dot(rgb).max(0.0);
            let gain = ys.powf(0.2) * (HLG_DISPLAY_NITS / REFERENCE_WHITE_NITS) as f32;
            rgb = rgb.map(|c| c * gain);
        }
        if let Some((peak, curve)) = &self.reduce {
            let y = dot(rgb).max(0.0);
            let t = (y / peak).clamp(0.0, 1.0).sqrt().sqrt();
            let i = (t * (CURVE_SIZE - 1) as f32 + 0.5) as usize;
            let ratio = curve[i.min(CURVE_SIZE - 1)];
            rgb = rgb.map(|c| c * ratio);
        }
        // BT.2446 method A, in units of its 1000-nit source: encoded
        // luminance through the curve, the chroma scaled along.
        let last = (CURVE_SIZE - 1) as f32;
        let encode = |v: f32| {
            let t = v.clamp(0.0, 1.0).sqrt().sqrt();
            self.encode[(t * last + 0.5) as usize]
        };
        let lookup = |table: &[f32], v: f32| table[(v.clamp(0.0, 1.0) * last + 0.5) as usize];
        let scale = (REFERENCE_WHITE_NITS / BT2446_HDR_NITS) as f32;
        let n = rgb.map(|c| (c * scale).max(0.0));
        let yp = encode(dot(n));
        let ys = lookup(&self.curve, yp);
        let s = if yp > 1e-4 { ys / (1.1 * yp) } else { 0.0 };
        let d = n.map(|c| encode(c) - yp);
        let cr = s * d[0] / 1.4746;
        let yt = ys - (0.1 * cr).max(0.0);
        let mut rgb = d.map(|dv| lookup(&self.decode, yt + s * dv));
        let y = dot(rgb).max(0.0);
        if let Some(m) = &self.to_bt709 {
            rgb = [
                m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
                m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
                m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
            ];
        }
        // Into the cube: toward the pixel's own luminance, no further
        // than needed.
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        let y = y.clamp(0.0, 1.0);
        let mut t = 0.0f32;
        if max > 1.0 && max > y {
            t = t.max((max - 1.0) / (max - y));
        }
        if min < 0.0 && y > min {
            t = t.max(-min / (y - min));
        }
        if t > 0.0 {
            let t = t.min(1.0);
            rgb = rgb.map(|c| c + (y - c) * t);
        }
        rgb.map(|c| c.clamp(0.0, 1.0))
    }
}

/// The BT.2390 EETF from a source range up to `source_peak` nits onto a
/// display up to `target_peak` nits, both starting at black.
#[derive(Debug, Clone, Copy)]
pub struct Eetf {
    source_peak_pq: f64,
    max_lum: f64,
    ks: f64,
}

impl Eetf {
    /// The curve for the two peaks in nits.
    pub fn new(source_peak: f64, target_peak: f64) -> Self {
        let source_peak_pq = linear_to_pq(source_peak / REFERENCE_WHITE_NITS);
        let target_pq = linear_to_pq(target_peak / REFERENCE_WHITE_NITS);
        let max_lum = (target_pq / source_peak_pq).min(1.0);
        Self {
            source_peak_pq,
            max_lum,
            ks: 1.5 * max_lum - 0.5,
        }
    }

    /// Maps a luminance in nits to the display's.
    pub fn map(&self, nits: f64) -> f64 {
        if self.max_lum >= 1.0 {
            return nits.min(self.source_peak_pq_nits());
        }
        let e1 = linear_to_pq(nits / REFERENCE_WHITE_NITS) / self.source_peak_pq;
        let e2 = if e1 < self.ks {
            e1
        } else {
            let t = (e1 - self.ks) / (1.0 - self.ks);
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * self.ks
                + (t3 - 2.0 * t2 + t) * (1.0 - self.ks)
                + (-2.0 * t3 + 3.0 * t2) * self.max_lum
        };
        pq_to_linear(e2 * self.source_peak_pq) * REFERENCE_WHITE_NITS
    }

    fn source_peak_pq_nits(&self) -> f64 {
        pq_to_linear(self.source_peak_pq) * REFERENCE_WHITE_NITS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Matrix, Range};

    const PQ_2020: ResolvedTags = ResolvedTags {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
    };

    const HLG_2020: ResolvedTags = ResolvedTags {
        transfer: Transfer::Hlg,
        ..PQ_2020
    };

    fn close(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() < eps, "{a} != {b} (eps {eps})");
    }

    #[test]
    fn eetf_keeps_the_ends_and_rolls_off_between() {
        let e = Eetf::new(1000.0, 203.0);
        close(e.map(0.0) as f32, 0.0, 1e-6);
        // The source peak lands on the display peak.
        close(e.map(1000.0) as f32, 203.0, 0.5);
        // Below the knee nothing changes.
        close(e.map(50.0) as f32, 50.0, 1e-3);
        // Above it, compressed but monotonic.
        let mut last = 0.0;
        for nits in (0..=1000).step_by(10) {
            let out = e.map(f64::from(nits));
            assert!(out >= last, "not monotonic at {nits}");
            assert!(out <= 203.0 + 1e-6);
            last = out;
        }
        // Reference white of a 1000-nit source comes out at about 78% of
        // SDR white, as the curve prescribes.
        close((e.map(203.0) / 203.0) as f32, 0.78, 0.02);
    }

    #[test]
    fn pq_white_and_black_land_where_expected() {
        let conv = HdrToSdr::new(PQ_2020, None).unwrap();
        let code = |nits: f64| linear_to_pq(nits / REFERENCE_WHITE_NITS) as f32;
        let gray = |nits: f64| conv.convert([code(nits); 3]);
        close(gray(0.0)[0], 0.0, 1e-6);
        let peak = gray(1000.0);
        close(peak[0], 1.0, 0.01);
        close(peak[1], 1.0, 0.01);
        // BT.2446 method A: reference white at 0.41 of SDR white,
        // 20 nits at 0.058.
        let white = gray(203.0);
        close(white[0], 0.406, 0.01);
        close(white[1], white[0], 1e-3);
        let low = gray(20.3);
        close(low[0], 0.058, 0.005);
        // Beyond the peak nothing exceeds SDR white.
        assert!(gray(4000.0).iter().all(|&c| c <= 1.0));
    }

    #[test]
    fn a_higher_source_peak_from_metadata_compresses_more() {
        let default = HdrToSdr::new(PQ_2020, None).unwrap();
        let bright = HdrToSdr::new(PQ_2020, Some(4000.0)).unwrap();
        // 900 nits: above the knee of the 4000-to-1000 reduction, so the
        // brighter source's picture is compressed there.
        let code = linear_to_pq(900.0 / REFERENCE_WHITE_NITS) as f32;
        assert!(bright.convert([code; 3])[0] < default.convert([code; 3])[0]);
    }

    #[test]
    fn hlg_reference_white_matches_pq_reference_white() {
        // HLG at 75% signal is 203 nits on a 1000-nit display (BT.2408).
        let hlg = HdrToSdr::new(HLG_2020, None).unwrap();
        let pq = HdrToSdr::new(PQ_2020, None).unwrap();
        let from_hlg = hlg.convert([0.75; 3]);
        let from_pq = pq.convert([linear_to_pq(203.0 / REFERENCE_WHITE_NITS) as f32; 3]);
        close(from_hlg[0], from_pq[0], 0.02);
        close(hlg.convert([1.0; 3])[0], 1.0, 0.01);
    }

    #[test]
    fn bt2020_red_lands_inside_the_bt709_cube_with_its_hue() {
        let conv = HdrToSdr::new(PQ_2020, None).unwrap();
        let code = linear_to_pq(100.0 / REFERENCE_WHITE_NITS) as f32;
        let red = conv.convert([code, 0.0, 0.0]);
        assert!(red.iter().all(|&c| (0.0..=1.0).contains(&c)), "{red:?}");
        assert!(red[0] > red[1] && red[0] > red[2], "{red:?}");
        // A 100-nit red is a dim red in SDR, but a red.
        assert!(red[0] > 0.25, "{red:?}");
    }

    #[test]
    fn sdr_tags_have_no_conversion() {
        assert!(HdrToSdr::new(ResolvedTags::SDR_VIDEO, None).is_none());
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;
    use crate::{Matrix, Range};

    #[test]
    fn the_luminance_curve_matches_the_recommendation() {
        // Values worked out from the method's equations for a 1000-nit
        // source and a 100-nit display, in linear light.
        for (nits, want) in [
            (10.0, 0.0313),
            (50.0, 0.1265),
            (100.0, 0.2266),
            (203.0, 0.406),
            (500.0, 0.7315),
            (1000.0, 1.0),
        ] {
            let yp = (nits / BT2446_HDR_NITS).powf(1.0 / BT2446_GAMMA);
            let got = bt2446a_luminance(yp).powf(BT2446_GAMMA);
            assert!(
                (got - want).abs() < 0.002,
                "{nits} nits: {got} wanted {want}"
            );
        }
    }

    #[test]
    fn a_bt709_green_at_reference_white_survives_the_round_trip() {
        let tags = ResolvedTags {
            primaries: Primaries::Bt2020,
            transfer: Transfer::Pq,
            matrix: Matrix::Bt2020Ncl,
            range: Range::Limited,
        };
        let conv = HdrToSdr::new(tags, None).unwrap();
        let to_2020 = primaries::conversion(Primaries::Bt709, Primaries::Bt2020).unwrap();
        for (name, rgb709) in [
            ("green", [0.0, 1.0, 0.0]),
            ("red", [1.0, 0.0, 0.0]),
            ("gray", [0.5, 0.5, 0.5]),
        ] {
            let in_2020 = primaries::mul(&to_2020, rgb709);
            let codes = in_2020.map(|c| linear_to_pq(c.max(0.0)) as f32);
            let out = conv.convert(codes);
            // Inside the cube, and the hue of the BT.709 color it was.
            assert!(
                out.iter().all(|v| (0.0..=1.0).contains(v)),
                "{name}: {out:?}"
            );
            let top = (0..3)
                .max_by(|a, b| rgb709[*a].total_cmp(&rgb709[*b]))
                .unwrap();
            for k in 0..3 {
                if k != top {
                    assert!(
                        rgb709[k] == rgb709[top] || out[k] < out[top] - 0.05,
                        "{name}: {out:?}"
                    );
                }
            }
            if name == "gray" {
                // Half of reference white is about 100 nits: 0.23 of SDR white.
                assert!((out[0] - 0.2266).abs() < 0.01, "{name}: {out:?}");
            }
        }
    }
}
