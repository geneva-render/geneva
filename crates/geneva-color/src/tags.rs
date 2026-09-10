use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Chromaticity of the red, green and blue primaries and the white point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Primaries {
    /// ITU-R BT.709, shared by sRGB. The default for HD material.
    Bt709,
    /// ITU-R BT.601 as used with 625-line systems (BT.470 B/G, PAL).
    Bt601_625,
    /// ITU-R BT.601 as used with 525-line systems (SMPTE 170M, NTSC).
    Bt601_525,
    /// ITU-R BT.2020, used by HDR and wide-gamut UHD material.
    Bt2020,
}

/// The opto-electronic transfer characteristic of encoded samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Transfer {
    /// The piecewise sRGB curve (IEC 61966-2-1). The default for still images
    /// and graphics.
    Srgb,
    /// The ITU-R BT.709 curve, shared by BT.601 and BT.2020 SDR. The default
    /// for SDR video.
    Bt709,
    /// Linear light; samples are proportional to radiance.
    Linear,
    /// SMPTE ST 2084 perceptual quantizer (HDR10).
    Pq,
    /// ARIB STD-B67 hybrid log-gamma.
    Hlg,
}

/// The matrix used to derive Y'CbCr from R'G'B'.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Matrix {
    /// ITU-R BT.709 coefficients. The default for HD material.
    Bt709,
    /// ITU-R BT.601 coefficients. The default for SD material.
    Bt601,
    /// ITU-R BT.2020 non-constant luminance.
    Bt2020Ncl,
    /// No matrix: samples are R'G'B'.
    Identity,
}

/// Whether coded samples use the full code range or the video (limited) range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Range {
    /// Y' in 16..=235 and Cb/Cr in 16..=240 at 8 bits, scaled with bit depth.
    Limited,
    /// All codes from 0 to the maximum are used.
    Full,
}

/// Color metadata as reported by a decoder or written in a timeline.
///
/// Each field is optional: `None` means "not tagged", which is exactly the
/// situation [`crate::infer`] resolves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColorTags {
    /// Primaries, if tagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primaries: Option<Primaries>,
    /// Transfer characteristic, if tagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<Transfer>,
    /// Y'CbCr matrix, if tagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<Matrix>,
    /// Sample range, if tagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

/// Fully resolved color metadata with no unknowns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedTags {
    /// Primaries.
    pub primaries: Primaries,
    /// Transfer characteristic.
    pub transfer: Transfer,
    /// Y'CbCr matrix.
    pub matrix: Matrix,
    /// Sample range.
    pub range: Range,
}

impl ResolvedTags {
    /// Standard-dynamic-range HD video: BT.709 throughout, limited range.
    pub const SDR_VIDEO: Self = Self {
        primaries: Primaries::Bt709,
        transfer: Transfer::Bt709,
        matrix: Matrix::Bt709,
        range: Range::Limited,
    };

    /// Full-range sRGB, as used by still images and rendered graphics.
    pub const SRGB: Self = Self {
        primaries: Primaries::Bt709,
        transfer: Transfer::Srgb,
        matrix: Matrix::Identity,
        range: Range::Full,
    };

    /// Returns true for transfer functions whose peak exceeds SDR white.
    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer, Transfer::Pq | Transfer::Hlg)
    }
}

impl From<ResolvedTags> for ColorTags {
    fn from(r: ResolvedTags) -> Self {
        Self {
            primaries: Some(r.primaries),
            transfer: Some(r.transfer),
            matrix: Some(r.matrix),
            range: Some(r.range),
        }
    }
}
