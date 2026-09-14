//! CSS `animation` and `@keyframes`, mapped onto the anim engine.
//!
//! A document names its motion the way a stylesheet does: a top-level
//! `keyframes` map holds the rules, and a clip's `animation` field names
//! one or more of them with a duration, a delay and a timing function.
//! Nothing here interprets selectors or layout; the properties a rule may
//! set are the four the renderer already animates, and resolution turns
//! them into ordinary keyframe tracks, so everything downstream — the
//! compositor, the copy planner, `--show-timeline` — is unchanged.

use std::fmt;

use geneva_anim::{Easing, NamedEasing, Spring};

/// Which way round each run of an animation plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Every run plays forwards.
    Normal,
    /// Every run plays backwards.
    Reverse,
    /// Odd-numbered runs play backwards.
    Alternate,
    /// Even-numbered runs play backwards.
    AlternateReverse,
}

impl Direction {
    /// Whether run `i` (counting from zero) plays backwards.
    pub fn reversed(self, i: u32) -> bool {
        match self {
            Self::Normal => false,
            Self::Reverse => true,
            Self::Alternate => i % 2 == 1,
            Self::AlternateReverse => i % 2 == 0,
        }
    }
}

/// One entry of an `animation` list.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    /// Name of the rule under the document's `keyframes`.
    pub name: String,
    /// Length of one run in seconds; always positive.
    pub duration: f64,
    /// Seconds before the first run.
    pub delay: f64,
    /// Curve applied between each pair of offsets.
    pub easing: Easing,
    /// Number of runs; infinite fills the clip.
    pub iterations: f64,
    /// Which way round each run plays.
    pub direction: Direction,
}

/// The properties one offset of a rule sets. Each is `None` when the
/// declaration block says nothing about it, which is how CSS decides
/// between which offsets a property interpolates.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Values {
    /// Pixels added to the clip's position.
    pub translate: Option<[f64; 2]>,
    /// Factors multiplied into the clip's scale.
    pub scale: Option<[f64; 2]>,
    /// Degrees added to the clip's rotation.
    pub rotate: Option<f64>,
    /// Opacity, which replaces the clip's own.
    pub opacity: Option<f64>,
}

