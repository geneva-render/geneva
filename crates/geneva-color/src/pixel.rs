use geneva_anim::Interpolate;

use crate::Transfer;

/// An sRGB color with straight (non-premultiplied) alpha, components in
/// `[0, 1]`. This is the form colors take in timelines and user input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    /// Red, sRGB encoded.
    pub r: f32,
    /// Green, sRGB encoded.
    pub g: f32,
    /// Blue, sRGB encoded.
    pub b: f32,
    /// Opacity; 1 is opaque.
    pub a: f32,
}

impl Color {
    /// Opaque black.
    pub const BLACK: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    /// Opaque white.
    pub const WHITE: Self = Self {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    /// Builds a color from 8-bit sRGB components and an 8-bit alpha.
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: f32::from(a) / 255.0,
        }
    }

    /// Converts to premultiplied linear light for compositing.
    pub fn to_linear(self) -> LinearRgba {
        let a = self.a;
        LinearRgba {
            r: Transfer::Srgb.to_linear(f64::from(self.r)) as f32 * a,
            g: Transfer::Srgb.to_linear(f64::from(self.g)) as f32 * a,
            b: Transfer::Srgb.to_linear(f64::from(self.b)) as f32 * a,
            a,
        }
    }

    /// Returns the color with its alpha multiplied by `opacity`.
    pub fn with_opacity(self, opacity: f32) -> Self {
        Self {
            a: self.a * opacity,
            ..self
        }
    }

    /// Formats the color as `#rrggbb` or `#rrggbbaa`.
    pub fn to_hex(self) -> String {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if (self.a - 1.0).abs() < 0.5 / 255.0 {
            format!("#{:02x}{:02x}{:02x}", q(self.r), q(self.g), q(self.b))
        } else {
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                q(self.r),
                q(self.g),
                q(self.b),
                q(self.a)
            )
        }
    }
}

/// A premultiplied linear-light RGBA value, the compositing working format.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LinearRgba {
    /// Red, linear, premultiplied by alpha.
    pub r: f32,
    /// Green, linear, premultiplied by alpha.
    pub g: f32,
    /// Blue, linear, premultiplied by alpha.
    pub b: f32,
    /// Coverage/opacity.
    pub a: f32,
}

impl LinearRgba {
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    /// Composites `self` over `dst` (Porter-Duff "over", premultiplied).
    pub fn over(self, dst: Self) -> Self {
        let k = 1.0 - self.a;
        Self {
            r: self.r + dst.r * k,
            g: self.g + dst.g * k,
            b: self.b + dst.b * k,
            a: self.a + dst.a * k,
        }
    }

    /// Scales every component, which for premultiplied values applies an
    /// opacity multiplier.
    pub fn scaled(self, k: f32) -> Self {
        Self {
            r: self.r * k,
            g: self.g * k,
            b: self.b * k,
            a: self.a * k,
        }
    }

    /// Converts to straight-alpha sRGB.
    ///
    /// Fully transparent pixels decode to transparent black.
    pub fn to_srgb(self) -> Color {
        if self.a <= 0.0 {
            return Color::TRANSPARENT;
        }
        let un = |v: f32| Transfer::Srgb.from_linear(f64::from(v / self.a).clamp(0.0, 1.0)) as f32;
        Color {
            r: un(self.r),
            g: un(self.g),
            b: un(self.b),
            a: self.a.min(1.0),
        }
    }

    /// Quantizes to 8-bit straight-alpha sRGB with round-to-nearest.
    ///
    /// The result is what [`to_srgb`](Self::to_srgb) followed by rounding
    /// gives, code for code, found by lookup rather than by evaluating
    /// the transfer curve three times a pixel.
    pub fn to_srgb8(self) -> [u8; 4] {
        if self.a <= 0.0 {
            return [0; 4];
        }
        let table = srgb8_table();
        let q = |v: f32| table.code(v / self.a);
        let a = (self.a.min(1.0) * 255.0 + 0.5) as u8;
        [q(self.r), q(self.g), q(self.b), a]
    }
}

