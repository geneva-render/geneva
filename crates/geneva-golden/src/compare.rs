use serde::{Deserialize, Serialize};

/// An 8-bit straight-alpha sRGB image, the form reference frames are stored
/// in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA bytes, row-major.
    pub data: Vec<u8>,
}

impl Rgba8Image {
    /// Wraps raw bytes; panics if the length does not match the size.
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len(),
            width as usize * height as usize * 4,
            "buffer size mismatch"
        );
        Self {
            width,
            height,
            data,
        }
    }

    /// Decodes a PNG.
    pub fn from_png(bytes: &[u8]) -> Result<Self, image::ImageError> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.to_rgba8();
        Ok(Self {
            width: img.width(),
            height: img.height(),
            data: img.into_raw(),
        })
    }

    /// Encodes as PNG.
    pub fn to_png(&self) -> Result<Vec<u8>, image::ImageError> {
        let img = image::RgbaImage::from_raw(self.width, self.height, self.data.clone())
            .expect("size checked");
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)?;
        Ok(out.into_inner())
    }

    /// Luma (Rec. 601 weights on the encoded values) as `f64` per pixel.
    fn luma(&self) -> Vec<f64> {
        self.data
            .chunks_exact(4)
            .map(|p| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]))
            .collect()
    }
}

/// What counts as a match.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tolerance {
    /// Per-channel difference (0..=255) at or below which a pixel is
    /// considered equal.
    pub channel_epsilon: u8,
    /// Largest fraction of pixels allowed to exceed `channel_epsilon`.
    pub max_mismatch_fraction: f64,
    /// Smallest acceptable structural similarity (0..=1).
    pub min_ssim: f64,
    /// Smallest acceptable peak signal-to-noise ratio in dB, if enforced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_psnr: Option<f64>,
}

impl Default for Tolerance {
    /// Tolerates rounding-level differences and a sliver of edge pixels,
    /// which is what cross-vendor GPU output looks like for the same scene.
    fn default() -> Self {
        Self {
            channel_epsilon: 2,
            max_mismatch_fraction: 0.002,
            min_ssim: 0.995,
            min_psnr: None,
        }
    }
}

impl Tolerance {
    /// A tolerance that only accepts identical images.
    pub const EXACT: Self = Self {
        channel_epsilon: 0,
        max_mismatch_fraction: 0.0,
        min_ssim: 1.0,
        min_psnr: None,
    };
}

/// The measured difference between two images.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    /// Whether the sizes matched; nothing else is meaningful when false.
    pub same_size: bool,
    /// Largest absolute difference per channel (R, G, B, A).
    pub max_channel_diff: [u8; 4],
    /// Number of pixels with any channel above the epsilon.
    pub mismatched_pixels: usize,
    /// `mismatched_pixels` as a fraction of all pixels.
    pub mismatch_fraction: f64,
    /// Peak signal-to-noise ratio over RGB in dB; infinite for identical images.
    pub psnr: f64,
    /// Structural similarity of the luma channels.
    pub ssim: f64,
}

impl Comparison {
    /// Whether the difference is within `tol`.
    pub fn passes(&self, tol: &Tolerance) -> bool {
        self.same_size
            && self.mismatch_fraction <= tol.max_mismatch_fraction
            && self.ssim >= tol.min_ssim
            && tol.min_psnr.is_none_or(|p| self.psnr >= p)
    }

    /// A one-line summary for logs.
    pub fn summary(&self) -> String {
        if !self.same_size {
            return "size mismatch".to_owned();
        }
        format!(
            "max diff {:?}, {} mismatched ({:.4}%), psnr {:.2} dB, ssim {:.5}",
            self.max_channel_diff,
            self.mismatched_pixels,
            self.mismatch_fraction * 100.0,
            self.psnr,
            self.ssim
        )
    }
}

