use std::fmt;

use geneva_anim::Easing;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, DeserializeOwned, Deserializer, IntoDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::time::Time;

/// One keyframe as written in a timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyframeSpec<T> {
    /// Time of the keyframe, relative to the start of the clip.
    pub t: Time,
    /// Value at that time.
    pub v: T,
    /// Easing applied between this keyframe and the next. Defaults to linear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Easing>,
}

/// A property that is either constant or driven by keyframes.
///
/// JSON forms: the plain value (`0.5`), or an object with a `keyframes` array
/// (`{"keyframes": [{"t": 0, "v": 0}, {"t": "1s", "v": 1, "ease": "ease-out"}]}`).
#[derive(Debug, Clone, PartialEq)]
pub enum Animated<T> {
    /// A value that does not change over the clip.
    Constant(T),
    /// A value driven by keyframes.
    Keyframes(Vec<KeyframeSpec<T>>),
}

impl<T> Animated<T> {
    /// Returns the constant value, if any.
    pub fn constant(&self) -> Option<&T> {
        match self {
            Self::Constant(v) => Some(v),
            Self::Keyframes(_) => None,
        }
    }

    /// Returns the keyframes, if any.
    pub fn keyframes(&self) -> Option<&[KeyframeSpec<T>]> {
        match self {
            Self::Constant(_) => None,
            Self::Keyframes(k) => Some(k),
        }
    }

    /// Iterates over every value the property takes.
    pub fn values(&self) -> impl Iterator<Item = &T> {
        let (single, many): (Option<&T>, &[KeyframeSpec<T>]) = match self {
            Self::Constant(v) => (Some(v), &[]),
            Self::Keyframes(k) => (None, k),
        };
        single.into_iter().chain(many.iter().map(|k| &k.v))
    }
}

impl<T: Serialize> Serialize for Animated<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Constant(v) => v.serialize(serializer),
            Self::Keyframes(k) => {
                #[derive(Serialize)]
                struct Wrapper<'a, T> {
                    keyframes: &'a [KeyframeSpec<T>],
                }
                Wrapper { keyframes: k }.serialize(serializer)
            }
        }
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Animated<T> {
    /// Objects whose first key is `keyframes` are parsed as keyframe lists
    /// through the live deserializer, so errors inside them keep their exact
    /// path. Any other input is buffered and handed to `T`.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AnimatedVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: DeserializeOwned> Visitor<'de> for AnimatedVisitor<T> {
            type Value = Animated<T>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a constant value or an object with a \"keyframes\" array")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let Some(first) = map.next_key::<String>()? else {
                    return T::deserialize(Value::Object(Map::new()).into_deserializer())
                        .map(Animated::Constant)
                        .map_err(constant_error);
                };
                if first == "keyframes" {
                    let keys = map.next_value::<Vec<KeyframeSpec<T>>>()?;
                    if let Some(extra) = map.next_key::<String>()? {
                        return Err(de::Error::custom(format!(
                            "unexpected field \"{extra}\" next to \"keyframes\"; an animated property has no other fields"
                        )));
                    }
                    if keys.is_empty() {
                        return Err(de::Error::custom(
                            "\"keyframes\" must contain at least one keyframe",
                        ));
                    }
                    return Ok(Animated::Keyframes(keys));
                }
                let mut object = Map::new();
                object.insert(first, map.next_value::<Value>()?);
                while let Some(key) = map.next_key::<String>()? {
                    object.insert(key, map.next_value::<Value>()?);
                }
                T::deserialize(Value::Object(object).into_deserializer())
                    .map(Animated::Constant)
                    .map_err(constant_error)
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                constant::<T, E>(Value::Bool(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                constant::<T, E>(Value::from(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                constant::<T, E>(Value::from(v))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                constant::<T, E>(Value::from(v))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                constant::<T, E>(Value::from(v))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                constant::<T, E>(Value::Null)
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<Value>()? {
                    items.push(item);
                }
                constant::<T, A::Error>(Value::Array(items))
            }
        }

        fn constant<T: DeserializeOwned, E: de::Error>(value: Value) -> Result<Animated<T>, E> {
            T::deserialize(value.into_deserializer())
                .map(Animated::Constant)
                .map_err(constant_error)
        }

        fn constant_error<E: de::Error>(e: serde_json::Error) -> E {
            E::custom(e)
        }

        deserializer.deserialize_any(AnimatedVisitor(std::marker::PhantomData))
    }
}

impl<T: JsonSchema> JsonSchema for Animated<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Animated_{}", T::schema_name()).into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        format!("geneva::Animated<{}>", T::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let value = generator.subschema_for::<T>();
        let keyframe = generator.subschema_for::<KeyframeSpec<T>>();
        json_schema!({
            "description": "Either a constant value or {\"keyframes\": [...]} with values interpolated over time.",
            "anyOf": [
                value,
                {
                    "type": "object",
                    "properties": {
                        "keyframes": { "type": "array", "items": keyframe, "minItems": 1 }
                    },
                    "required": ["keyframes"],
                    "additionalProperties": false
                }
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::length::Point;

    #[test]
    fn parses_constants_and_keyframes() {
        let a: Animated<f64> = serde_json::from_str("0.5").unwrap();
        assert_eq!(a, Animated::Constant(0.5));
        let a: Animated<f64> = serde_json::from_str(
            r#"{"keyframes":[{"t":0,"v":0},{"t":"1s","v":1,"ease":"ease-in"}]}"#,
        )
        .unwrap();
        assert_eq!(a.keyframes().unwrap().len(), 2);
        let a: Animated<Point> = serde_json::from_str(r#"{"x": 1, "y": "50%"}"#).unwrap();
        assert!(a.constant().is_some());
    }

    #[test]
    fn keyframe_errors_keep_their_path() {
        let json = r#"{"keyframes":[{"t":0,"v":0},{"t":"1s","v":"big"}]}"#;
        let mut de = serde_json::Deserializer::from_str(json);
        let err = serde_path_to_error::deserialize::<_, Animated<f64>>(&mut de).unwrap_err();
        assert_eq!(err.path().to_string(), "keyframes[1].v");
    }

    #[test]
    fn rejects_extra_fields_and_empty_lists() {
        assert!(serde_json::from_str::<Animated<f64>>(r#"{"keyframes":[]}"#).is_err());
        let err = serde_json::from_str::<Animated<f64>>(r#"{"keyframes":[{"t":0,"v":0}],"x":1}"#)
            .unwrap_err();
        assert!(err.to_string().contains("unexpected field \"x\""));
        let err = serde_json::from_str::<Animated<Point>>(r#"{"x": 1}"#).unwrap_err();
        assert!(err.to_string().contains("missing field `y`"));
    }

    #[test]
    fn serializes_back_to_the_same_shape() {
        let a: Animated<f64> =
            serde_json::from_str(r#"{"keyframes":[{"t":0,"v":0},{"t":"1s","v":1}]}"#).unwrap();
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(
            s,
            r#"{"keyframes":[{"t":"0s","v":0.0},{"t":"1s","v":1.0}]}"#
        );
        assert_eq!(
            serde_json::to_string(&Animated::Constant(2.5)).unwrap(),
            "2.5"
        );
    }
}
