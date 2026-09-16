//! A background resolved against one box, so the per-pixel work is a
//! projection and a lookup rather than parsing geometry again. Boxes in
//! markup and the glyphs of a text both draw through it.

use geneva_color::{Color, LinearRgba};
use geneva_html::style::{Background, Direction, Stop};

/// A colour as a browser keeps it while painting: sRGB-encoded channels
/// premultiplied by alpha. It is carried in a [`LinearRgba`] so the same
/// blending code serves both spaces; nothing here reads it as light.
pub(crate) fn encoded(c: Color) -> LinearRgba {
    LinearRgba {
        r: c.r * c.a,
        g: c.g * c.a,
        b: c.b * c.a,
        a: c.a,
    }
}

/// A colour or gradient laid over a tile that repeats across the plane,
/// which is CSS's background: `background-size` is the tile and
/// `background-position` is where it starts.
pub(crate) struct Fill {
    kind: Kind,
    /// The tile's origin and size, in the pixels being painted.
    tile: (f64, f64, f64, f64),
}

enum Kind {
    Flat(LinearRgba),
    /// Stops premultiplied in linear light, which is what `LinearRgba`
    /// already is, so interpolating them component by component is the
    /// correct blend rather than an approximation of it.
    Ramp {
        /// Where the ramp starts, in pixels.
        origin: (f64, f64),
        /// The direction and length of the gradient line. For a radial
        /// fill these are the two radii instead.
        axis: (f64, f64),
        radial: bool,
        stops: Vec<(f64, LinearRgba)>,
    },
}

impl Fill {
    /// A fill whose tile is `(x, y, width, height)`, with its colours in
    /// linear light.
    pub(crate) fn new(background: &Background, tile: (f64, f64, f64, f64)) -> Self {
        Self::build(background, tile, false)
    }

    /// The same fill with its colours sRGB-encoded and premultiplied,
    /// which is how a browser mixes a gradient's stops.
    pub(crate) fn new_encoded(background: &Background, tile: (f64, f64, f64, f64)) -> Self {
        Self::build(background, tile, true)
    }

    fn build(background: &Background, tile: (f64, f64, f64, f64), encode: bool) -> Self {
        let colour = |c: Color| if encode { encoded(c) } else { c.to_linear() };
        let (x, y, w, h) = (tile.0, tile.1, tile.2.max(1.0), tile.3.max(1.0));
        let kind = match background {
            Background::Color(c) => Kind::Flat(colour(*c)),
            Background::Linear { direction, stops } => {
                let degrees = match *direction {
                    Direction::Angle(d) => d,
                    // A corner's angle depends on the box: the gradient
                    // line has to be perpendicular to the diagonal, so a
                    // wide box points the ramp more sideways.
                    Direction::Corner { top, right } => {
                        let corner = w.atan2(h).to_degrees();
                        match (top, right) {
                            (true, true) => corner,
                            (true, false) => 360.0 - corner,
                            (false, true) => 180.0 - corner,
                            (false, false) => 180.0 + corner,
                        }
                    }
                };
                let radians = degrees.to_radians();
                // CSS measures clockwise from "to top", and y grows down.
                let (dx, dy) = (radians.sin(), -radians.cos());
                let length = (w * dx).abs() + (h * dy).abs();
                Kind::Ramp {
                    origin: (x + w / 2.0, y + h / 2.0),
                    axis: (dx * length.max(1.0), dy * length.max(1.0)),
                    radial: false,
                    stops: ramp(stops, &colour),
                }
            }
            Background::Radial { circle, at, stops } => {
                let centre = (x + w * at.0, y + h * at.1);
                // Sized to the farthest corner, which is what CSS does
                // when nothing says otherwise.
                let side_x = (centre.0 - x).max(x + w - centre.0);
                let side_y = (centre.1 - y).max(y + h - centre.1);
                let (rx, ry) = if *circle {
                    let r = side_x.hypot(side_y).max(1.0);
                    (r, r)
                } else {
                    // The ellipse keeps the box's proportions and is
                    // scaled out until it touches that corner.
                    let k = std::f64::consts::SQRT_2;
                    ((side_x * k).max(1.0), (side_y * k).max(1.0))
                };
                Kind::Ramp {
                    origin: centre,
                    axis: (rx, ry),
                    radial: true,
                    stops: ramp(stops, &colour),
                }
            }
        };
        Self {
            kind,
            tile: (x, y, w, h),
        }
    }