/// Compares `actual` against `expected` using `epsilon` as the per-channel
/// threshold for counting a pixel as mismatched.
pub fn compare(actual: &Rgba8Image, expected: &Rgba8Image, epsilon: u8) -> Comparison {
    if actual.width != expected.width || actual.height != expected.height {
        return Comparison {
            same_size: false,
            max_channel_diff: [255; 4],
            mismatched_pixels: 0,
            mismatch_fraction: 1.0,
            psnr: 0.0,
            ssim: 0.0,
        };
    }
    let mut max = [0u8; 4];
    let mut mismatched = 0usize;
    let mut sq_err = 0.0f64;
    for (a, e) in actual
        .data
        .chunks_exact(4)
        .zip(expected.data.chunks_exact(4))
    {
        let mut bad = false;
        for c in 0..4 {
            let d = a[c].abs_diff(e[c]);
            max[c] = max[c].max(d);
            if d > epsilon {
                bad = true;
            }
            if c < 3 {
                sq_err += f64::from(d) * f64::from(d);
            }
        }
        if bad {
            mismatched += 1;
        }
    }
    let pixels = actual.width as usize * actual.height as usize;
    let mse = sq_err / (pixels as f64 * 3.0);
    let psnr = if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    };
    let ssim = ssim(
        &actual.luma(),
        &expected.luma(),
        actual.width as usize,
        actual.height as usize,
    );
    Comparison {
        same_size: true,
        max_channel_diff: max,
        mismatched_pixels: mismatched,
        mismatch_fraction: mismatched as f64 / pixels.max(1) as f64,
        psnr,
        ssim,
    }
}

/// Produces an image highlighting where two same-sized images differ:
/// differences are amplified and drawn over a darkened copy of `expected`.
pub fn diff_image(actual: &Rgba8Image, expected: &Rgba8Image) -> Rgba8Image {
    let mut out = Vec::with_capacity(expected.data.len());
    for (a, e) in actual
        .data
        .chunks_exact(4)
        .zip(expected.data.chunks_exact(4))
    {
        let d = (0..3)
            .map(|c| a[c].abs_diff(e[c]))
            .max()
            .unwrap_or(0)
            .max(a[3].abs_diff(e[3]));
        if d == 0 {
            let dim = |v: u8| v / 4;
            out.extend_from_slice(&[dim(e[0]), dim(e[1]), dim(e[2]), 255]);
        } else {
            let heat = u16::from(d).saturating_mul(8).min(255) as u8;
            out.extend_from_slice(&[255, 255 - heat, 0, 255]);
        }
    }
    Rgba8Image::new(expected.width, expected.height, out)
}

/// Mean structural similarity with an 11×11 Gaussian window (σ = 1.5), the
/// configuration from the original SSIM paper.
fn ssim(a: &[f64], b: &[f64], width: usize, height: usize) -> f64 {
    const RADIUS: usize = 5;
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    if width < 2 * RADIUS + 1 || height < 2 * RADIUS + 1 {
        return ssim_global(a, b);
    }
    let kernel: Vec<f64> = {
        let sigma = 1.5f64;
        let raw: Vec<f64> = (0..=2 * RADIUS)
            .map(|i| {
                let x = i as f64 - RADIUS as f64;
                (-(x * x) / (2.0 * sigma * sigma)).exp()
            })
            .collect();
        let sum: f64 = raw.iter().sum();
        raw.into_iter().map(|v| v / sum).collect()
    };
    let blur = |src: &[f64]| -> Vec<f64> {
        let mut tmp = vec![0.0; width * height];
        for y in 0..height {
            for x in RADIUS..width - RADIUS {
                let mut acc = 0.0;
                for (k, w) in kernel.iter().enumerate() {
                    acc += w * src[y * width + x + k - RADIUS];
                }
                tmp[y * width + x] = acc;
            }
        }
        let mut out = vec![0.0; width * height];
        for y in RADIUS..height - RADIUS {
            for x in RADIUS..width - RADIUS {
                let mut acc = 0.0;
                for (k, w) in kernel.iter().enumerate() {
                    acc += w * tmp[(y + k - RADIUS) * width + x];
                }
                out[y * width + x] = acc;
            }
        }
        out
    };
    let aa: Vec<f64> = a.iter().map(|v| v * v).collect();
    let bb: Vec<f64> = b.iter().map(|v| v * v).collect();
    let ab: Vec<f64> = a.iter().zip(b).map(|(x, y)| x * y).collect();
    let mu_a = blur(a);
    let mu_b = blur(b);
    let s_aa = blur(&aa);
    let s_bb = blur(&bb);
    let s_ab = blur(&ab);
    let mut total = 0.0;
    let mut count = 0usize;
    for y in RADIUS..height - RADIUS {
        for x in RADIUS..width - RADIUS {
            let i = y * width + x;
            let var_a = s_aa[i] - mu_a[i] * mu_a[i];
            let var_b = s_bb[i] - mu_b[i] * mu_b[i];
            let cov = s_ab[i] - mu_a[i] * mu_b[i];
            let num = (2.0 * mu_a[i] * mu_b[i] + C1) * (2.0 * cov + C2);
            let den = (mu_a[i] * mu_a[i] + mu_b[i] * mu_b[i] + C1) * (var_a + var_b + C2);
            total += num / den;
            count += 1;
        }
    }
    if count == 0 {
        1.0
    } else {
        total / count as f64
    }
}

