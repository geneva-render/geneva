//! CSS shorthand strings, accepted where an object also is.
//!
//! Authors and agents know `text-shadow: 0 2px 8px #0008` and
//! `font: 600 40px/1.2 Inter`; the timeline takes those strings for
//! `shadow`, `outline`, `padding` and `font` and turns them into the same
//! structures the objects give, so nothing downstream sees a difference.
//! The object form stays canonical: printed timelines use it. Nothing
//! here is a layout engine: no selectors, no box model, no cascade.

use std::fmt;
use std::str::FromStr;

use geneva_color::Color;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::animated::Animated;
use crate::color::ColorValue;
use crate::schema::{Shadow, Stroke, TextFill, TextStyle};

/// Splits on whitespace, keeping parenthesized groups such as
/// `rgba(0, 0, 0, 0.5)` and quoted names together.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => {
                    depth += 1;
                    current.push(c);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    current.push(c);
                }
                c if c.is_whitespace() && depth == 0 => {
                    if !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            },
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// A pixel length: `12`, `12px`, `-2.5px`, `0`.
fn pixels(token: &str) -> Option<f64> {
    let t = token.trim();
    let number = t.strip_suffix("px").unwrap_or(t).trim();
    if number.is_empty() {
        return None;
    }
    number.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn color(token: &str) -> Option<ColorValue> {
    Color::from_str(token).ok().map(ColorValue)
}

/// `"0 2px 8px #0008"`: horizontal and vertical offsets, an optional blur,
/// and an optional color, in any order for the color (as `text-shadow`).
pub fn parse_shadow(text: &str) -> Result<Shadow, String> {
    let mut lengths = Vec::new();
    let mut shade = None;
    for token in tokens(text) {
        if let Some(px) = pixels(&token) {
            lengths.push(px);
        } else if let Some(c) = color(&token) {
            if shade.replace(c).is_some() {
                return Err(format!("{text:?}: a shadow has one color"));
            }
        } else {
            return Err(format!(
                "{text:?}: {token:?} is neither a length such as 2px nor a color"
            ));
        }
    }
    match lengths.as_slice() {
        [x, y] | [x, y, _] => {
            let blur = lengths.get(2).copied().unwrap_or(0.0);
            if blur < 0.0 {
                return Err(format!("{text:?}: the blur {blur}px cannot be negative"));
            }
            // The shorthand can only say one value, so it is a constant.
            Ok(Shadow {
                color: shade.map(Animated::Constant),
                x: Animated::Constant(*x),
                y: Animated::Constant(*y),
                blur: Animated::Constant(blur),
            })
        }
        _ => Err(format!(
            "{text:?}: a shadow is \"<x> <y> [blur] [color]\", for example \"0 2px 8px #0008\""
        )),
    }
}

/// `"2px black"`: a width and a color, in either order (as `outline`).
pub fn parse_stroke(text: &str) -> Result<Stroke, String> {
    let mut width = None;
    let mut shade = None;
    for token in tokens(text) {
        if let Some(px) = pixels(&token) {
            if width.replace(px).is_some() {
                return Err(format!("{text:?}: an outline has one width"));
            }
        } else if let Some(c) = color(&token) {
            if shade.replace(c).is_some() {
                return Err(format!("{text:?}: an outline has one color"));
            }
        } else {
            return Err(format!(
                "{text:?}: {token:?} is neither a length such as 2px nor a color"
            ));
        }
    }
    match (width, shade) {
        (Some(w), Some(c)) if w >= 0.0 => Ok(Stroke { color: c, width: w }),
        (Some(w), _) if w < 0.0 => Err(format!("{text:?}: the width cannot be negative")),
        _ => Err(format!(
            "{text:?}: an outline is \"<width> <color>\", for example \"2px black\""
        )),
    }
}

/// `"8px"` or `"8"`: one value (padding is the same on every side).
pub fn parse_padding(text: &str) -> Result<f64, String> {
    let list = tokens(text);
    match list.as_slice() {
        [one] => pixels(one)
            .filter(|v| *v >= 0.0)
            .ok_or_else(|| format!("{text:?}: padding is one length such as \"8px\"")),
        _ => Err(format!(
            "{text:?}: padding takes one value, the same on every side"
        )),
    }
}

/// The parts of a `font` shorthand.
#[derive(Debug, Clone, PartialEq)]
pub struct FontShorthand {
    /// `italic` or `oblique` was given.
    pub italic: Option<bool>,
    /// A weight from 100 to 900 (`bold` is 700).
    pub weight: Option<u16>,
    /// Size in pixels.
    pub size: f64,
    /// A multiple of the size.
    pub line_height: Option<f64>,
    /// The first family named.
    pub family: String,
}

/// Whether a `font` value is a shorthand rather than a family name: it
/// has a size token such as `40px` or `40px/1.2`.
fn looks_like_shorthand(text: &str) -> bool {
    tokens(text).iter().any(|t| size_token(t).is_some())
}

/// `40px`, `40px/1.2`, `40px/48px`: the size and an optional line height.
/// The size needs its unit, as in CSS: a bare number before it is a
/// weight.
fn size_token(token: &str) -> Option<(f64, Option<Result<f64, String>>)> {
    let (size, rest) = match token.split_once('/') {
        Some((s, lh)) => (s, Some(lh)),
        None => (token, None),
    };
    let size = size
        .trim()
        .strip_suffix("px")?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)?;
    let line_height = rest.map(|lh| {
        let lh = lh.trim();
        if let Some(pct) = lh.strip_suffix('%') {
            pct.trim()
                .parse::<f64>()
                .map(|p| p / 100.0)
                .map_err(|_| format!("{lh:?} is not a line height"))
        } else if let Some(px) = lh.strip_suffix("px") {
            px.trim()
                .parse::<f64>()
                .map(|p| p / size)
                .map_err(|_| format!("{lh:?} is not a line height"))
        } else {
            lh.parse::<f64>()
                .map_err(|_| format!("{lh:?} is not a line height"))
        }
    });
    Some((size, line_height))
}

