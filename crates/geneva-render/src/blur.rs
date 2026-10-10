//! Gaussian blur of a premultiplied linear frame.
//!
//! The blur is three box blurs in a row, whose widths are chosen so that
//! their combination matches a Gaussian of the requested standard
//! deviation to within a fraction of a percent (the usual method for a
//! separable blur in linear time). Pixels beyond the frame count as
//! transparent, which is right for a layer drawn over transparency.

use geneva_color::LinearRgba;
use rayon::prelude::*;

use crate::Frame;

/// Blurs `frame` in place with a Gaussian of standard deviation `sigma`
/// pixels; a `sigma` of 0 or less leaves it as it is.
pub fn gaussian_blur(frame: &mut Frame, sigma: f64) {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    blur_pixels(frame.pixels_mut(), w, h, sigma);
}

/// The same blur over a bare row-major buffer of `w` by `h` pixels.
pub(crate) fn blur_pixels(pixels: &mut [LinearRgba], w: usize, h: usize, sigma: f64) {
    if sigma <= 0.0 || w == 0 || h == 0 || pixels.len() < w * h {
        return;
    }
    let mut scratch = vec![LinearRgba::TRANSPARENT; w * h];
    for radius in box_radii(sigma) {
        if radius == 0 {
            continue;
        }
        box_blur_rows(pixels, &mut scratch, w, radius);
        box_blur_columns(&scratch, pixels, w, h, radius);
    }
}

/// Blurs above this standard deviation, in pixels, are worked on a
/// smaller copy (see [`blur_pixels_reduced`]).
pub(crate) const REDUCE_ABOVE: f64 = 8.0;

const SPREAD_SHARE: f64 = 5.0;

/// How many times smaller [`blur_pixels_reduced`] works for `sigma`: so
/// that the smaller copy is blurred by about 4 pixels or more, and at
/// most 4 times smaller.
fn reduction(sigma: f64) -> usize {
    if sigma <= REDUCE_ABOVE {
        1
    } else {
        ((sigma / 4.0).floor() as usize).clamp(1, 4)
    }
}

/// The same blur as [`blur_pixels`], computed for a wide `sigma` on a
/// copy `k` times smaller (each of its pixels the mean of a k-by-k
/// block, the blocks past the edge transparent), blurred there by a
/// Gaussian of `sigma / k`, and brought back bilinearly. Against a true
/// Gaussian it is within about half a code, where the three boxes at
/// full size are within about five.
pub(crate) fn blur_pixels_reduced(pixels: &mut [LinearRgba], w: usize, h: usize, sigma: f64) {
    let k = reduction(sigma);
    if k == 1 || w < 2 * k || h < 2 * k || pixels.len() < w * h {
        blur_pixels(pixels, w, h, sigma);
        return;
    }
    let (lw, lh) = (w.div_ceil(k), h.div_ceil(k));
    let norm = 1.0 / (k * k) as f32;
    let mut small = vec![LinearRgba::TRANSPARENT; lw * lh];
    small.par_chunks_mut(lw).enumerate().for_each(|(ly, out)| {
        for y in ly * k..((ly + 1) * k).min(h) {
            let row = &pixels[y * w..][..w];
            for (lx, o) in out.iter_mut().enumerate() {
                for p in &row[lx * k..((lx + 1) * k).min(w)] {
                    o.r += p.r;
                    o.g += p.g;
                    o.b += p.b;
                    o.a += p.a;
                }
            }
        }
        for o in out.iter_mut() {
            *o = o.scaled(norm);
        }
    });
    // The small copy is blurred with a true Gaussian rather than three
    // boxes: at its few pixels of radius the kernel is short, and the
    // result is closer to a Gaussian than the boxes are at full size.
    // The block mean and the bilinear return widen it a little, a few
    // times the variance of a box k pixels wide, (k^2 - 1) / 12, which is
    // taken off what the small copy is blurred by.
    let spread = (k * k - 1) as f64 / 12.0;
    let rest = (sigma * sigma - SPREAD_SHARE * spread).max(0.0).sqrt() / k as f64;
    gaussian_kernel_blur(&mut small, lw, lh, rest);
    let kf = k as f32;
    pixels.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let v = ((y as f32 + 0.5) / kf - 0.5).clamp(0.0, (lh - 1) as f32);
        let y0 = v.floor() as usize;
        let y1 = (y0 + 1).min(lh - 1);
        let fy = v - y0 as f32;
        let (top, bottom) = (&small[y0 * lw..][..lw], &small[y1 * lw..][..lw]);
        for (x, p) in row.iter_mut().enumerate() {
            let u = ((x as f32 + 0.5) / kf - 0.5).clamp(0.0, (lw - 1) as f32);
            let x0 = u.floor() as usize;
            let x1 = (x0 + 1).min(lw - 1);
            let fx = u - x0 as f32;
            let lerp = |a: LinearRgba, b: LinearRgba, t: f32| LinearRgba {
                r: a.r + (b.r - a.r) * t,
                g: a.g + (b.g - a.g) * t,
                b: a.b + (b.b - a.b) * t,
                a: a.a + (b.a - a.a) * t,
            };
            *p = lerp(
                lerp(top[x0], top[x1], fx),
                lerp(bottom[x0], bottom[x1], fx),
                fy,
            );
        }
    });
}

