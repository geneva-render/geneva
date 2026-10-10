//! `backdrop-filter`: what a box in markup does to the picture behind it.
//!
//! As in a browser (Chromium, measured), the filters read the backdrop
//! inside the box's border box only, with transparency around it, so
//! nothing outside bleeds in and a blur thins out towards the edges; they
//! work on sRGB-encoded values; the result is laid over the unfiltered
//! backdrop, clipped to the box's rounded corners and mixed in by the
//! box's opacity. The box itself is then drawn over it. The backdrop is everything composited under the clip
//! the box belongs to. Boxes of the same markup painted under this one
//! are not part of it: they are drawn with the rest of the clip, after.
//!
//! The filtering here works on a region of premultiplied, encoded RGBA,
//! so the compositor (which holds linear light) and the direct path
//! (which holds decoded Y'CbCr planes) share it and differ only in how
//! they read the region and write it back.

pub use geneva_html::style::Filter;
use rayon::prelude::*;

use crate::placement::Placement;

/// Whether any markup in `comp` (nested compositions included) asks for
/// a `backdrop-filter`: a renderer that does not draw one hands the
/// frames to the CPU. Read from the text, so a declaration the cascade
/// would drop still counts; that costs a CPU render, never a wrong
/// picture.
#[must_use]
pub fn used_in(comp: &geneva_timeline::Composition) -> bool {
    first_use(comp).is_some()
}

/// The JSON pointer of the first clip [`used_in`] finds.
#[must_use]
pub fn first_use(comp: &geneva_timeline::Composition) -> Option<&str> {
    use geneva_timeline::{ResolvedLayer, ResolvedSource};
    fn layers(list: &[ResolvedLayer]) -> Option<&str> {
        list.iter()
            .flat_map(|l| &l.clips)
            .find_map(|c| match &c.source {
                ResolvedSource::Html(h) => (h.html.contains("backdrop-filter")
                    || h.css.contains("backdrop-filter")
                    || h.linked.values().any(|t| t.contains("backdrop-filter")))
                .then_some(c.path.as_str()),
                ResolvedSource::Composition(nested) => layers(&nested.layers),
                _ => None,
            })
    }
    layers(&comp.layers)
}

/// A box with a `backdrop-filter`, in the paint coordinates of its clip.
#[derive(Debug, Clone, PartialEq)]
pub struct Backdrop {
    /// The border box: x, y, width, height.
    pub rect: [f64; 4],
    /// Corner radii: top-left, top-right, bottom-right, bottom-left.
    pub radius: [f64; 4],
    /// The filters, in order.
    pub filters: Vec<Filter>,
    /// The box's opacity with its ancestors', which the filtered
    /// backdrop is mixed in by.
    pub opacity: f64,
}

/// A backdrop where it lands on the output.
#[derive(Clone)]
pub struct PlacedBackdrop {
    /// The box.
    pub backdrop: Backdrop,
    /// How the clip's paint maps onto the output.
    pub place: Placement,
    /// The clip's own opacity at this moment, on top of the box's.
    pub opacity: f64,
    /// The output pixels it may touch: `[x0, y0, x1, y1]`.
    pub bounds: [u32; 4],
}