/// `"600 40px/1.2 Inter"`: optional style and weight, a size with an
/// optional line height, then the family (the first of a comma list).
/// `None` when the text is a plain family name.
pub fn parse_font(text: &str) -> Option<Result<FontShorthand, String>> {
    if !looks_like_shorthand(text) {
        return None;
    }
    Some(parse_font_shorthand(text))
}

fn parse_font_shorthand(text: &str) -> Result<FontShorthand, String> {
    let list = tokens(text);
    let mut italic = None;
    let mut weight = None;
    let mut i = 0;
    let (size, line_height) = loop {
        let Some(token) = list.get(i) else {
            return Err(format!(
                "{text:?}: a font shorthand needs a size such as 40px"
            ));
        };
        if let Some(found) = size_token(token) {
            i += 1;
            break found;
        }
        match token.to_ascii_lowercase().as_str() {
            "normal" => {}
            "italic" | "oblique" => italic = Some(true),
            "bold" => weight = Some(700),
            "bolder" | "lighter" | "small-caps" => {
                return Err(format!(
                    "{text:?}: {token:?} is relative to a parent, which a timeline has none of; use a weight from 100 to 900"
                ));
            }
            other => match other.parse::<u16>() {
                Ok(w) if (100..=900).contains(&w) => weight = Some(w),
                _ => {
                    return Err(format!(
                        "{text:?}: {token:?} is not a style (italic), a weight (100 to 900, bold) or a size such as 40px"
                    ));
                }
            },
        }
        i += 1;
    };
    let line_height = line_height.transpose()?;
    let family = list[i..]
        .join(" ")
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_owned();
    if family.is_empty() {
        return Err(format!(
            "{text:?}: a font shorthand ends with the family, for example \"600 40px Inter\""
        ));
    }
    Ok(FontShorthand {
        italic,
        weight,
        size,
        line_height,
        family,
    })
}

/// Expands a `font` shorthand in `style` into its fields, leaving fields
/// the author set explicitly alone. `line_height` is the source's, which
/// the shorthand's `/1.2` part sets.
pub fn expand_font(style: &mut TextStyle, line_height: &mut Option<f64>) -> Result<(), String> {
    let Some(font) = &style.font else {
        return Ok(());
    };
    let Some(parsed) = parse_font(font) else {
        return Ok(());
    };
    let short = parsed?;
    style.font = Some(short.family);
    style.size.get_or_insert(short.size);
    if let Some(w) = short.weight {
        style.weight.get_or_insert(w);
    }
    if let Some(i) = short.italic {
        style.italic.get_or_insert(i);
    }
    if let Some(lh) = short.line_height {
        line_height.get_or_insert(lh);
    }
    Ok(())
}

