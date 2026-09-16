use std::fmt;

use geneva_anim::Easing;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, DeserializeOwned, Deserializer, IntoDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::time::Time;

/// One keyframe as written in a timeline.
///
/// JSON forms: the object (`{"t": 0, "v": 1, "ease": "ease-out"}`) or the
/// same three in order (`[0, 1, "ease-out"]`, or `[0, 1]` for linear).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyframeSpec<T> {
    /// Time of the keyframe, relative to the start of the clip.
    pub t: Time,
    /// Value at that time.
    pub v: T,
    /// Easing applied between this keyframe and the next. Defaults to linear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Easing>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for KeyframeSpec<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The canonical object form, kept on the derive so its field errors
        /// keep their paths and spelling.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, bound = "T: Deserialize<'de>")]
        struct Object<T> {
            t: Time,
            v: T,
            #[serde(default)]
            ease: Option<Easing>,
        }

        struct KeyframeVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for KeyframeVisitor<T> {
            type Value = KeyframeSpec<T>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a keyframe: {\"t\": .., \"v\": ..} or [t, v] or [t, v, ease]")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let o = Object::<T>::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(KeyframeSpec {
                    t: o.t,
                    v: o.v,
                    ease: o.ease,
                })
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let t: Time = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a keyframe needs a time and a value"))?;
                let v: T = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a keyframe needs a time and a value"))?;
                let ease: Option<Easing> = seq.next_element()?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(
                        "a keyframe in list form is [t, v] or [t, v, ease], nothing longer",
                    ));
                }
                Ok(KeyframeSpec { t, v, ease })
            }
        }

        deserializer.deserialize_any(KeyframeVisitor(std::marker::PhantomData))
    }
}

impl<T: JsonSchema> JsonSchema for KeyframeSpec<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("Keyframe_{}", T::schema_name()).into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        format!("geneva::KeyframeSpec<{}>", T::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let value = generator.subschema_for::<T>();
        let time = generator.subschema_for::<Time>();
        let ease = generator.subschema_for::<Easing>();
        json_schema!({
            "description": "One keyframe: {\"t\": .., \"v\": .., \"ease\": ..} or the same three in order as [t, v] or [t, v, ease].",
            "anyOf": [
                {
                    "type": "object",
                    "properties": { "t": time, "v": value, "ease": ease },
                    "required": ["t", "v"],
                    "additionalProperties": false
                },
                { "type": "array", "minItems": 2, "maxItems": 3 }
            ]
        })
    }
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

impl<T: Default> Default for Animated<T> {
    /// The type's own default, as a constant. Lets a field default to
    /// zero or none and still accept keyframes.
    fn default() -> Self {
        Self::Constant(T::default())
    }
}

impl<T> Animated<T> {
    /// Applies `f` to every value the property takes, keeping the
    /// keyframe times and easing.
    pub fn map<U>(&self, f: impl Fn(&T) -> U) -> Animated<U> {
        match self {
            Self::Constant(v) => Animated::Constant(f(v)),
            Self::Keyframes(k) => Animated::Keyframes(
                k.iter()
                    .map(|k| KeyframeSpec {
                        t: k.t,
                        v: f(&k.v),
                        ease: k.ease.clone(),
                    })
                    .collect(),
            ),
        }
    }

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
    fn keyframes_in_list_form() {
        let long: Animated<f64> = serde_json::from_str(
            r#"{"keyframes":[{"t":0,"v":0},{"t":"0.3s","v":1,"ease":"ease-out"}]}"#,
        )
        .unwrap();
        let short: Animated<f64> =
            serde_json::from_str(r#"{"keyframes":[[0,0],["0.3s",1,"ease-out"]]}"#).unwrap();
        assert_eq!(short, long);

        // A point value keeps its own shorthand inside the list form.
        let p: Animated<Point> =
            serde_json::from_str(r#"{"keyframes":[[0,"-600 648","ease-out"],["0.5s",[56,648]]]}"#)
                .unwrap();
        let k = p.keyframes().unwrap();
        assert_eq!(k[0].v.to_px(0.0, 0.0), [-600.0, 648.0]);
        assert_eq!(k[1].v.to_px(0.0, 0.0), [56.0, 648.0]);

        assert!(serde_json::from_str::<Animated<f64>>(r#"{"keyframes":[[0]]}"#).is_err());
        assert!(
            serde_json::from_str::<Animated<f64>>(r#"{"keyframes":[[0,1,"linear",2]]}"#).is_err()
        );
    }

    #[test]
    fn list_keyframe_errors_keep_their_path() {
        let json = r#"{"keyframes":[[0,0],["1s","big"]]}"#;
        let mut de = serde_json::Deserializer::from_str(json);
        let err = serde_path_to_error::deserialize::<_, Animated<f64>>(&mut de).unwrap_err();
        assert_eq!(err.path().to_string(), "keyframes[1][1]");
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
        // The list form is an input spelling; printed timelines use objects.
        let a: Animated<f64> = serde_json::from_str(r#"{"keyframes":[[0,0],["1s",1]]}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            r#"{"keyframes":[{"t":"0s","v":0.0},{"t":"1s","v":1.0}]}"#
        );
    }
}