impl PlacedBackdrop {
    /// Places `backdrop` with the clip's `place`, on an output of `size`;
    /// `None` when it lands outside or does nothing.
    #[must_use]
    pub fn new(
        backdrop: &Backdrop,
        place: &Placement,
        opacity: f64,
        size: (u32, u32),
    ) -> Option<Self> {
        let opacity = opacity * backdrop.opacity;
        if opacity <= 0.0 || backdrop.filters.is_empty() {
            return None;
        }
        let [x, y, w, h] = backdrop.rect;
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let forward = |px: f64, py: f64| {
            let dx = (px - place.anchor[0]) * place.scale[0];
            let dy = (py - place.anchor[1]) * place.scale[1];
            (
                place.position[0] + place.cos * dx - place.sin * dy,
                place.position[1] + place.sin * dx + place.cos * dy,
            )
        };
        let corners = [
            forward(x, y),
            forward(x + w, y),
            forward(x, y + h),
            forward(x + w, y + h),
        ];
        let x0 = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0);
        let y0 = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .max(0.0);
        let x1 = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(f64::from(size.0));
        let y1 = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .min(f64::from(size.1));
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        Some(Self {
            backdrop: backdrop.clone(),
            place: place.clone(),
            opacity,
            bounds: [x0 as u32, y0 as u32, x1 as u32, y1 as u32],
        })
    }

    /// Output pixels per paint pixel, for blur radii and edge widths.
    #[must_use]
    pub fn scale(&self) -> f64 {
        f64::midpoint(self.place.scale[0].abs(), self.place.scale[1].abs())
    }

    /// How much of the output pixel at `(x, y)` (its top-left corner)
    /// the box covers, with the box's opacity and the clip's: what the
    /// filtered backdrop is mixed in by there.
    #[must_use]
    pub fn weight(&self, x: u32, y: u32) -> f32 {
        let (px, py) = self.place.inverse(f64::from(x) + 0.5, f64::from(y) + 0.5);
        let d = rounded_rect_distance(self.backdrop.rect, self.backdrop.radius, px, py);
        // Half a pixel either side of the edge, in output pixels.
        let cover = (0.5 - d * self.scale()).clamp(0.0, 1.0);
        (cover * self.opacity) as f32
    }
}

/// Signed distance from a point to a rounded rectangle, negative inside.
fn rounded_rect_distance(rect: [f64; 4], radius: [f64; 4], px: f64, py: f64) -> f64 {
    let [x, y, w, h] = rect;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (qx, qy) = (px - cx, py - cy);
    // The corner this point is nearest, and its radius, kept within the
    // box as CSS does.
    let r = match (qx < 0.0, qy < 0.0) {
        (true, true) => radius[0],
        (false, true) => radius[1],
        (false, false) => radius[2],
        (true, false) => radius[3],
    }
    .clamp(0.0, (w / 2.0).min(h / 2.0));
    let ax = qx.abs() - (w / 2.0 - r);
    let ay = qy.abs() - (h / 2.0 - r);
    let outside = (ax.max(0.0).powi(2) + ay.max(0.0).powi(2)).sqrt();
    outside + ax.max(ay).min(0.0) - r
}

/// Applies `filters` in order to a `w` by `h` region of premultiplied,
/// sRGB-encoded RGBA, in place, and lays the result over the region as
/// it was. Blur radii are in paint pixels and are multiplied by `scale`.
/// Nothing outside the region is read: a blur sees transparency there,
/// so near the edges the result is partly the unblurred backdrop.
pub fn filter_region(region: &mut [[f32; 4]], w: usize, h: usize, filters: &[Filter], scale: f64) {
    if w == 0 || h == 0 || region.len() < w * h {
        return;
    }
    let original = region[..w * h].to_vec();
    for f in filters {
        match *f {
            Filter::Blur(r) => blur_region(region, w, h, r * scale),
            Filter::Saturate(s) => per_pixel(region, |c| saturate(c, s as f32)),
            Filter::Brightness(b) => per_pixel(region, |c| c.map(|v| v * b as f32)),
            Filter::Contrast(k) => {
                per_pixel(region, |c| c.map(|v| (v - 0.5) * k as f32 + 0.5));
            }
        }
    }
    region.par_iter_mut().zip(&original).for_each(|(f, o)| {
        let under = 1.0 - f[3];
        for c in 0..4 {
            f[c] += o[c] * under;
        }
    });
}

/// Runs a colour function over unpremultiplied values, clamped to [0, 1]
/// as a browser's filter chain clamps between steps.
fn per_pixel(region: &mut [[f32; 4]], f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    region.par_iter_mut().for_each(|p| {
        let a = p[3];
        if a <= 0.0 {
            return;
        }
        let c = f([p[0] / a, p[1] / a, p[2] / a]).map(|v| v.clamp(0.0, 1.0));
        *p = [c[0] * a, c[1] * a, c[2] * a, a];
    });
}

/// The Filter Effects `saturate()` matrix.
fn saturate([r, g, b]: [f32; 3], s: f32) -> [f32; 3] {
    [
        (0.213 + 0.787 * s) * r + (0.715 - 0.715 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 + 0.285 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 - 0.715 * s) * g + (0.072 + 0.928 * s) * b,
    ]
}

