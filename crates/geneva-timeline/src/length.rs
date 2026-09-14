use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// A length in output pixels or as a percentage of a reference dimension.
///
/// JSON forms: a number (`120`, pixels) or a string with a unit (`"120px"`,
/// `"50%"`). Percentages of positions and sizes refer to the output width for
/// horizontal values and the output height for vertical ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    /// Pixels in output space.
    Px(f64),
    /// Percent of the reference dimension; `50.0` is half.
    Percent(f64),
}

impl Length {
    /// Resolves to pixels against `reference` (the size a percentage refers to).
    pub fn to_px(self, reference: f64) -> f64 {
        match self {
            Self::Px(v) => v,
            Self::Percent(p) => reference * p / 100.0,
        }
    }

    /// Returns true when both values use the same unit.
    pub fn same_unit(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Px(_), Self::Px(_)) | (Self::Percent(_), Self::Percent(_))
        )
    }

    /// The numeric part regardless of unit.
    pub fn value(self) -> f64 {
        match self {
            Self::Px(v) | Self::Percent(v) => v,
        }
    }

    /// Parses `"120"`, `"120px"` or `"50%"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (body, ctor): (&str, fn(f64) -> Self) = if let Some(b) = s.strip_suffix('%') {
            (b, Self::Percent)
        } else if let Some(b) = s.strip_suffix("px") {
            (b, Self::Px)
        } else {
            (s, Self::Px)
        };
        let v: f64 = body.trim().parse().map_err(|_| {
            format!(
                "invalid length {s:?}; use a number of pixels like 120 or a percentage like \"50%\""
            )
        })?;
        if !v.is_finite() {
            return Err("length must be finite".to_owned());
        }
        Ok(ctor(v))
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Px(v) => write!(f, "{v}px"),
            Self::Percent(v) => write!(f, "{v}%"),
        }
    }
}

impl Serialize for Length {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Px(v) => serializer.serialize_f64(*v),
            Self::Percent(_) => serializer.serialize_str(&self.to_string()),
        }
    }
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LengthVisitor;

        impl Visitor<'_> for LengthVisitor {
            type Value = Length;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a length: pixels as a number, or a string like \"120px\" or \"50%\"")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Length, E> {
                if !v.is_finite() {
                    return Err(E::custom("length must be finite"));
                }
                Ok(Length::Px(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Length, E> {
                Ok(Length::Px(v as f64))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Length, E> {
                Ok(Length::Px(v as f64))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Length, E> {
                Length::parse(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(LengthVisitor)
    }
}

impl JsonSchema for Length {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Length".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "title": "Length",
            "description": "Pixels as a number (120) or a string with a unit (\"120px\", \"50%\"). Percentages refer to the output width for x values and the output height for y values.",
            "anyOf": [
                { "type": "number" },
                { "type": "string", "pattern": "^\\s*-?\\d+(\\.\\d+)?\\s*(px|%)?\\s*$" }
            ]
        })
    }
}

/// A 2D point or size with independent units per axis.
///
/// JSON forms: the object (`{"x": 30, "y": 36}`), a pair (`[30, 36]`), or a
/// string of one or two components (`"30 36"`, `"0% 50%"`, `"left"`,
/// `"bottom right"`). A component is a length or one of the CSS keywords
/// `left`, `right`, `top`, `bottom`, `center`. A single length applies to
/// both axes; a single keyword centers the axis it says nothing about.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    /// Horizontal component; percentages refer to the output width.
    pub x: Length,
    /// Vertical component; percentages refer to the output height.
    pub y: Length,
}

/// The axis a component of a shorthand point lands on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

/// The two components a CSS position keyword fixes, if any.
fn keyword(token: &str) -> Option<(Option<Length>, Option<Length>)> {
    const ZERO: Length = Length::Percent(0.0);
    const HALF: Length = Length::Percent(50.0);
    const FULL: Length = Length::Percent(100.0);
    Some(match token {
        "left" => (Some(ZERO), None),
        "right" => (Some(FULL), None),
        "top" => (None, Some(ZERO)),
        "bottom" => (None, Some(FULL)),
        "center" => (Some(HALF), Some(HALF)),
        _ => return None,
    })
}

/// One component of a shorthand point: a keyword that fixes `axis`, or a
/// length.
fn component(token: &str, axis: Axis) -> Result<Length, String> {
    match keyword(token) {
        Some((x, y)) => {
            let (taken, other) = match axis {
                Axis::X => (x, "vertical"),
                Axis::Y => (y, "horizontal"),
            };
            taken.ok_or_else(|| format!("{token:?} positions the {other} axis"))
        }
        None => Length::parse(token),
    }
}

const POINT_FORMS: &str = "write two components like \"30 36\" or \"0% 50%\", a keyword like \"left\" or \"bottom right\", or an object {\"x\": .., \"y\": ..}";

impl Point {
    /// Resolves both components to pixels against an output size.
    pub fn to_px(self, width: f64, height: f64) -> [f64; 2] {
        [self.x.to_px(width), self.y.to_px(height)]
    }

    /// Parses the shorthand string form.
    pub fn parse(s: &str) -> Result<Self, String> {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        let (a, b) = match tokens.as_slice() {
            [one] => {
                if let Some((x, y)) = keyword(one) {
                    return Ok(Self {
                        x: x.unwrap_or(Length::Percent(50.0)),
                        y: y.unwrap_or(Length::Percent(50.0)),
                    });
                }
                let l = Length::parse(one)
                    .map_err(|_| format!("invalid point {s:?}; {POINT_FORMS}"))?;
                return Ok(Self { x: l, y: l });
            }
            // "top left" reads as naturally as "left top", so a component
            // that can only be vertical, or a second one that can only be
            // horizontal, puts the pair back in x-then-y order.
            [a, b] if matches!(*a, "top" | "bottom") || matches!(*b, "left" | "right") => (*b, *a),
            [a, b] => (*a, *b),
            _ => return Err(format!("invalid point {s:?}; {POINT_FORMS}")),
        };
        let x = component(a, Axis::X).map_err(|e| format!("{e} in point {s:?}; {POINT_FORMS}"))?;
        let y = component(b, Axis::Y).map_err(|e| format!("{e} in point {s:?}; {POINT_FORMS}"))?;
        Ok(Self { x, y })
    }
}

impl JsonSchema for Point {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Point".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let length = generator.subschema_for::<Length>();
        json_schema!({
            "title": "Point",
            "description": "A 2D point: {\"x\": .., \"y\": ..}, a pair like [30, 36], or a string of one or two components (\"30 36\", \"0% 50%\", \"left\", \"bottom right\"). A component is a length or one of left, right, top, bottom, center.",
            "anyOf": [
                {
                    "type": "object",
                    "properties": { "x": length, "y": length },
                    "required": ["x", "y"],
                    "additionalProperties": false
                },
                { "type": "array", "items": length, "minItems": 2, "maxItems": 2 },
                { "type": "string" }
            ]
        })
    }
}