impl Values {
    /// Whether the block sets nothing at all.
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// Splits on a separator outside parentheses.
fn split_outside_parens(text: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 => {
                out.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

/// Whitespace-separated tokens, keeping `f(a, b)` together.
fn tokens(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if let Some(s) = start.take() {
                out.push(&text[s..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

/// A CSS `<time>`: `0.5s`, `500ms`. The unit is required, which is what
/// keeps a bare number free to mean an iteration count.
fn time(token: &str) -> Option<f64> {
    let (body, factor) = if let Some(b) = token.strip_suffix("ms") {
        (b, 0.001)
    } else if let Some(b) = token.strip_suffix('s') {
        (b, 1.0)
    } else {
        return None;
    };
    let v: f64 = body.parse().ok()?;
    v.is_finite().then_some(v * factor)
}

/// A CSS `<angle>` in degrees: `90deg`, `0.25turn`, `1.57rad`, or a bare
/// number of degrees.
fn angle(token: &str) -> Result<f64, String> {
    let (body, factor) = if let Some(b) = token.strip_suffix("deg") {
        (b, 1.0)
    } else if let Some(b) = token.strip_suffix("turn") {
        (b, 360.0)
    } else if let Some(b) = token.strip_suffix("rad") {
        (b, 180.0 / std::f64::consts::PI)
    } else if let Some(b) = token.strip_suffix("grad") {
        (b, 0.9)
    } else {
        (token, 1.0)
    };
    let v: f64 = body
        .trim()
        .parse()
        .map_err(|_| format!("invalid angle {token:?}; write it like \"90deg\" or \"0.25turn\""))?;
    finite(v, token).map(|v| v * factor)
}

/// A pixel length: `-656px`, `12`, `0`. Percentages are not accepted,
/// since a clip's own box is not known until the source is opened.
fn pixels(token: &str) -> Result<f64, String> {
    if token.ends_with('%') {
        return Err(format!(
            "percentages are not supported in {token:?}; write the distance in pixels"
        ));
    }
    let body = token.strip_suffix("px").unwrap_or(token);
    let v: f64 = body
        .trim()
        .parse()
        .map_err(|_| format!("invalid length {token:?}; write it like \"-656px\""))?;
    finite(v, token)
}

/// A plain number, or a percentage for the properties that take one.
fn number(token: &str) -> Result<f64, String> {
    let (body, factor) = match token.strip_suffix('%') {
        Some(b) => (b, 0.01),
        None => (token, 1.0),
    };
    let v: f64 = body
        .trim()
        .parse()
        .map_err(|_| format!("invalid number {token:?}"))?;
    finite(v, token).map(|v| v * factor)
}

fn finite(v: f64, token: &str) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("{token:?} is not a finite number"))
    }
}

/// The arguments of `name(a, b)`, if the token is that call.
fn call<'a>(token: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let rest = token.strip_prefix(name)?.strip_prefix('(')?;
    let inner = rest.strip_suffix(')')?;
    Some(
        split_outside_parens(inner, ',')
            .into_iter()
            .map(str::trim)
            .collect(),
    )
}

/// A CSS timing function, plus geneva's own `spring()`.
fn easing(token: &str) -> Option<Result<Easing, String>> {
    let named = match token {
        "linear" => Some(NamedEasing::Linear),
        "ease" => Some(NamedEasing::Ease),
        "ease-in" => Some(NamedEasing::EaseIn),
        "ease-out" => Some(NamedEasing::EaseOut),
        "ease-in-out" => Some(NamedEasing::EaseInOut),
        "step-end" | "hold" => Some(NamedEasing::Hold),
        _ => None,
    };
    if let Some(n) = named {
        return Some(Ok(Easing::Named(n)));
    }
    if let Some(args) = call(token, "cubic-bezier") {
        return Some(cubic_bezier(&args));
    }
    if let Some(args) = call(token, "spring") {
        return Some(spring(&args));
    }
    None
}

fn cubic_bezier(args: &[&str]) -> Result<Easing, String> {
    if args.len() != 4 {
        return Err("cubic-bezier takes four numbers".to_owned());
    }
    let mut p = [0.0; 4];
    for (slot, arg) in p.iter_mut().zip(args) {
        *slot = number(arg)?;
    }
    Ok(Easing::CubicBezier { cubic_bezier: p })
}

/// `spring(stiffness, damping, mass)`, with the last two optional.
fn spring(args: &[&str]) -> Result<Easing, String> {
    if args.is_empty() || args.len() > 3 {
        return Err("spring takes a stiffness and optionally a damping and a mass".to_owned());
    }
    let mut s = Spring {
        stiffness: number(args[0])?,
        ..Spring::default()
    };
    if let Some(a) = args.get(1) {
        s.damping = number(a)?;
    }
    if let Some(a) = args.get(2) {
        s.mass = number(a)?;
    }
    Ok(Easing::Spring { spring: s })
}

const ANIMATION_FORM: &str = "an animation is a rule name with a duration, and optionally a delay, \
a timing function, an iteration count and a direction: \
\"slide-in 0.5s ease-out\", \"pulse 2s 1s infinite alternate\"";

/// Parses an `animation` value: one or more comma-separated entries.
pub fn parse_animations(value: &str) -> Result<Vec<Animation>, String> {
    let mut out = Vec::new();
    for entry in split_outside_parens(value, ',') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(format!("empty animation in {value:?}; {ANIMATION_FORM}"));
        }
        out.push(parse_animation(entry)?);
    }
    Ok(out)
}

fn parse_animation(entry: &str) -> Result<Animation, String> {
    let mut name: Option<String> = None;
    let mut times: Vec<f64> = Vec::new();
    let mut ease: Option<Easing> = None;
    let mut iterations: Option<f64> = None;
    let mut direction: Option<Direction> = None;

    for token in tokens(entry) {
        if let Some(t) = time(token) {
            if t < 0.0 && times.is_empty() {
                return Err(format!("a duration cannot be negative in {entry:?}"));
            }
            if times.len() == 2 {
                return Err(format!(
                    "{entry:?} has three times; an animation takes a duration and a delay"
                ));
            }
            times.push(t);
            continue;
        }
        if let Some(e) = easing(token) {
            if ease.is_some() {
                return Err(format!("{entry:?} names two timing functions"));
            }
            ease = Some(e?);
            continue;
        }
        match token {
            "infinite" => {
                iterations = Some(f64::INFINITY);
                continue;
            }
            "normal" | "reverse" | "alternate" | "alternate-reverse" => {
                direction = Some(match token {
                    "reverse" => Direction::Reverse,
                    "alternate" => Direction::Alternate,
                    "alternate-reverse" => Direction::AlternateReverse,
                    _ => Direction::Normal,
                });
                continue;
            }
            _ => {}
        }
        if let Ok(n) = token.parse::<f64>() {
            if !n.is_finite() || n < 0.0 {
                return Err(format!("an iteration count cannot be {token:?}"));
            }
            iterations = Some(n);
            continue;
        }
        if name.is_some() {
            return Err(format!("{entry:?} names two rules; {ANIMATION_FORM}"));
        }
        name = Some(token.to_owned());
    }

    let name = name.ok_or_else(|| format!("{entry:?} names no rule; {ANIMATION_FORM}"))?;
    let duration = *times
        .first()
        .ok_or_else(|| format!("{entry:?} has no duration; {ANIMATION_FORM}"))?;
    if duration <= 0.0 {
        return Err(format!("the duration in {entry:?} must be more than zero"));
    }
    Ok(Animation {
        name,
        duration,
        delay: times.get(1).copied().unwrap_or(0.0),
        easing: ease.unwrap_or_default(),
        iterations: iterations.unwrap_or(1.0),
        direction: direction.unwrap_or(Direction::Normal),
    })
}

/// Parses a keyframe offset: `from`, `to` or a percentage.
pub fn parse_offset(key: &str) -> Result<f64, String> {
    let key = key.trim();
    match key {
        "from" => return Ok(0.0),
        "to" => return Ok(1.0),
        _ => {}
    }
    let body = key.strip_suffix('%').ok_or_else(|| {
        format!(
            "invalid keyframe offset {key:?}; use \"from\", \"to\" or a percentage like \"60%\""
        )
    })?;
    let v: f64 = body
        .trim()
        .parse()
        .map_err(|_| format!("invalid keyframe offset {key:?}"))?;
    if !(0.0..=100.0).contains(&v) {
        return Err(format!("keyframe offset {key:?} is outside 0% to 100%"));
    }
    Ok(v / 100.0)
}

/// Parses a declaration block: `transform: translateX(-656px); opacity: 0`.
pub fn parse_declarations(block: &str) -> Result<Values, String> {
    let mut v = Values::default();
    for decl in split_outside_parens(block, ';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let (property, value) = decl
            .split_once(':')
            .ok_or_else(|| format!("{decl:?} is not a declaration; write \"property: value\""))?;
        let (property, value) = (property.trim(), value.trim());
        match property {
            "transform" => apply_transform(value, &mut v)?,
            "translate" => v.translate = Some(add(v.translate, translate_pair(value)?)),
            "scale" => v.scale = Some(mul(v.scale, scale_pair(value)?)),
            "rotate" => v.rotate = Some(v.rotate.unwrap_or(0.0) + angle(value)?),
            "opacity" => v.opacity = Some(number(value)?),
            _ => {
                return Err(format!(
                    "{property:?} cannot be animated; a keyframe sets transform, translate, \
scale, rotate or opacity"
                ));
            }
        }
    }
    Ok(v)
}

fn add(base: Option<[f64; 2]>, d: [f64; 2]) -> [f64; 2] {
    let b = base.unwrap_or([0.0, 0.0]);
    [b[0] + d[0], b[1] + d[1]]
}

fn mul(base: Option<[f64; 2]>, d: [f64; 2]) -> [f64; 2] {
    let b = base.unwrap_or([1.0, 1.0]);
    [b[0] * d[0], b[1] * d[1]]
}

/// One or two lengths, as the `translate` property takes.
fn translate_pair(value: &str) -> Result<[f64; 2], String> {
    let t = tokens(value);
    match t.as_slice() {
        [x] => Ok([pixels(x)?, 0.0]),
        [x, y] => Ok([pixels(x)?, pixels(y)?]),
        _ => Err(format!(
            "invalid translate {value:?}; write one or two lengths"
        )),
    }
}

/// One or two factors, as the `scale` property takes.
fn scale_pair(value: &str) -> Result<[f64; 2], String> {
    let t = tokens(value);
    match t.as_slice() {
        [s] => {
            let s = number(s)?;
            Ok([s, s])
        }
        [x, y] => Ok([number(x)?, number(y)?]),
        _ => Err(format!("invalid scale {value:?}; write one or two numbers")),
    }
}

/// A `transform` function list. The functions are collected rather than
/// multiplied in order: translations add, scales multiply and rotations
/// add, and the renderer applies scale and rotation about the anchor and
/// then the translation.
fn apply_transform(value: &str, v: &mut Values) -> Result<(), String> {
    if value.trim() == "none" {
        return Ok(());
    }
    for token in tokens(value) {
        let (name, _) = token
            .split_once('(')
            .ok_or_else(|| format!("{token:?} is not a transform function"))?;
        let args: Vec<&str> = call(token, name).unwrap_or_default();
        if args.is_empty() || args.iter().any(|a| a.is_empty()) {
            return Err(format!("{token:?} has no arguments"));
        }
        let two = |i: usize| args.get(i).copied();
        match name {
            "translate" => {
                let x = pixels(args[0])?;
                let y = two(1).map(pixels).transpose()?.unwrap_or(0.0);
                v.translate = Some(add(v.translate, [x, y]));
            }
            "translateX" => v.translate = Some(add(v.translate, [pixels(args[0])?, 0.0])),
            "translateY" => v.translate = Some(add(v.translate, [0.0, pixels(args[0])?])),
            "scale" => {
                let x = number(args[0])?;
                let y = two(1).map(number).transpose()?.unwrap_or(x);
                v.scale = Some(mul(v.scale, [x, y]));
            }
            "scaleX" => v.scale = Some(mul(v.scale, [number(args[0])?, 1.0])),
            "scaleY" => v.scale = Some(mul(v.scale, [1.0, number(args[0])?])),
            "rotate" => v.rotate = Some(v.rotate.unwrap_or(0.0) + angle(args[0])?),
            _ => {
                return Err(format!(
                    "{name:?} is not a transform geneva animates; use translate, translateX, \
translateY, scale, scaleX, scaleY or rotate"
                ));
            }
        }
    }
    Ok(())
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "normal",
            Self::Reverse => "reverse",
            Self::Alternate => "alternate",
            Self::AlternateReverse => "alternate-reverse",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(s: &str) -> Animation {
        parse_animations(s).unwrap().pop().unwrap()
    }

    #[test]
    fn parses_the_animation_shorthand() {
        let a = one("slide-in 0.5s ease-out");
        assert_eq!(a.name, "slide-in");
        assert!((a.duration - 0.5).abs() < 1e-9);
        assert_eq!(a.delay, 0.0);
        assert_eq!(a.easing, Easing::Named(NamedEasing::EaseOut));
        assert_eq!(a.iterations, 1.0);
        assert_eq!(a.direction, Direction::Normal);
    }

    #[test]
    fn takes_the_parts_in_any_order() {
        let a = one("alternate 2s infinite pulse 500ms");
        assert_eq!(a.name, "pulse");
        assert_eq!(a.duration, 2.0);
        assert!((a.delay - 0.5).abs() < 1e-9);
        assert_eq!(a.iterations, f64::INFINITY);
        assert_eq!(a.direction, Direction::Alternate);
    }

    #[test]
    fn takes_a_list() {
        let list = parse_animations("fade-in 0.3s, fade-out 0.3s 3.7s").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].name, "fade-out");
        assert!((list[1].delay - 3.7).abs() < 1e-9);
    }

