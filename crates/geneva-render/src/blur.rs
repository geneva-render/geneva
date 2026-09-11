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
    if sigma <= 0.0 || frame.width() == 0 || frame.height() == 0 {
        return;
    }
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let mut scratch = vec![LinearRgba::TRANSPARENT; w * h];
    for radius in box_radii(sigma) {
        if radius == 0 {
            continue;
        }
        box_blur_rows(frame.pixels(), &mut scratch, w, radius);
        box_blur_columns(&scratch, frame.pixels_mut(), w, h, radius);
    }
}

/// The radii of three box blurs that together approximate a Gaussian of
/// standard deviation `sigma`.
fn box_radii(sigma: f64) -> [usize; 3] {
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
