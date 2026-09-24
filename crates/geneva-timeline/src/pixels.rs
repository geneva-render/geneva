//! Fields measured in pixels read a number (`8`) or a CSS length (`"8px"`).
//!
//! The fields themselves stay `f64` or `u32` and read through the functions
//! here, so nothing past parsing sees a unit, and a printed timeline writes
//! the plain number, which stays the canonical form. Fields that also take
//! a percentage are a [`Length`](crate::Length) instead.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

use crate::Animated;

/// A number of pixels: `8`, `-2.5`, `"8px"` or `"8"`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Px(pub f64);

impl Px {
    /// Parses the string form.
    pub fn parse(s: &str) -> Result<f64, String> {
        let t = s.trim();
        if t.ends_with('%') {
            return Err(format!(
                "{s:?}: this field is in pixels and takes no percentage; write 8 or \"8px\""
            ));
        }
        t.strip_suffix("px")
            .unwrap_or(t)
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{s:?} is not a number of pixels; write 8 or \"8px\""))
    }
}

impl Serialize for Px {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for Px {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PxVisitor;

        impl Visitor<'_> for PxVisitor {
            type Value = Px;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("pixels as a number or a string like \"8px\"")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Px, E> {
                if !v.is_finite() {
                    return Err(E::custom("pixels must be finite"));
                }
                Ok(Px(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Px, E> {
                Ok(Px(v as f64))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Px, E> {
                Ok(Px(v as f64))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Px, E> {
                Px::parse(v).map(Px).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(PxVisitor)
    }
}

impl JsonSchema for Px {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Pixels".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "title": "Pixels",
            "description": "Pixels as a number (8) or a string (\"8px\").",
            "anyOf": [
                { "type": "number" },
                { "type": "string", "pattern": "^\\s*-?\\d+(\\.\\d+)?\\s*(px)?\\s*$" }
            ]
        })
    }
}

/// A whole number of pixels for a frame or picture size: `1920` or
/// `"1920px"`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct WholePx(u32);

impl<'de> Deserialize<'de> for WholePx {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let Px(v) = Px::deserialize(deserializer)?;
        if v.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&v) {
            return Err(de::Error::custom(format!(
                "{v} is not a size in whole pixels; write 1920 or \"1920px\""
            )));
        }
        Ok(WholePx(v as u32))
    }
}

impl JsonSchema for WholePx {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WholePixels".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "title": "WholePixels",
            "description": "A size in whole pixels, as a number (1920) or a string (\"1920px\").",
            "anyOf": [
                { "type": "integer", "minimum": 0 },
                { "type": "string", "pattern": "^\\s*\\d+\\s*(px)?\\s*$" }
            ]
        })
    }
}

/// Reads a required pixel value.
pub fn de_px<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    Px::deserialize(deserializer).map(|p| p.0)
}

/// Reads an optional pixel value.
pub fn de_opt_px<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    Option::<Px>::deserialize(deserializer).map(|p| p.map(|p| p.0))
}

/// Reads a pixel value that takes keyframes; each keyframe's value may
/// be written either way too.
pub fn de_animated_px<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Animated<f64>, D::Error> {
    Animated::<Px>::deserialize(deserializer).map(|a| a.map(|p| p.0))
}

/// Reads a required frame or picture size.
pub fn de_whole_px<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    WholePx::deserialize(deserializer).map(|p| p.0)
}

/// Reads an optional frame or picture size.
pub fn de_opt_whole_px<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    Option::<WholePx>::deserialize(deserializer).map(|p| p.map(|p| p.0))
}

/// Schema for [`de_px`].
pub fn px_schema(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<Px>()
}

/// Schema for [`de_opt_px`]: a missing value may also be written `null`.
pub fn opt_px_schema(generator: &mut SchemaGenerator) -> Schema {
    nullable(&generator.subschema_for::<Px>())
}

/// Schema for [`de_animated_px`].
pub fn animated_px_schema(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<Animated<Px>>()
}

/// Schema for [`de_whole_px`].
pub fn whole_px_schema(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<WholePx>()
}

/// Schema for [`de_opt_whole_px`].
pub fn opt_whole_px_schema(generator: &mut SchemaGenerator) -> Schema {
    nullable(&generator.subschema_for::<WholePx>())
}