/// SSIM over the whole image as a single window, for images too small for
/// the Gaussian window.
fn ssim_global(a: &[f64], b: &[f64]) -> f64 {
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let n = a.len().max(1) as f64;
    let mu_a = a.iter().sum::<f64>() / n;
    let mu_b = b.iter().sum::<f64>() / n;
    let var_a = a.iter().map(|v| (v - mu_a).powi(2)).sum::<f64>() / n;
    let var_b = b.iter().map(|v| (v - mu_b).powi(2)).sum::<f64>() / n;
    let cov = a
        .iter()
        .zip(b)
        .map(|(x, y)| (x - mu_a) * (y - mu_b))
        .sum::<f64>()
        / n;
    ((2.0 * mu_a * mu_b + C1) * (2.0 * cov + C2))
        / ((mu_a * mu_a + mu_b * mu_b + C1) * (var_a + var_b + C2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32, shift: u8) -> Rgba8Image {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 255 / w.max(1)) as u8).wrapping_add(shift);
                data.extend_from_slice(&[v, (y * 255 / h.max(1)) as u8, 128, 255]);
            }
        }
        Rgba8Image::new(w, h, data)
    }

    #[test]
    fn identical_images_are_perfect() {
        let a = gradient(32, 32, 0);
        let c = compare(&a, &a, 0);
        assert!(c.passes(&Tolerance::EXACT));
        assert_eq!(c.ssim, 1.0);
        assert!(c.psnr.is_infinite());
        assert_eq!(c.mismatched_pixels, 0);
    }

    #[test]
    fn one_code_value_off_passes_default_tolerance_but_not_exact() {
        let a = gradient(32, 32, 0);
        let b = gradient(32, 32, 1);
        let c = compare(&a, &b, Tolerance::default().channel_epsilon);
        assert!(c.passes(&Tolerance::default()), "{}", c.summary());
        assert!(!compare(&a, &b, 0).passes(&Tolerance::EXACT));
        assert_eq!(c.max_channel_diff[0], 1);
    }

    #[test]
    fn large_localized_changes_fail() {
        let a = gradient(64, 64, 0);
        let mut b = a.clone();
        for y in 10..30 {
            for x in 10..30 {
                let i = (y * 64 + x) * 4;
                b.data[i] = 255 - b.data[i];
                b.data[i + 1] = 0;
            }
        }
        let c = compare(&a, &b, 2);
        assert!(!c.passes(&Tolerance::default()), "{}", c.summary());
        assert!(c.ssim < 0.99);
        assert_eq!(c.mismatched_pixels, 400);
    }

    #[test]
    fn size_mismatch_never_passes() {
        let a = gradient(16, 16, 0);
        let b = gradient(16, 15, 0);
        assert!(!compare(&a, &b, 255).passes(&Tolerance {
            channel_epsilon: 255,
            max_mismatch_fraction: 1.0,
            min_ssim: 0.0,
            min_psnr: None
        }));
    }

    #[test]
    fn diff_image_marks_changed_pixels() {
        let a = gradient(16, 16, 0);
        let mut b = a.clone();
        b.data[0] = b.data[0].wrapping_add(40);
        let d = diff_image(&a, &b);
        assert_eq!(&d.data[0..3], &[255, 255 - 255, 0]);
        assert_eq!(d.data[7], 255);
        assert!(d.data[4] < 64);
    }

    #[test]
    fn png_round_trip() {
        let a = gradient(8, 8, 0);
        let png = a.to_png().unwrap();
        assert_eq!(Rgba8Image::from_png(&png).unwrap(), a);
    }
}
