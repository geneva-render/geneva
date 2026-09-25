use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

use crate::ratio::Ratio;

/// A point in time or a duration as written in a timeline.
///
/// Accepted JSON forms:
///
/// - a number, in seconds: `1.5`
/// - a string with a unit: `"1.5s"`, `"1500ms"`, `"45f"` (frames at the
///   output rate)
/// - a timecode: `"00:01:02.500"` or `"1:02.5"`
///
/// Frame counts stay symbolic until the output frame rate is known; see
/// [`Time::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Time {
    /// Seconds, exact.
    Seconds(Ratio),
    /// A count of frames at the output frame rate.
    Frames(i64),
}

impl Time {
    /// Zero seconds.
    pub const ZERO: Self = Self::Seconds(Ratio::ZERO);

    /// A time in whole seconds.
    pub const fn seconds(s: i64) -> Self {
        Self::Seconds(Ratio::from_int(s))
    }

    /// Converts to seconds given the output frame rate.
    pub fn resolve(self, fps: Ratio) -> Ratio {
        match self {
            Self::Seconds(s) => s,
            Self::Frames(n) => Ratio::from_int(n) / fps,
        }
    }

    /// Returns true for negative values.
    pub fn is_negative(self) -> bool {
        match self {
            Self::Seconds(s) => s.is_negative(),
            Self::Frames(n) => n < 0,
        }
    }

    /// Parses the string forms described on [`Time`].
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty time; use a number of seconds like 1.5 or \"1.5s\"".to_owned());
        }
        if s.contains(':') {
            return parse_timecode(s).map(Self::Seconds);
        }
        if let Some(message) = spelled_unit(s) {
            return Err(message);
        }
        if let Some(body) = s.strip_suffix("ms") {
            let v = parse_decimal(body)?;
            return Ok(Self::Seconds(v / Ratio::from_int(1000)));
        }
        if let Some(body) = s.strip_suffix('s') {
            return parse_decimal(body).map(Self::Seconds);
        }
        if let Some(body) = s.strip_suffix('f') {
            let n: i64 = body.trim().parse().map_err(|_| {
                format!(
                    "invalid frame count {body:?}; use a whole number followed by f, like \"45f\""
                )
            })?;
            return Ok(Self::Frames(n));
        }
        parse_decimal(s).map(Self::Seconds)
    }
}

/// For a number followed by a unit other than `s`, `ms` or `f` ("4
/// seconds", "2 min"), the error that names the unit and the form to
/// write instead; `None` for anything else, which parses as before.
fn spelled_unit(s: &str) -> Option<String> {
    let at = s.find(|c: char| c.is_alphabetic())?;
    let (number, unit) = (s[..at].trim(), s[at..].trim());
    let value = parse_decimal(number).ok()?.to_f64();
    let in_seconds = |factor: f64| format!("\"{}s\"", value * factor);
    let instead = match unit.to_ascii_lowercase().as_str() {
        "s" | "ms" | "f" => return None,
        "sec" | "secs" | "second" | "seconds" => format!("\"{number}s\""),
        "msec" | "msecs" | "millisecond" | "milliseconds" => format!("\"{number}ms\""),
        "frame" | "frames" => format!("\"{number}f\""),
        "m" | "min" | "mins" | "minute" | "minutes" => in_seconds(60.0),
        "h" | "hr" | "hrs" | "hour" | "hours" => in_seconds(3600.0),
        _ => {
            return Some(format!(
                "unknown unit {unit:?} in {s:?}; a time is seconds (1.5 or \"1.5s\"), \
                 milliseconds (\"1500ms\"), frames (\"45f\") or a timecode (\"1:02.5\")"
            ));
        }
    };
    Some(format!("unknown unit {unit:?} in {s:?}; write {instead}"))
}

/// Parses a decimal string such as `-1.25` into an exact ratio.
pub(crate) fn parse_decimal(s: &str) -> Result<Ratio, String> {
    let s = s.trim();
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if (int.is_empty() && frac.is_empty())
        || !int.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
    {
        return Err(format!("invalid number {s:?}"));
    }
    if frac.len() > 9 {
        return Err(format!("{s:?} has more than nine decimal places"));
    }
    let int_part: i64 = if int.is_empty() {
        0
    } else {
        int.parse().map_err(|_| format!("{s:?} is too large"))?
    };
    let den = 10_i64.pow(frac.len() as u32);
    let frac_part: i64 = if frac.is_empty() {
        0
    } else {
        frac.parse().map_err(|_| format!("invalid number {s:?}"))?
    };
    let num = int_part
        .checked_mul(den)
        .and_then(|v| v.checked_add(frac_part))
        .ok_or_else(|| format!("{s:?} is too large"))?;
    let r = Ratio::new(num, den);
    Ok(if neg { Ratio::ZERO - r } else { r })
}