// ---- serde: the object form or the string form -------------------------

/// The object form of a shadow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShadowFields {
    /// Shadow color. Defaults to 50% black. Takes keyframes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Animated<ColorValue>>,
    /// Horizontal offset in pixels. Takes keyframes.
    #[serde(default)]
    pub x: Animated<f64>,
    /// Vertical offset in pixels. Takes keyframes.
    #[serde(default)]
    pub y: Animated<f64>,
    /// Blur radius in pixels. Takes keyframes.
    #[serde(default)]
    pub blur: Animated<f64>,
}

/// The object form of a text fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextFillFields {
    /// The colour or gradient, in CSS.
    pub gradient: String,
    /// The tile's width in pixels. Defaults to the text's own width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// The tile's height in pixels. Defaults to the text's own height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Where the tile starts, in pixels from the left of the text's box.
    /// Takes keyframes.
    #[serde(default)]
    pub x: Animated<f64>,
    /// Where the tile starts, in pixels from the top of the text's box.
    /// Takes keyframes.
    #[serde(default)]
    pub y: Animated<f64>,
}

/// The object form of an outline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StrokeFields {
    /// Outline color.
    pub color: ColorValue,
    /// Outline width in pixels.
    pub width: f64,
}

impl<'de> Deserialize<'de> for Shadow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ShadowVisitor;

        impl<'de> Visitor<'de> for ShadowVisitor {
            type Value = Shadow;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a shadow: {\"x\", \"y\", \"blur\", \"color\"} or a string like \"0 2px 8px #0008\"")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Shadow, E> {
                parse_shadow(v).map_err(E::custom)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Shadow, A::Error> {
                let f = ShadowFields::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(Shadow {
                    color: f.color,
                    x: f.x,
                    y: f.y,
                    blur: f.blur,
                })
            }
        }

        deserializer.deserialize_any(ShadowVisitor)
    }
}

impl JsonSchema for Shadow {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Shadow".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let object = ShadowFields::json_schema(generator);
        json_schema!({
            "title": "Shadow",
            "description": "A drop shadow: an object with x, y, blur and color, or the text-shadow shorthand \"<x> <y> [blur] [color]\" such as \"0 2px 8px #0008\".",
            "anyOf": [
                object,
                { "type": "string" }
            ]
        })
    }
}

impl<'de> Deserialize<'de> for TextFill {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FillVisitor;

        impl<'de> Visitor<'de> for FillVisitor {
            type Value = TextFill;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a fill: {\"gradient\", \"width\", \"height\", \"x\", \"y\"} or a gradient string",
                )
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<TextFill, E> {
                Ok(TextFill {
                    gradient: v.to_owned(),
                    width: None,
                    height: None,
                    x: Animated::Constant(0.0),
                    y: Animated::Constant(0.0),
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<TextFill, A::Error> {
                let f = TextFillFields::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(TextFill {
                    gradient: f.gradient,
                    width: f.width,
                    height: f.height,
                    x: f.x,
                    y: f.y,
                })
            }
        }

        deserializer.deserialize_any(FillVisitor)
    }
}

impl JsonSchema for TextFill {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TextFill".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let object = TextFillFields::json_schema(generator);
        json_schema!({
            "title": "TextFill",
            "description": "A gradient that fills the glyphs: an object with gradient, width, height, x and y, or the gradient string alone, such as \"linear-gradient(90deg, #7A51CF, #C28072)\".",
            "anyOf": [
                object,
                { "type": "string" }
            ]
        })
    }
}

impl<'de> Deserialize<'de> for Stroke {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrokeVisitor;

        impl<'de> Visitor<'de> for StrokeVisitor {
            type Value = Stroke;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an outline: {\"width\", \"color\"} or a string like \"2px black\"")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Stroke, E> {
                parse_stroke(v).map_err(E::custom)
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Stroke, A::Error> {
                let f = StrokeFields::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(Stroke {
                    color: f.color,
                    width: f.width,
                })
            }
        }