impl<'de> Deserialize<'de> for Point {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The canonical object form, kept on the derive so its field errors
        /// keep their paths and spelling.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Object {
            x: Length,
            y: Length,
        }

        struct PointVisitor;

        impl<'de> Visitor<'de> for PointVisitor {
            type Value = Point;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a point: {\"x\": .., \"y\": ..}, a pair like [30, 36], or a string like \"30 36\" or \"bottom right\"",
                )
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Point, E> {
                Point::parse(v).map_err(E::custom)
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Point, A::Error> {
                let o = Object::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(Point { x: o.x, y: o.y })
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Point, A::Error> {
                let x: Length = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a point pair needs two components"))?;
                let y: Length = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a point pair needs two components"))?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(
                        "a point pair has two components, not more",
                    ));
                }
                Ok(Point { x, y })
            }
        }

        deserializer.deserialize_any(PointVisitor)
    }
}

/// A scale factor: one number applied to both axes, or separate factors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    /// Horizontal factor.
    pub x: f64,
    /// Vertical factor.
    pub y: f64,
}

impl Scale {
    /// Identity scale.
    pub const ONE: Self = Self { x: 1.0, y: 1.0 };

    /// The factors as an array.
    pub fn to_array(self) -> [f64; 2] {
        [self.x, self.y]
    }
}