/// Above this standard deviation, in output pixels, the region is
/// blurred at a half or a quarter of its size and scaled back, as
/// browsers do: the difference does not show and the cost falls by the
/// square of the factor.
const FULL_SIZE_SIGMA: f64 = 8.0;

/// A Gaussian blur of standard deviation `sigma`, transparent outside.
fn blur_region(region: &mut [[f32; 4]], w: usize, h: usize, sigma: f64) {
    if sigma <= 0.0 {
        return;
    }
    let k = if sigma <= FULL_SIZE_SIGMA {
        1
    } else if sigma <= 2.0 * FULL_SIZE_SIGMA {
        2
    } else {
        4
    };
    if k == 1 || w < 2 * k || h < 2 * k {
        blur_full(region, w, h, sigma);
        return;
    }
    let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
    let mut small = vec![[0.0f32; 4]; sw * sh];
    small.par_chunks_mut(sw).enumerate().for_each(|(sy, row)| {
        for (sx, out) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0f32;
            for y in sy * k..((sy + 1) * k).min(h) {
                for x in sx * k..((sx + 1) * k).min(w) {
                    let p = region[y * w + x];
                    for c in 0..4 {
                        acc[c] += p[c];
                    }
                    n += 1.0;
                }
            }
            *out = acc.map(|v| v / n);
        }
    });
    blur_full(&mut small, sw, sh, sigma / k as f64);
    // Bilinear back up, sampling the small picture at pixel centres.
    let kf = k as f32;
    region.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let fy = ((y as f32 + 0.5) / kf - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for (x, out) in row.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) / kf - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let at = |xx: usize, yy: usize| small[yy * sw + xx];
            let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            for i in 0..4 {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bottom = c[i] + (d[i] - c[i]) * tx;
                out[i] = top + (bottom - top) * ty;
            }
        }
    });
}

/// Three box blurs along the rows and then down the columns.
fn blur_full(region: &mut [[f32; 4]], w: usize, h: usize, sigma: f64) {
    for r in crate::blur::box_radii(sigma) {
        if r == 0 {
            continue;
        }
        region
            .par_chunks_mut(w)
            .for_each(|row| box_line(row, r, &mut Vec::new()));
        let mut columns = transpose(region, w, h);
        columns
            .par_chunks_mut(h)
            .for_each(|col| box_line(col, r, &mut Vec::new()));
        let back = transpose(&columns, h, w);
        region.copy_from_slice(&back);
    }
}

fn transpose(src: &[[f32; 4]], w: usize, h: usize) -> Vec<[f32; 4]> {
    let mut out = vec![[0.0f32; 4]; w * h];
    out.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, v) in col.iter_mut().enumerate() {
            *v = src[y * w + x];
        }
    });
    out
}

/// A box blur of radius `r` along one line, transparent past the ends.
fn box_line(line: &mut [[f32; 4]], r: usize, padded: &mut Vec<[f32; 4]>) {
    let n = line.len();
    if n == 0 {
        return;
    }
    padded.clear();
    padded.resize(r, [0.0; 4]);
    padded.extend_from_slice(line);
    padded.resize(n + 2 * r, [0.0; 4]);
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut acc = [0.0f32; 4];
    for p in &padded[..=2 * r] {
        for c in 0..4 {
            acc[c] += p[c];
        }
    }
    for i in 0..n {
        line[i] = acc.map(|v| v * norm);
        if i + 1 < n {
            let (add, sub) = (padded[i + 2 * r + 1], padded[i]);
            for c in 0..4 {
                acc[c] += add[c] - sub[c];
            }
        }
    }
}

