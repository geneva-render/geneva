use std::str::FromStr;

use thiserror::Error;

use crate::Color;

/// Error returned when a color string cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct ColorParseError(String);

impl ColorParseError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// Named colors accepted in timelines. Kept deliberately short: the
/// well-known CSS basics plus `transparent`.
const NAMED: &[(&str, [u8; 4])] = &[
    ("transparent", [0, 0, 0, 0]),
    ("black", [0, 0, 0, 255]),
    ("white", [255, 255, 255, 255]),
    ("red", [255, 0, 0, 255]),
    ("green", [0, 128, 0, 255]),
    ("blue", [0, 0, 255, 255]),
    ("yellow", [255, 255, 0, 255]),
    ("cyan", [0, 255, 255, 255]),
    ("magenta", [255, 0, 255, 255]),
    ("gray", [128, 128, 128, 255]),
    ("grey", [128, 128, 128, 255]),
    ("orange", [255, 165, 0, 255]),
];

impl FromStr for Color {
    type Err = ColorParseError;

    /// Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r, g, b)`,
    /// `rgba(r, g, b, a)` and a short list of names. Alpha in the
    /// functional forms is `0..=1`; RGB components are `0..=255`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            return parse_hex(hex);
        }
        let lower = s.to_ascii_lowercase();
        if let Some((_, rgba)) = NAMED.iter().find(|(n, _)| *n == lower) {
            return Ok(Self::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
        }
        if let Some(inner) = lower
            .strip_prefix("rgba(")
            .or_else(|| lower.strip_prefix("rgb("))
        {
            return parse_functional(inner);
        }
        Err(ColorParseError::new(format!(
            "unrecognized color {s:?}; use #rrggbb, #rrggbbaa, rgb(), rgba() or a basic color name"
        )))
    }
}

fn parse_hex(hex: &str) -> Result<Color, ColorParseError> {
    let digit = |c: char| {
        c.to_digit(16)
            .map(|v| v as u8)
            .ok_or_else(|| ColorParseError::new(format!("invalid hex digit {c:?} in color #{hex}")))
    };
    let chars: Vec<char> = hex.chars().collect();
    let bytes: Vec<u8> = match chars.len() {
        3 | 4 => chars
            .iter()
            .map(|&c| digit(c).map(|v| v * 17))
            .collect::<Result<_, _>>()?,
        6 | 8 => chars
            .chunks(2)
            .map(|p| Ok(digit(p[0])? * 16 + digit(p[1])?))
            .collect::<Result<_, ColorParseError>>()?,
        n => {
            return Err(ColorParseError::new(format!(
                "hex color #{hex} has {n} digits; expected 3, 4, 6 or 8"
            )));
        }
    };
    let a = bytes.get(3).copied().unwrap_or(255);
    Ok(Color::from_rgba8(bytes[0], bytes[1], bytes[2], a))
}

fn parse_functional(inner: &str) -> Result<Color, ColorParseError> {
    let Some(body) = inner.strip_suffix(')') else {
        return Err(ColorParseError::new(
            "rgb()/rgba() color is missing the closing parenthesis",
        ));
    };
    let parts: Vec<&str> = body.split(',').map(str::trim).collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(ColorParseError::new(format!(
            "rgb()/rgba() color needs 3 or 4 components, found {}",
            parts.len()
        )));
    }
    let channel = |s: &str| -> Result<f32, ColorParseError> {
        let v: f32 = s
            .parse()
            .map_err(|_| ColorParseError::new(format!("invalid color component {s:?}")))?;
        if !(0.0..=255.0).contains(&v) {
            return Err(ColorParseError::new(format!(
                "color component {s} is outside 0..=255"
            )));
        }
        Ok(v / 255.0)
    };
    let a = match parts.get(3) {
        Some(s) => {
            let v: f32 = s
                .parse()
                .map_err(|_| ColorParseError::new(format!("invalid alpha {s:?}")))?;
            if !(0.0..=1.0).contains(&v) {
                return Err(ColorParseError::new(format!("alpha {s} is outside 0..=1")));
            }
            v
        }
        None => 1.0,
    };
    Ok(Color {
        r: channel(parts[0])?,
        g: channel(parts[1])?,
        b: channel(parts[2])?,
        a,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_forms() {
        assert_eq!("#fff".parse::<Color>().unwrap(), Color::WHITE);
        assert_eq!(
            "#f00".parse::<Color>().unwrap(),
            Color::from_rgba8(255, 0, 0, 255)
        );
        assert_eq!(
            "#ff880080".parse::<Color>().unwrap(),
            Color::from_rgba8(255, 136, 0, 128)
        );
        assert_eq!(
            "#0008".parse::<Color>().unwrap(),
            Color::from_rgba8(0, 0, 0, 136)
        );
    }

    #[test]
    fn parses_functional_and_named_forms() {
        assert_eq!(
            "rgb(255, 136, 0)".parse::<Color>().unwrap(),
            Color::from_rgba8(255, 136, 0, 255)
        );
        let c: Color = "rgba(0,0,0,0.5)".parse().unwrap();
        assert!((c.a - 0.5).abs() < 1e-6);
        assert_eq!("Transparent".parse::<Color>().unwrap(), Color::TRANSPARENT);
    }

    #[test]
    fn rejects_garbage_with_a_reason() {
        let err = "#12345".parse::<Color>().unwrap_err();
        assert!(err.to_string().contains("5 digits"));
        let err = "rgb(300,0,0)".parse::<Color>().unwrap_err();
        assert!(err.to_string().contains("outside"));
        assert!("blurple".parse::<Color>().is_err());
    }
}