fn nullable(schema: &Schema) -> Schema {
    json_schema!({ "anyOf": [schema, { "type": "null" }] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn pixels_read_as_numbers_or_css_lengths() {
        for (text, want) in [
            ("8", 8.0),
            ("\"8px\"", 8.0),
            ("\" 2.5 px \"", 2.5),
            ("-3", -3.0),
        ] {
            let got: Px = serde_json::from_str(text).unwrap();
            assert_eq!(got.0, want, "{text}");
        }
        for text in ["\"50%\"", "\"8em\"", "\"px\"", "\"\"", "\"inf\"", "true"] {
            assert!(serde_json::from_str::<Px>(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_percentage_is_refused_with_the_reason() {
        let err = serde_json::from_str::<Px>("\"50%\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("takes no percentage"), "{err}");
    }

    #[test]
    fn keyframe_values_take_either_form() {
        let json = r#"{"keyframes": [[0, "0px"], ["1s", 12]]}"#;
        let a = de_animated_px(&mut serde_json::Deserializer::from_str(json)).unwrap();
        let v: Vec<f64> = a.values().copied().collect();
        assert_eq!(v, [0.0, 12.0]);
    }

    #[test]
    fn frame_sizes_are_whole_pixels() {
        let read = |t: &str| de_whole_px(&mut serde_json::Deserializer::from_str(t));
        assert_eq!(read("1920").unwrap(), 1920);
        assert_eq!(read("\"1080px\"").unwrap(), 1080);
        assert!(read("\"1080.5px\"").is_err());
        assert!(read("-4").is_err());
    }

    /// One document with every pixel field written as a CSS length, keyframe
    /// values included, and the text style's fields going through the
    /// flattened text source.
    const AS_PX: &str = r##"{
      "geneva": "0.5",
      "output": { "width": "640px", "height": 360, "fps": 30, "duration": "1s", "background": "#223344" },
      "outputs": { "poster": { "kind": "poster", "width": "320px", "at": "0.5s" } },
      "layers": [
        { "clips": [ {
            "source": { "kind": "shape", "shape": "rect", "width": 200, "height": 100, "radius": "12px",
                        "stroke": { "color": "white", "width": "2px" } },
            "mask": { "radius": "8px", "feather": "4px" },
            "effects": [ { "kind": "blur", "radius": { "keyframes": [[0, "0px"], ["1s", "3px"]] } } ] } ] },
        { "clips": [ {
            "source": { "kind": "text", "text": "pixels",
              "font": "500 34px/1.35 Liberation Sans", "size": "30px", "letter_spacing": "1.5px",
              "background": "#0a0f14cc", "padding": "14px", "radius": "3px",
              "outline": { "color": "black", "width": "1px" },
              "fill": { "gradient": "linear-gradient(90deg, red, blue)", "width": "400px", "height": "40px",
                        "x": { "keyframes": [[0, "0px"], ["1s", "-200px"]] }, "y": "0px" },
              "shadow": [ { "x": "0px", "y": "2px", "blur": { "keyframes": [[0, 0], ["1s", "8px"]] }, "color": "#0008" } ] } } ] }
      ]
    }"##;

    /// The same document with each of those values written as the number.
    const AS_NUMBERS: &str = r##"{
      "geneva": "0.5",
      "output": { "width": 640, "height": 360, "fps": 30, "duration": "1s", "background": "#223344" },
      "outputs": { "poster": { "kind": "poster", "width": 320, "at": "0.5s" } },
      "layers": [
        { "clips": [ {
            "source": { "kind": "shape", "shape": "rect", "width": 200, "height": 100, "radius": 12,
                        "stroke": { "color": "white", "width": 2 } },
            "mask": { "radius": 8, "feather": 4 },
            "effects": [ { "kind": "blur", "radius": { "keyframes": [[0, 0], ["1s", 3]] } } ] } ] },
        { "clips": [ {
            "source": { "kind": "text", "text": "pixels",
              "font": "500 34px/1.35 Liberation Sans", "size": 30, "letter_spacing": 1.5,
              "background": "#0a0f14cc", "padding": "14px", "radius": 3,
              "outline": { "color": "black", "width": 1 },
              "fill": { "gradient": "linear-gradient(90deg, red, blue)", "width": 400, "height": 40,
                        "x": { "keyframes": [[0, 0], ["1s", -200]] }, "y": 0 },
              "shadow": [ { "x": 0, "y": 2, "blur": { "keyframes": [[0, 0], ["1s", 8]] }, "color": "#0008" } ] } } ] }
      ]
    }"##;

    #[test]
    fn a_pixel_field_reads_the_same_either_way() {
        let px = parse(AS_PX).expect("the px spelling parses");
        let numbers = parse(AS_NUMBERS).expect("the number spelling parses");
        assert_eq!(px, numbers);
    }

    #[test]
    fn a_printed_document_writes_the_numbers() {
        let printed = serde_json::to_string(&parse(AS_PX).unwrap()).unwrap();
        assert!(!printed.contains("px\""), "{printed}");
    }

    #[test]
    fn printed_as_the_plain_number() {
        assert_eq!(serde_json::to_string(&Px(8.0)).unwrap(), "8.0");
    }
}