/// A separable Gaussian of standard deviation `sigma` over a buffer of
/// `w` by `h` pixels, the kernel cut at three deviations and pixels past
/// the edge transparent.
fn gaussian_kernel_blur(pixels: &mut [LinearRgba], w: usize, h: usize, sigma: f64) {
    if sigma <= 0.0 {
        return;
    }
    let r = (3.0 * sigma).ceil() as usize;
    let mut kernel: Vec<f32> = (0..=2 * r)
        .map(|i| {
            let t = i as f64 - r as f64;
            (-t * t / (2.0 * sigma * sigma)).exp() as f32
        })
        .collect();
    let total: f32 = kernel.iter().sum();
    for k in &mut kernel {
        *k /= total;
    }
    let mut across = vec![LinearRgba::TRANSPARENT; w * h];
    across
        .par_chunks_mut(w)
        .zip(pixels.par_chunks(w))
        .for_each(|(out, row)| {
            for (x, o) in out.iter_mut().enumerate() {
                let (from, to) = (x.saturating_sub(r), (x + r).min(w - 1));
                let mut acc = [0f32; 4];
                for (j, p) in row[from..=to].iter().enumerate() {
                    let k = kernel[from + j + r - x];
                    acc[0] += p.r * k;
                    acc[1] += p.g * k;
                    acc[2] += p.b * k;
                    acc[3] += p.a * k;
                }
                *o = LinearRgba {
                    r: acc[0],
                    g: acc[1],
                    b: acc[2],
                    a: acc[3],
                };
            }
        });
    pixels.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
        let (from, to) = (y.saturating_sub(r), (y + r).min(h - 1));
        out.fill(LinearRgba::TRANSPARENT);
        for yy in from..=to {
            let k = kernel[yy + r - y];
            for (o, p) in out.iter_mut().zip(&across[yy * w..][..w]) {
                o.r += p.r * k;
                o.g += p.g * k;
                o.b += p.b * k;
                o.a += p.a * k;
            }
        }
    });
}

/// The radii of three box blurs that together approximate a Gaussian of
/// standard deviation `sigma`; each is applied along the rows and then
/// down the columns, and a radius of 0 is skipped. Public so that
/// another renderer blurs by the same three boxes.
pub fn box_radii(sigma: f64) -> [usize; 3] {
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let lower = lower.max(1);
    let upper = lower + 2;
    let (lo, up) = (lower as f64, upper as f64);
    let m = ((12.0 * sigma * sigma - n * lo * lo - 4.0 * n * lo - 3.0 * n) / (-4.0 * lo - 4.0))
        .round() as i64;
    let mut radii = [0; 3];
    for (i, r) in radii.iter_mut().enumerate() {
        let size = if (i as i64) < m { lower } else { upper };
        *r = ((size - 1) / 2).max(0) as usize;
    }
    let _ = up;
    radii
}

#[inline]
fn add(acc: &mut [f64; 4], p: LinearRgba) {
    acc[0] += f64::from(p.r);
    acc[1] += f64::from(p.g);
    acc[2] += f64::from(p.b);
    acc[3] += f64::from(p.a);
}

#[inline]
fn sub(acc: &mut [f64; 4], p: LinearRgba) {
    acc[0] -= f64::from(p.r);
    acc[1] -= f64::from(p.g);
    acc[2] -= f64::from(p.b);
    acc[3] -= f64::from(p.a);
}

#[inline]
fn mean(acc: &[f64; 4], norm: f64) -> LinearRgba {
    LinearRgba {
        r: (acc[0] * norm) as f32,
        g: (acc[1] * norm) as f32,
        b: (acc[2] * norm) as f32,
        a: (acc[3] * norm) as f32,
    }
}

/// Averages each pixel with its `r` neighbours on either side along the
/// row, a running sum per row, rows in parallel.
fn box_blur_rows(src: &[LinearRgba], dst: &mut [LinearRgba], w: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f64;
    dst.par_chunks_mut(w)
        .zip(src.par_chunks(w))
        .for_each(|(out, row)| {
            let mut acc = [0f64; 4];
            for p in &row[..(r + 1).min(w)] {
                add(&mut acc, *p);
            }
            for x in 0..w {
                out[x] = mean(&acc, norm);
                if x + r + 1 < w {
                    add(&mut acc, row[x + r + 1]);
                }
                if x >= r {
                    sub(&mut acc, row[x - r]);
                }
            }
        });
}