impl Serialize for Scale {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if (self.x - self.y).abs() < f64::EPSILON {
            serializer.serialize_f64(self.x)
        } else {
            #[derive(Serialize)]
            struct Xy {
                x: f64,
                y: f64,
            }
            Xy {
                x: self.x,
                y: self.y,
            }
            .serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for Scale {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ScaleVisitor;

        impl<'de> Visitor<'de> for ScaleVisitor {
            type Value = Scale;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a scale: a number like 1.5, or an object {\"x\": 1, \"y\": 2}")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Scale, E> {
                if !v.is_finite() {
                    return Err(E::custom("scale must be finite"));
                }
                Ok(Scale { x: v, y: v })
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Scale, E> {
                Ok(Scale {
                    x: v as f64,
                    y: v as f64,
                })
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Scale, E> {
                Ok(Scale {
                    x: v as f64,
                    y: v as f64,
                })
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Scale, A::Error> {
                let (mut x, mut y) = (None, None);
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "x" => x = Some(map.next_value::<f64>()?),
                        "y" => y = Some(map.next_value::<f64>()?),
                        other => {
                            return Err(de::Error::unknown_field(other, &["x", "y"]));
                        }
                    }
                }
                Ok(Scale {
                    x: x.ok_or_else(|| de::Error::missing_field("x"))?,
                    y: y.ok_or_else(|| de::Error::missing_field("y"))?,
                })
            }
        }

        deserializer.deserialize_any(ScaleVisitor)
    }
}

impl JsonSchema for Scale {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Scale".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "title": "Scale",
            "description": "A uniform scale factor (1.5) or separate factors per axis ({\"x\": 1, \"y\": 2}). 1 is natural size.",
            "anyOf": [
                { "type": "number" },
                {
                    "type": "object",
                    "properties": { "x": { "type": "number" }, "y": { "type": "number" } },
                    "required": ["x", "y"],
                    "additionalProperties": false
                }
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lengths() {
        assert_eq!(Length::parse("50%").unwrap(), Length::Percent(50.0));
        assert_eq!(Length::parse("120px").unwrap(), Length::Px(120.0));
        assert_eq!(Length::parse("-3.5").unwrap(), Length::Px(-3.5));
        assert!(Length::parse("12em").is_err());
    }

    #[test]
    fn json_forms() {
        let p: Point = serde_json::from_str(r#"{"x": 10, "y": "50%"}"#).unwrap();
        assert_eq!(p.to_px(1920.0, 1080.0), [10.0, 540.0]);
        let s: Scale = serde_json::from_str("2").unwrap();
        assert_eq!(s, Scale { x: 2.0, y: 2.0 });
        let s: Scale = serde_json::from_str(r#"{"x": 1, "y": 0.5}"#).unwrap();
        assert_eq!(s, Scale { x: 1.0, y: 0.5 });
        assert!(serde_json::from_str::<Scale>(r#"{"x": 1}"#).is_err());
        assert!(serde_json::from_str::<Point>(r#"{"x": 1, "y": 2, "z": 3}"#).is_err());
    }

    fn point(json: &str) -> Point {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn point_shorthands() {
        let object = point(r#"{"x": 30, "y": 36}"#);
        assert_eq!(point(r#""30 36""#), object);
        assert_eq!(point("[30, 36]"), object);
        assert_eq!(point(r#""30px 36px""#), object);
        assert_eq!(
            point(r#""0% 50%""#),
            Point {
                x: Length::Percent(0.0),
                y: Length::Percent(50.0)
            }
        );
        // One length goes on both axes.
        assert_eq!(point("\"12\""), point(r#"{"x": 12, "y": 12}"#));
    }

    #[test]
    fn point_keywords() {
        assert_eq!(point(r#""left""#), point(r#"{"x": "0%", "y": "50%"}"#));
        assert_eq!(point(r#""bottom""#), point(r#"{"x": "50%", "y": "100%"}"#));
        assert_eq!(point(r#""center""#), point(r#"{"x": "50%", "y": "50%"}"#));
        let corner = point(r#"{"x": "100%", "y": "0%"}"#);
        assert_eq!(point(r#""right top""#), corner);
        assert_eq!(point(r#""top right""#), corner);
        // A keyword names its own axis, so it can pair with a length.
        assert_eq!(point(r#""left 36""#), point(r#"{"x": "0%", "y": 36}"#));
        assert_eq!(point(r#""center 36""#), point(r#"{"x": "50%", "y": 36}"#));
    }

    #[test]
    fn point_shorthand_errors() {
        for bad in [
            r#""left right""#,
            r#""top bottom""#,
            r#""1 2 3""#,
            r#""""#,
            "[1]",
            "[1,2,3]",
        ] {
            assert!(
                serde_json::from_str::<Point>(bad).is_err(),
                "{bad} should not parse"
            );
        }
        let err = serde_json::from_str::<Point>(r#""left right""#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("horizontal axis"), "{err}");
        // Either order of a keyword pair, and a keyword with a length.
        assert_eq!(point(r#""top 36""#), point(r#""36 top""#));
    }
}
