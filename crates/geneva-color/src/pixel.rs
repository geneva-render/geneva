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
    pub fn to_srgb8(self) -> [u8; 4] {
        let c = self.to_srgb();
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [q(c.r), q(c.g), q(c.b), q(c.a)]
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