/// The 8-bit sRGB code of a straight linear value, evaluated the slow
/// way: this is what the table reproduces.
fn srgb8_reference(v: f32) -> u8 {
    let s = Transfer::Srgb.from_linear(f64::from(v.clamp(0.0, 1.0))) as f32;
    (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Where each 8-bit sRGB code begins on the linear axis, and a coarse
/// table that lands within one code of the answer, so that quantizing
/// is one lookup and at most one comparison.
struct Srgb8Table {
    /// `starts[k]` is the smallest `f32` whose code is `k`; `starts[0]`
    /// is zero.
    starts: [f32; 256],
    /// The code at `i / COARSE` for each `i`, which is never more than
    /// one below the code of any value in that cell, since the curve's
    /// slope at the origin is 12.92, or 3295 codes per unit, and a cell
    /// is 1 / 4096 wide.
    coarse: Vec<u8>,
}

const COARSE: usize = 4096;

impl Srgb8Table {
    fn build() -> Self {
        let mut starts = [0f32; 256];
        for (k, start) in starts.iter_mut().enumerate().skip(1) {
            // Non-negative floats order as their bit patterns do, so the
            // first value with this code is found by bisecting bits
            // against the slow function itself.
            let (mut lo, mut hi) = (0u32, 1.0f32.to_bits());
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if usize::from(srgb8_reference(f32::from_bits(mid))) >= k {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            *start = f32::from_bits(lo);
        }
        let coarse = (0..=COARSE)
            .map(|i| srgb8_reference(i as f32 / COARSE as f32))
            .collect();
        Self { starts, coarse }
    }

    #[inline]
    fn code(&self, v: f32) -> u8 {
        let v = v.clamp(0.0, 1.0);
        // A NaN clamps to NaN and casts to zero, as the slow path does.
        let mut c = usize::from(self.coarse[(v * COARSE as f32) as usize]);
        while c < 255 && v >= self.starts[c + 1] {
            c += 1;
        }
        while c > 0 && v < self.starts[c] {
            c -= 1;
        }
        c as u8
    }
}

fn srgb8_table() -> &'static Srgb8Table {
    static TABLE: std::sync::OnceLock<Srgb8Table> = std::sync::OnceLock::new();
    TABLE.get_or_init(Srgb8Table::build)
}

#[cfg(test)]
mod srgb8_tests {
    use super::*;

    #[test]
    fn table_matches_the_curve() {
        let table = srgb8_table();
        for i in 0..=65535u32 {
            let v = i as f32 / 65535.0;
            assert_eq!(table.code(v), srgb8_reference(v), "at {v}");
        }
        // Either side of every code boundary, where a table can be off.
        for k in 1..256 {
            let s = table.starts[k];
            for v in [
                s,
                f32::from_bits(s.to_bits() - 1),
                s * 0.999_999,
                s * 1.000_001,
            ] {
                assert_eq!(table.code(v), srgb8_reference(v), "near code {k} at {v}");
            }
        }
        assert_eq!(table.code(-1.0), 0);
        assert_eq!(table.code(2.0), 255);
        assert_eq!(table.code(f32::NAN), srgb8_reference(f32::NAN));
    }

    #[test]
    fn premultiplied_pixels_quantize_as_before() {
        for (r, g, b, a) in [
            (0.5f32, 0.25, 0.125, 1.0),
            (0.1, 0.1, 0.1, 0.5),
            (0.0, 0.0, 0.0, 0.0),
            (1.0, 1.0, 1.0, 1.0),
            (0.214, 0.214, 0.214, 1.0),
        ] {
            let p = LinearRgba { r, g, b, a };
            let c = p.to_srgb();
            let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            assert_eq!(p.to_srgb8(), [q(c.r), q(c.g), q(c.b), q(c.a)]);
        }
    }
}

impl Interpolate for LinearRgba {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        Self {
            r: self.r.lerp(&other.r, t),
            g: self.g.lerp(&other.g, t),
            b: self.b.lerp(&other.b, t),
            a: self.a.lerp(&other.a, t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_white_over_black_blends_in_linear_light() {
        // Blending in sRGB space would give code 128; linear light gives 188.
        let src = Color {
            a: 0.5,
            ..Color::WHITE
        }
        .to_linear();
        let out = src.over(Color::BLACK.to_linear());
        assert_eq!(out.to_srgb8(), [188, 188, 188, 255]);
    }

    #[test]
    fn opaque_colors_round_trip_exactly_at_8_bits() {
        for v in 0..=255u8 {
            let c = Color::from_rgba8(v, 255 - v, v / 2, 255);
            assert_eq!(c.to_linear().to_srgb8(), [v, 255 - v, v / 2, 255]);
        }
    }

    #[test]
    fn over_is_associative_for_premultiplied_values() {
        let a = Color::from_rgba8(200, 30, 30, 120).to_linear();
        let b = Color::from_rgba8(30, 200, 30, 200).to_linear();
        let c = Color::from_rgba8(30, 30, 200, 255).to_linear();
        let left = a.over(b).over(c);
        let right = a.over(b.over(c));
        assert!((left.r - right.r).abs() < 1e-6);
        assert!((left.g - right.g).abs() < 1e-6);
        assert!((left.b - right.b).abs() < 1e-6);
        assert!((left.a - right.a).abs() < 1e-6);
    }

    #[test]
    fn transparent_decodes_to_transparent_black() {
        assert_eq!(LinearRgba::TRANSPARENT.to_srgb8(), [0, 0, 0, 0]);
    }

    #[test]
    fn hex_formatting() {
        assert_eq!(Color::from_rgba8(255, 136, 0, 255).to_hex(), "#ff8800");
        assert_eq!(Color::from_rgba8(255, 136, 0, 128).to_hex(), "#ff880080");
    }
}
