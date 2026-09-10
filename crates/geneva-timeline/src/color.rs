use std::fmt;
use std::str::FromStr;

use geneva_color::Color;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// A color as written in a timeline: `"#rrggbb"`, `"#rrggbbaa"`, `"rgb()"`,
/// `"rgba()"` or a basic color name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorValue(pub Color);

impl From<Color> for ColorValue {
    fn from(c: Color) -> Self {
        Self(c)
    }
}

impl fmt::Display for ColorValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_hex())
    }
}

impl Serialize for ColorValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_hex())
    }
}

impl<'de> Deserialize<'de> for ColorValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColorVisitor;

        impl Visitor<'_> for ColorVisitor {
            type Value = ColorValue;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a color string like \"#ff8800\", \"#ff880080\", \"rgba(255, 136, 0, 0.5)\" or \"white\"")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<ColorValue, E> {
                Color::from_str(v).map(ColorValue).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(ColorVisitor)
    }
}

impl JsonSchema for ColorValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Color".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "title": "Color",
            "description": "An sRGB color: \"#rgb\", \"#rrggbb\", \"#rrggbbaa\", \"rgb(r, g, b)\", \"rgba(r, g, b, a)\" or a basic name (black, white, red, green, blue, yellow, cyan, magenta, gray, orange, transparent).",
            "type": "string"
        })
    }
}