fn parse_timecode(s: &str) -> Result<Ratio, String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return Err(format!(
            "timecode {s:?} has too many parts; use hh:mm:ss.fff"
        ));
    }
    let mut total = Ratio::ZERO;
    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let value = if is_last {
            parse_decimal(part)
                .map_err(|_| format!("invalid seconds {part:?} in timecode {s:?}"))?
        } else {
            let n: i64 = part.trim().parse().map_err(|_| {
                format!("invalid component {part:?} in timecode {s:?}; only the seconds may have decimals")
            })?;
            Ratio::from_int(n)
        };
        total = total * Ratio::from_int(60) + value;
    }
    Ok(total)
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Seconds(s) => write!(f, "{s}s"),
            Self::Frames(n) => write!(f, "{n}f"),
        }
    }
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TimeVisitor;

        impl Visitor<'_> for TimeVisitor {
            type Value = Time;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a time: seconds as a number, or a string like \"1.5s\", \"500ms\", \"45f\" or \"00:01:02.5\"")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Time, E> {
                if !v.is_finite() {
                    return Err(E::custom("time must be a finite number"));
                }
                // `{}` prints the shortest decimal that round-trips, which
                // recovers the digits the author typed.
                parse_decimal(&format!("{v}"))
                    .map(Time::Seconds)
                    .map_err(E::custom)
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Time, E> {
                Ok(Time::Seconds(Ratio::from_int(v)))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Time, E> {
                i64::try_from(v)
                    .map(|v| Time::Seconds(Ratio::from_int(v)))
                    .map_err(|_| E::custom("time is too large"))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Time, E> {
                Time::parse(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(TimeVisitor)
    }
}

impl JsonSchema for Time {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Time".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "title": "Time",
            "description": "A time or duration: seconds as a number, or a string with a unit (\"1.5s\", \"500ms\", \"45f\" for frames at the output rate) or a timecode (\"00:01:02.5\").",
            "anyOf": [
                { "type": "number" },
                { "type": "string", "pattern": "^\\s*-?(\\d+(\\.\\d+)?(s|ms)?|\\d+f|(\\d+:){1,2}\\d+(\\.\\d+)?)\\s*$" }
            ]
        })
    }
}

/// A frame rate in frames per second.
///
/// Accepted JSON forms: a number (`30`, `29.97`) or a string ratio
/// (`"30000/1001"`). The common NTSC approximations 23.976, 29.97 and 59.94
/// are snapped to their exact 1001-based ratios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fps(pub Ratio);

impl Fps {
    /// The exact rate.
    pub fn ratio(self) -> Ratio {
        self.0
    }

    /// Parses `"30000/1001"`, `"30"` or `"29.97"`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some((n, d)) = s.split_once('/') {
            let n: i64 = n
                .trim()
                .parse()
                .map_err(|_| format!("invalid frame rate numerator {n:?}"))?;
            let d: i64 = d
                .trim()
                .parse()
                .map_err(|_| format!("invalid frame rate denominator {d:?}"))?;
            if d == 0 {
                return Err("frame rate denominator must not be zero".to_owned());
            }
            return Ok(Self(Ratio::new(n, d)));
        }
        let r = parse_decimal(s).map_err(|_| {
            format!("invalid frame rate {s:?}; use a number like 30 or a ratio like \"30000/1001\"")
        })?;
        Ok(Self(snap_ntsc(r)))
    }
}

fn snap_ntsc(r: Ratio) -> Ratio {
    const SNAPS: [(&str, i64); 3] = [("23.976", 24000), ("29.97", 30000), ("59.94", 60000)];
    for (text, num) in SNAPS {
        let approx = parse_decimal(text).expect("constant");
        let exact = Ratio::new(num, 1001);
        if r == approx || r == exact {
            return exact;
        }
        // Also accept longer decimal spellings such as 23.976023976.
        let diff = (r.to_f64() - exact.to_f64()).abs();
        if diff < 1e-4 {
            return exact;
        }
    }
    r
}

impl fmt::Display for Fps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_integer() {
            write!(f, "{}", self.0.numer())
        } else {
            write!(f, "{}/{}", self.0.numer(), self.0.denom())
        }
    }
}

impl Serialize for Fps {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0.is_integer() {
            serializer.serialize_i64(self.0.numer())
        } else {
            serializer.serialize_str(&self.to_string())
        }
    }
}

impl<'de> Deserialize<'de> for Fps {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FpsVisitor;

        impl Visitor<'_> for FpsVisitor {
            type Value = Fps;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a frame rate: a number like 30 or a ratio string like \"30000/1001\"")
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Fps, E> {
                if !v.is_finite() || v <= 0.0 {
                    return Err(E::custom("frame rate must be a positive number"));
                }
                Fps::parse(&format!("{v}")).map_err(E::custom)
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Fps, E> {
                if v <= 0 {
                    return Err(E::custom("frame rate must be a positive number"));
                }
                Ok(Fps(Ratio::from_int(v)))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Fps, E> {
                i64::try_from(v)
                    .map_err(|_| E::custom("frame rate is too large"))
                    .and_then(|v| self.visit_i64(v))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Fps, E> {
                let fps = Fps::parse(v).map_err(E::custom)?;
                if fps.0 <= Ratio::ZERO {
                    return Err(E::custom("frame rate must be positive"));
                }
                Ok(fps)
            }
        }

        deserializer.deserialize_any(FpsVisitor)
    }
}