        deserializer.deserialize_any(StrokeVisitor)
    }
}

impl JsonSchema for Stroke {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Stroke".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let object = StrokeFields::json_schema(generator);
        json_schema!({
            "title": "Stroke",
            "description": "An outline: an object with width and color, or the shorthand \"<width> <color>\" such as \"2px black\".",
            "anyOf": [
                object,
                { "type": "string" }
            ]
        })
    }
}

/// Deserializes an optional pixel value given as a number or a string
/// such as `"8px"`.
pub fn de_opt_pixels<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    struct PixelsVisitor;

    impl Visitor<'_> for PixelsVisitor {
        type Value = Option<f64>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("pixels as a number or a string like \"8px\"")
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v as f64))
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v as f64))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            parse_padding(v).map(Some).map_err(E::custom)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
    }

    deserializer.deserialize_any(PixelsVisitor)
}

/// Schema for a pixel value as a number or a `"8px"` string.
pub fn pixels_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "description": "Pixels as a number (8) or a string (\"8px\").",
        "anyOf": [
            { "type": "number" },
            { "type": "string", "pattern": "^\\s*-?\\d+(\\.\\d+)?\\s*(px)?\\s*$" }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadows_parse_in_the_text_shadow_order() {
        // The shorthand can only say one value, so every part is a constant.
        let c = |a: &Animated<f64>| *a.constant().expect("a constant");
        let s = parse_shadow("0 2px 8px #0008").unwrap();
        assert_eq!((c(&s.x), c(&s.y), c(&s.blur)), (0.0, 2.0, 8.0));
        assert_eq!(s.color.unwrap().constant().unwrap().0.to_hex(), "#00000088");
        let s = parse_shadow("rgba(0, 0, 0, 0.5) 1px 1px").unwrap();
        assert_eq!((c(&s.x), c(&s.y), c(&s.blur)), (1.0, 1.0, 0.0));
        assert!(parse_shadow("2px").is_err());
        assert!(parse_shadow("0 0 -1px").is_err());
        assert!(parse_shadow("0 0 red blue").is_err());
    }

    #[test]
    fn outlines_take_either_order() {
        let a = parse_stroke("2px black").unwrap();
        let b = parse_stroke("black 2").unwrap();
        assert_eq!(a, b);
        assert!(parse_stroke("black").is_err());
        assert!(parse_stroke("-1px black").is_err());
    }

    #[test]
    fn font_shorthand_expands_and_plain_families_do_not() {
        let f = parse_font("italic 600 40px/1.2 Inter, sans-serif")
            .unwrap()
            .unwrap();
        assert_eq!(f.italic, Some(true));
        assert_eq!(f.weight, Some(600));
        assert_eq!(f.size, 40.0);
        assert_eq!(f.line_height, Some(1.2));
        assert_eq!(f.family, "Inter");
        let f = parse_font("bold 24px/36px 'Noto Sans'").unwrap().unwrap();
        assert_eq!(f.weight, Some(700));
        assert_eq!(f.line_height, Some(1.5));
        assert_eq!(f.family, "Noto Sans");
        assert!(parse_font("Inter").is_none());
        assert!(parse_font("Noto Sans").is_none());
        assert!(parse_font("40px").unwrap().is_err());
        assert!(parse_font("bolder 40px Inter").unwrap().is_err());

        let mut style = TextStyle {
            font: Some("600 40px/1.2 Inter".to_owned()),
            size: Some(30.0),
            ..TextStyle::default()
        };
        let mut lh = None;
        expand_font(&mut style, &mut lh).unwrap();
        assert_eq!(style.font.as_deref(), Some("Inter"));
        assert_eq!(style.size, Some(30.0), "an explicit size wins");
        assert_eq!(style.weight, Some(600));
        assert_eq!(lh, Some(1.2));
    }

    #[test]
    fn padding_is_one_length() {
        assert_eq!(parse_padding("8px").unwrap(), 8.0);
        assert_eq!(parse_padding("8").unwrap(), 8.0);
        assert!(parse_padding("8px 4px").is_err());
        assert!(parse_padding("-2px").is_err());
    }
}