/// Averages each pixel with its `r` neighbours above and below, a
/// running sum per column; the rows are split into bands that run in
/// parallel, each starting its sums from the rows above it.
fn box_blur_columns(src: &[LinearRgba], dst: &mut [LinearRgba], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f64;
    let threads = rayon::current_num_threads().max(1);
    let band = h.div_ceil(threads).max(1);
    dst.par_chunks_mut(w * band)
        .enumerate()
        .for_each(|(b, out)| {
            let first = b * band;
            let rows = out.len() / w;
            let mut acc = vec![[0f64; 4]; w];
            for y in first.saturating_sub(r)..=(first + r).min(h - 1) {
                for (a, p) in acc.iter_mut().zip(&src[y * w..][..w]) {
                    add(a, *p);
                }
            }
            for i in 0..rows {
                let y = first + i;
                for (o, a) in out[i * w..][..w].iter_mut().zip(&acc) {
                    *o = mean(a, norm);
                }
                if y + r + 1 < h {
                    for (a, p) in acc.iter_mut().zip(&src[(y + r + 1) * w..][..w]) {
                        add(a, *p);
                    }
                }
                if y >= r {
                    for (a, p) in acc.iter_mut().zip(&src[(y - r) * w..][..w]) {
                        sub(a, *p);
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use geneva_color::Color;

    #[test]
    fn a_point_spreads_symmetrically_and_keeps_its_energy() {
        let mut f = Frame::new(41, 41, Color::TRANSPARENT);
        f.set(
            20,
            20,
            LinearRgba {
                r: 1.0,
                g: 0.5,
                b: 0.0,
                a: 1.0,
            },
        );
        gaussian_blur(&mut f, 3.0);
        let total: f64 = f.pixels().iter().map(|p| f64::from(p.r)).sum();
        assert!((total - 1.0).abs() < 1e-3, "{total}");
        assert!(f.get(20, 20).r < 0.05 && f.get(20, 20).r > 0.0);
        assert!((f.get(23, 20).r - f.get(17, 20).r).abs() < 1e-6);
        assert!((f.get(20, 24).r - f.get(20, 16).r).abs() < 1e-6);
        assert!((f.get(20, 24).r - f.get(24, 20).r).abs() < 1e-6);
        // Green keeps its ratio to red everywhere.
        let p = f.get(22, 21);
        assert!((p.g - p.r * 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_flat_picture_stays_flat_away_from_its_edges() {
        let mut f = Frame::new(64, 64, Color::WHITE);
        gaussian_blur(&mut f, 2.0);
        let p = f.get(32, 32);
        assert!(
            (p.r - 1.0).abs() < 1e-5 && (p.a - 1.0).abs() < 1e-5,
            "{p:?}"
        );
        // The edge fades toward the transparent outside.
        assert!(f.get(0, 32).a < 0.8);
    }

    #[test]
    fn a_wide_blur_on_a_smaller_copy_is_within_a_code_of_a_gaussian() {
        // A word-sized block of colour with room around it for the blur.
        for sigma in [8.5, 11.0, 17.0, 23.0] {
            let pad = (3.0 * sigma) as usize + 4;
            let (w, h) = (120 + 2 * pad, 40 + 2 * pad);
            let mut full = vec![LinearRgba::TRANSPARENT; w * h];
            for y in pad..pad + 40 {
                for x in pad..pad + 120 {
                    full[y * w + x] = LinearRgba {
                        r: 0.9,
                        g: 0.6,
                        b: 0.2,
                        a: 1.0,
                    };
                }
            }
            let mut reduced = full.clone();
            // The reference is the Gaussian kernel itself, at full size.
            gaussian_kernel_blur(&mut full, w, h, sigma);
            blur_pixels_reduced(&mut reduced, w, h, sigma);
            let worst = full
                .iter()
                .zip(&reduced)
                .map(|(a, b)| (a.a - b.a).abs().max((a.r - b.r).abs()))
                .fold(0.0f32, f32::max);
            assert!(worst < 0.6 / 255.0, "sigma {sigma}: {worst}");
        }
    }

    #[test]
    fn zero_radius_is_a_no_op() {
        let mut f = Frame::new(8, 8, Color::TRANSPARENT);
        f.set(
            3,
            3,
            LinearRgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
        );
        let before = f.clone();
        gaussian_blur(&mut f, 0.0);
        assert_eq!(f, before);
    }
}