impl JsonSchema for Fps {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Fps".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "title": "Fps",
            "description": "Frames per second: a positive number (30, 29.97) or an exact ratio string (\"30000/1001\").",
            "anyOf": [
                { "type": "number", "exclusiveMinimum": 0 },
                { "type": "string", "pattern": "^\\s*\\d+(\\.\\d+)?(\\s*/\\s*\\d+)?\\s*$" }
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(
            Time::parse("1.5s").unwrap(),
            Time::Seconds(Ratio::new(3, 2))
        );
        assert_eq!(
            Time::parse("1500ms").unwrap(),
            Time::Seconds(Ratio::new(3, 2))
        );
        assert_eq!(Time::parse("45f").unwrap(), Time::Frames(45));
        assert_eq!(Time::parse("2").unwrap(), Time::seconds(2));
        assert_eq!(
            Time::parse("-0.5s").unwrap(),
            Time::Seconds(Ratio::new(-1, 2))
        );
    }

    #[test]
    fn a_spelled_out_unit_is_pointed_at_the_form_to_write() {
        let err = |s: &str| Time::parse(s).unwrap_err();
        assert_eq!(
            err("4 seconds"),
            r#"unknown unit "seconds" in "4 seconds"; write "4s""#
        );
        assert!(err("1.5 secs").ends_with(r#"write "1.5s""#));
        assert!(err("4seconds").ends_with(r#"write "4s""#));
        assert!(err("2 minutes").ends_with(r#"write "120s""#));
        assert!(err("1.5 min").ends_with(r#"write "90s""#));
        assert!(err("30 frames").ends_with(r#"write "30f""#));
        assert!(err("250 milliseconds").ends_with(r#"write "250ms""#));
        assert!(err("3 fortnights").contains("a time is seconds"));
        // The accepted forms, spaced or not, still parse.
        assert_eq!(
            Time::parse("1.5 s").unwrap(),
            Time::Seconds(Ratio::new(3, 2))
        );
        assert_eq!(Time::parse("45 f").unwrap(), Time::Frames(45));
        assert_eq!(
            Time::parse("1500 ms").unwrap(),
            Time::Seconds(Ratio::new(3, 2))
        );
    }

    #[test]
    fn parses_timecodes() {
        assert_eq!(
            Time::parse("00:01:02.5").unwrap(),
            Time::Seconds(Ratio::new(125, 2))
        );
        assert_eq!(Time::parse("1:02").unwrap(), Time::seconds(62));
        assert_eq!(Time::parse("2:00:00").unwrap(), Time::seconds(7200));
        assert!(Time::parse("1:2:3:4").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Time::parse("").is_err());
        assert!(Time::parse("1.5x").is_err());
        assert!(Time::parse("abc").is_err());
        assert!(Time::parse("1.5.5s").is_err());
    }

    #[test]
    fn json_numbers_are_exact() {
        let t: Time = serde_json::from_str("0.1").unwrap();
        assert_eq!(t, Time::Seconds(Ratio::new(1, 10)));
        let t: Time = serde_json::from_str("3").unwrap();
        assert_eq!(t, Time::seconds(3));
        let t: Time = serde_json::from_str("\"2.5s\"").unwrap();
        assert_eq!(t, Time::Seconds(Ratio::new(5, 2)));
    }

    #[test]
    fn frames_resolve_against_fps() {
        let fps = Ratio::new(30000, 1001);
        assert_eq!(Time::Frames(30000).resolve(fps), Ratio::from_int(1001));
    }

    #[test]
    fn fps_snaps_ntsc_rates() {
        assert_eq!(Fps::parse("29.97").unwrap().0, Ratio::new(30000, 1001));
        assert_eq!(Fps::parse("23.976").unwrap().0, Ratio::new(24000, 1001));
        assert_eq!(
            Fps::parse("23.976023976").unwrap().0,
            Ratio::new(24000, 1001)
        );
        assert_eq!(Fps::parse("30000/1001").unwrap().0, Ratio::new(30000, 1001));
        assert_eq!(Fps::parse("25").unwrap().0, Ratio::from_int(25));
        assert_eq!(Fps::parse("24.5").unwrap().0, Ratio::new(49, 2));
    }

    #[test]
    fn fps_json_forms() {
        let f: Fps = serde_json::from_str("29.97").unwrap();
        assert_eq!(f.0, Ratio::new(30000, 1001));
        assert!(serde_json::from_str::<Fps>("0").is_err());
        assert!(serde_json::from_str::<Fps>("\"30/0\"").is_err());
        assert_eq!(serde_json::to_string(&f).unwrap(), "\"30000/1001\"");
        assert_eq!(
            serde_json::to_string(&Fps(Ratio::from_int(30))).unwrap(),
            "30"
        );
    }
}