    #[test]
    fn rejects_nonsense() {
        for bad in [
            "slide-in",
            "0.5s",
            "slide-in 0s",
            "slide-in fade-out 1s",
            "slide-in 1s 2s 3s",
            "slide-in 1s ease-out linear",
            "",
        ] {
            assert!(parse_animations(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn parses_offsets() {
        assert_eq!(parse_offset("from").unwrap(), 0.0);
        assert_eq!(parse_offset("to").unwrap(), 1.0);
        assert!((parse_offset("60%").unwrap() - 0.6).abs() < 1e-9);
        assert!(parse_offset("60").is_err());
        assert!(parse_offset("140%").is_err());
    }

    #[test]
    fn parses_declarations_into_values() {
        let v = parse_declarations("transform: translateX(-656px) rotate(0.25turn); opacity: 0")
            .unwrap();
        assert_eq!(v.translate, Some([-656.0, 0.0]));
        assert_eq!(v.rotate, Some(90.0));
        assert_eq!(v.opacity, Some(0.0));
        assert_eq!(v.scale, None);

        let v = parse_declarations("translate: 10px 20px; scale: 2; rotate: 45deg").unwrap();
        assert_eq!(v.translate, Some([10.0, 20.0]));
        assert_eq!(v.scale, Some([2.0, 2.0]));
        assert_eq!(v.rotate, Some(45.0));

        // Functions of the same kind compose.
        let v =
            parse_declarations("transform: translateX(10px) translateY(20px) scale(2) scaleX(3)")
                .unwrap();
        assert_eq!(v.translate, Some([10.0, 20.0]));
        assert_eq!(v.scale, Some([6.0, 2.0]));
    }

    #[test]
    fn rejects_what_it_cannot_animate() {
        for bad in [
            "color: red",
            "transform: skew(10deg)",
            "transform: translateX(50%)",
            "opacity",
            "transform: translateX()",
        ] {
            assert!(parse_declarations(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn directions_reverse_the_right_runs() {
        assert!(!Direction::Normal.reversed(3));
        assert!(Direction::Reverse.reversed(0));
        assert!(!Direction::Alternate.reversed(0));
        assert!(Direction::Alternate.reversed(1));
        assert!(Direction::AlternateReverse.reversed(0));
    }
}