    /// The colour at a pixel. Outside the tile it is the tile repeated,
    /// as a CSS background is.
    pub(crate) fn at(&self, px: f64, py: f64) -> LinearRgba {
        match &self.kind {
            Kind::Flat(c) => *c,
            Kind::Ramp {
                origin,
                axis,
                radial,
                stops,
            } => {
                let (x, y, w, h) = self.tile;
                let px = x + (px - x).rem_euclid(w);
                let py = y + (py - y).rem_euclid(h);
                let (ox, oy) = *origin;
                let t = if *radial {
                    ((px - ox) / axis.0).hypot((py - oy) / axis.1)
                } else {
                    let length2 = axis.0 * axis.0 + axis.1 * axis.1;
                    ((px - ox) * axis.0 + (py - oy) * axis.1) / length2 + 0.5
                };
                sample(stops, t.clamp(0.0, 1.0))
            }
        }
    }
}

/// Stops as (position, premultiplied colour), in whichever space
/// `colour` gives.
fn ramp(stops: &[Stop], colour: &impl Fn(Color) -> LinearRgba) -> Vec<(f64, LinearRgba)> {
    stops
        .iter()
        .map(|s| (s.at.unwrap_or(0.0), colour(s.color)))
        .collect()
}

fn sample(stops: &[(f64, LinearRgba)], t: f64) -> LinearRgba {
    let Some(first) = stops.first() else {
        return LinearRgba::TRANSPARENT;
    };
    if t <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t <= b.0 {
            let span = b.0 - a.0;
            // Two stops at the same place are a hard edge, not a
            // division by zero.
            let k = if span <= f64::EPSILON {
                1.0
            } else {
                (t - a.0) / span
            } as f32;
            return LinearRgba {
                r: a.1.r + (b.1.r - a.1.r) * k,
                g: a.1.g + (b.1.g - a.1.g) * k,
                b: a.1.b + (b.1.b - a.1.b) * k,
                a: a.1.a + (b.1.a - a.1.a) * k,
            };
        }
    }
    stops[stops.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;
    use geneva_color::Color;

    fn red_to_blue() -> Background {
        Background::Linear {
            direction: Direction::Angle(90.0),
            stops: vec![
                Stop {
                    color: Color::from_rgba8(255, 0, 0, 255),
                    at: Some(0.0),
                },
                Stop {
                    color: Color::from_rgba8(0, 0, 255, 255),
                    at: Some(1.0),
                },
            ],
        }
    }

    #[test]
    fn the_tile_repeats() {
        let fill = Fill::new(&red_to_blue(), (0.0, 0.0, 100.0, 10.0));
        let inside = fill.at(10.0, 5.0);
        let next_tile = fill.at(110.0, 5.0);
        assert!((inside.r - next_tile.r).abs() < 1e-6);
        assert!(inside.r > 0.8, "near the red end: {inside:?}");
        assert!(
            fill.at(-10.0, 5.0).b > 0.8,
            "a step back wraps to the blue end"
        );
    }

    #[test]
    fn a_shifted_tile_moves_the_ramp() {
        let still = Fill::new(&red_to_blue(), (0.0, 0.0, 100.0, 10.0));
        let moved = Fill::new(&red_to_blue(), (30.0, 0.0, 100.0, 10.0));
        assert!((still.at(20.0, 5.0).r - moved.at(50.0, 5.0).r).abs() < 1e-6);
    }
}