/// Does `placed` to a linear-light frame: the region under the box read
/// as encoded values, filtered, mixed in by [`PlacedBackdrop::weight`]
/// in the encoded values a browser mixes in, and written back.
pub(crate) fn apply_to_frame(frame: &mut crate::Frame, placed: &PlacedBackdrop) {
    use geneva_color::LinearRgba;
    let [x0, y0, x1, y1] = placed.bounds;
    let fw = frame.width();
    let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let pixels = frame.pixels_mut();
    let at = |x: u32, y: u32| (y * fw + x) as usize;
    let mut original = vec![[0.0f32; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = crate::html::encode_pixel(pixels[at(x0 + x as u32, y0 + y as u32)]);
            original[y * w + x] = [p.r, p.g, p.b, p.a];
        }
    }
    let mut filtered = original.clone();
    filter_region(
        &mut filtered,
        w,
        h,
        &placed.backdrop.filters,
        placed.scale(),
    );
    for y in 0..h {
        for x in 0..w {
            let (ox, oy) = (x0 + x as u32, y0 + y as u32);
            let k = placed.weight(ox, oy);
            if k <= 0.0 {
                continue;
            }
            let (o, f) = (original[y * w + x], filtered[y * w + x]);
            let mix = |i: usize| o[i] + (f[i] - o[i]) * k;
            pixels[at(ox, oy)] = crate::html::decode_pixel(LinearRgba {
                r: mix(0),
                g: mix(1),
                b: mix(2),
                a: mix(3),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_outside_the_region_bleeds_in() {
        // A white region blurred stays white: the blurred picture thins
        // out at the edges and the unblurred white shows through.
        let (w, h) = (40, 30);
        let mut region = vec![[1.0f32, 1.0, 1.0, 1.0]; w * h];
        filter_region(&mut region, w, h, &[Filter::Blur(12.0)], 1.0);
        assert!(
            region.iter().all(|p| (p[0] - 1.0).abs() < 1e-4),
            "{:?}",
            region[0]
        );
    }

    #[test]
    fn near_an_edge_about_half_is_the_unblurred_backdrop() {
        // Columns alternating black and white: the middle row blurs to
        // grey in the middle, and keeps about half its contrast at the
        // top edge (Chromium: 0.5 at the edge, 1 two sigma in).
        let (w, h) = (96, 96);
        let stripes: Vec<[f32; 4]> = (0..w * h)
            .map(|i| {
                if (i % w / 12) % 2 == 0 {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    [1.0, 1.0, 1.0, 1.0]
                }
            })
            .collect();
        let mut region = stripes;
        filter_region(&mut region, w, h, &[Filter::Blur(12.0)], 1.0);
        let middle = region[48 * w + 18][0];
        let edge = region[18][0];
        assert!((middle - 0.5).abs() < 0.08, "{middle}");
        assert!(edge > 0.6 && edge < 0.9, "{edge}");
    }

    #[test]
    fn a_large_blur_on_a_small_copy_matches_the_full_one() {
        let (w, h) = (96, 64);
        let stripes: Vec<[f32; 4]> = (0..w * h)
            .map(|i| {
                if (i % w / 8) % 2 == 0 {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    [1.0, 1.0, 1.0, 1.0]
                }
            })
            .collect();
        let mut full = stripes.clone();
        blur_full(&mut full, w, h, 12.0);
        let mut quick = stripes;
        blur_region(&mut quick, w, h, 12.0);
        let worst = full
            .iter()
            .zip(&quick)
            .map(|(a, b)| (a[0] - b[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.02, "{worst}");
    }

    #[test]
    fn saturate_leaves_grey_alone_and_one_is_the_identity() {
        let grey = saturate([0.5, 0.5, 0.5], 1.15);
        assert!(grey.iter().all(|v| (v - 0.5).abs() < 1e-3), "{grey:?}");
        let c = [0.2, 0.6, 0.9];
        let same = saturate(c, 1.0);
        assert!(same.iter().zip(c).all(|(a, b)| (a - b).abs() < 1e-3));
    }

    #[test]
    fn the_rounded_corner_is_outside_the_box() {
        let rect = [0.0, 0.0, 100.0, 50.0];
        let r = [12.0; 4];
        assert!(rounded_rect_distance(rect, r, 50.0, 25.0) < 0.0);
        assert!(rounded_rect_distance(rect, r, 1.0, 1.0) > 0.0);
        assert!(rounded_rect_distance(rect, r, 12.0, 1.0) < 0.0);
    }
}
