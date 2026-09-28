//! CSS `animation` and `@keyframes`, mapped onto the anim engine.
//!
//! A document names its motion the way a stylesheet does: a top-level
//! `keyframes` map holds the rules, and a clip's `animation` field names
//! one or more of them with a duration, a delay and a timing function.
//! Nothing here interprets selectors or layout; the properties a rule may
//! set are the four the renderer already animates, and resolution turns
//! them into ordinary keyframe tracks, so everything downstream (the
//! compositor, the copy planner, `--show-timeline`) is unchanged.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use geneva_anim::{Easing, NamedEasing, Spring, StepPosition};
use geneva_color::Color;

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
            Self::AlternateReverse => i.is_multiple_of(2),
        }
    }
}

/// What an animation leaves on the element outside its runs:
/// `animation-fill-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fill {
    /// Nothing: before the delay and after the last run the element is
    /// as its style says.
    #[default]
    None,
    /// The last run's final values stay after it ends.
    Forwards,
    /// The first run's initial values apply during the delay.
    Backwards,
    /// Both of the above.
    Both,
}

impl Fill {
    /// Whether the animation holds its end after the last run.
    #[must_use]
    pub fn forwards(self) -> bool {
        matches!(self, Self::Forwards | Self::Both)
    }

    /// Whether the animation shows its start during the delay.
    #[must_use]
    pub fn backwards(self) -> bool {
        matches!(self, Self::Backwards | Self::Both)
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
    /// What is left outside the runs.
    pub fill: Fill,
}

/// A translation component: pixels, or a percentage of the clip's own
/// box, as CSS resolves a percentage in `translate`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shift {
    /// A distance in pixels.
    Px(f64),
    /// A share of the clip's own width or height; `100.0` is all of it.
    Percent(f64),
}

impl Shift {
    /// Resolves against the clip's box, when its size is known.
    pub fn to_px(self, box_size: Option<f64>) -> Option<f64> {
        match self {
            Self::Px(v) => Some(v),
            Self::Percent(p) => box_size.map(|s| s * p / 100.0),
        }
    }

    /// Whether the value needs the clip's box to resolve.
    pub fn is_relative(self) -> bool {
        matches!(self, Self::Percent(_))
    }
}

/// The properties one offset of a rule sets. Each is `None` when the
/// declaration block says nothing about it, which is how CSS decides
/// between which offsets a property interpolates.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Values {
    /// Added to the clip's position.
    pub translate: Option<[Shift; 2]>,
    /// Factors multiplied into the clip's scale.
    pub scale: Option<[f64; 2]>,
    /// Degrees added to the clip's rotation.
    pub rotate: Option<f64>,
    /// Opacity, which replaces the clip's own.
    pub opacity: Option<f64>,
    /// `filter: blur()`, in pixels. The properties from here on are
    /// played on an element inside markup, not on a clip.
    pub blur: Option<f64>,
    /// `color`.
    pub color: Option<Color>,
    /// `text-shadow`: the list front to back, empty for `none`.
    pub text_shadow: Option<Vec<TextShadow>>,
    /// `letter-spacing`.
    pub letter_spacing: Option<Spacing>,
    /// `width`.
    pub width: Option<Shift>,
    /// `height`.
    pub height: Option<Shift>,
    /// `max-width`.
    pub max_width: Option<Shift>,
    /// `min-width`.
    pub min_width: Option<Shift>,
    /// `background-position`.
    pub background_position: Option<[Shift; 2]>,
    /// `clip-path`: a polygon's points, or empty for `none`.
    pub clip_path: Option<Vec<[Shift; 2]>>,
}

/// A `text-shadow` in a keyframe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextShadow {
    /// Horizontal offset in pixels.
    pub x: f64,
    /// Vertical offset in pixels.
    pub y: f64,
    /// Blur radius in pixels.
    pub blur: f64,
    /// Colour.
    pub color: Color,
}

impl Values {
    /// Whether the block sets nothing at all.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether it sets something a clip cannot play: anything past
    /// transform and opacity belongs to an element inside markup.
    pub fn beyond_a_clip(&self) -> bool {
        self.blur.is_some()
            || self.color.is_some()
            || self.text_shadow.is_some()
            || self.moves_layout()
            || self.background_position.is_some()
            || self.clip_path.is_some()
    }

    /// Whether it changes where boxes land, so the markup is laid out
    /// again at each frame it plays.
    pub fn moves_layout(&self) -> bool {
        self.letter_spacing.is_some()
            || self.width.is_some()
            || self.height.is_some()
            || self.max_width.is_some()
            || self.min_width.is_some()
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
    } else {
        let b = token.strip_suffix('s')?;
        (b, 1.0)
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

/// A distance: `-656px`, `12`, `0`, or `-100%` of the clip's own box.
/// A `letter-spacing` in a keyframe. An `em` is the element's own font
/// size, which only the element knows, so it is kept as written and
/// resolved when the animation is played on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spacing {
    /// Pixels.
    Px(f64),
    /// Multiples of the element's font size.
    Em(f64),
}

impl Spacing {
    /// In pixels, for an element whose font size is `font_size`.
    #[must_use]
    pub fn to_px(self, font_size: f64) -> f64 {
        match self {
            Spacing::Px(p) => p,
            Spacing::Em(e) => e * font_size,
        }
    }
}

fn spacing(value: &str) -> Result<Spacing, String> {
    let value = value.trim();
    if value == "normal" {
        return Ok(Spacing::Px(0.0));
    }
    if let Some(body) = value.strip_suffix("em").filter(|b| !b.ends_with('r')) {
        let v: f64 = body
            .trim()
            .parse()
            .map_err(|_| format!("invalid length {value:?}; write it like \"2px\" or \".3em\""))?;
        return finite(v, value).map(Spacing::Em);
    }
    match pixels(value)? {
        Shift::Px(p) => Ok(Spacing::Px(p)),
        Shift::Percent(_) => Err(format!("letter-spacing takes a length, not {value:?}")),
    }
}

fn pixels(token: &str) -> Result<Shift, String> {
    if let Some(body) = token.strip_suffix('%') {
        let v: f64 = body
            .trim()
            .parse()
            .map_err(|_| format!("invalid percentage {token:?}"))?;
        return finite(v, token).map(Shift::Percent);
    }
    let body = token.strip_suffix("px").unwrap_or(token);
    let v: f64 = body
        .trim()
        .parse()
        .map_err(|_| format!("invalid length {token:?}; write it like \"-656px\" or \"-100%\""))?;
    finite(v, token).map(Shift::Px)
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
    if let Some(args) = call(token, "steps") {
        return Some(steps(&args));
    }
    if let Some(args) = call(token, "linear") {
        return Some(linear_points(&args));
    }
    None
}

/// `steps(n[, position])`.
fn steps(args: &[&str]) -> Result<Easing, String> {
    let (count, position) = match args {
        [n] => (n, StepPosition::default()),
        [n, p] => (
            n,
            match *p {
                "jump-start" | "start" => StepPosition::JumpStart,
                "jump-end" | "end" => StepPosition::JumpEnd,
                "jump-none" => StepPosition::JumpNone,
                "jump-both" => StepPosition::JumpBoth,
                _ => {
                    return Err(format!(
                        "{p:?} is not a step position; use jump-start, jump-end, jump-none or jump-both"
                    ));
                }
            },
        ),
        _ => return Err("steps takes a count and optionally a position".to_owned()),
    };
    let n: u32 = count
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("steps takes a whole number of steps, not {count:?}"))?;
    let e = Easing::Steps {
        steps: (n, position),
    };
    match e.validate() {
        Some(why) => Err(why),
        None => Ok(e),
    }
}

/// `linear(<output> [<input>%]{0,2}, ...)`: each entry an output with up
/// to two inputs. An entry with none is spread evenly between the
/// entries around it that have one, the first at 0% and the last at
/// 100%, as CSS spreads them.
fn linear_points(args: &[&str]) -> Result<Easing, String> {
    let mut points: Vec<(Option<f64>, f64)> = Vec::new();
    for entry in args {
        let t = tokens(entry);
        let (Some(y), rest) = (t.first(), t.get(1..).unwrap_or(&[])) else {
            return Err("linear has an empty entry".to_owned());
        };
        let y = number(y)?;
        if rest.len() > 2 {
            return Err(format!(
                "{entry:?}: a linear entry takes at most two inputs"
            ));
        }
        let xs: Vec<f64> = rest
            .iter()
            .map(|p| {
                p.strip_suffix('%')
                    .and_then(|b| b.parse::<f64>().ok())
                    .map(|v| v / 100.0)
                    .ok_or_else(|| format!("{p:?} is not a percentage"))
            })
            .collect::<Result<_, _>>()?;
        if xs.is_empty() {
            points.push((None, y));
        }
        for x in xs {
            points.push((Some(x), y));
        }
    }
    if points.len() < 2 {
        return Err("linear takes at least two points".to_owned());
    }
    let last = points.len() - 1;
    points[0].0.get_or_insert(0.0);
    points[last].0.get_or_insert(1.0);
    // Inputs never go backwards, then the gaps are spread evenly.
    let mut highest = 0.0f64;
    for p in &mut points {
        if let Some(x) = p.0.as_mut() {
            *x = x.max(highest);
            highest = *x;
        }
    }
    let mut i = 0;
    while i < points.len() {
        if points[i].0.is_some() {
            i += 1;
            continue;
        }
        let from = i - 1;
        let to = (i..points.len())
            .find(|j| points[*j].0.is_some())
            .expect("the last point has an input");
        let (x0, x1) = (points[from].0.unwrap_or(0.0), points[to].0.unwrap_or(1.0));
        let gaps = (to - from) as f64;
        for (k, p) in points[from + 1..to].iter_mut().enumerate() {
            p.0 = Some(x0 + (x1 - x0) * (k + 1) as f64 / gaps);
        }
        i = to;
    }
    let e = Easing::Linear {
        linear: points
            .into_iter()
            .map(|(x, y)| [x.unwrap_or(0.0), y])
            .collect(),
    };
    match e.validate() {
        Some(why) => Err(why),
        None => Ok(e),
    }
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
    let mut fill: Option<Fill> = None;

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
                direction = Some(direction_of(token));
                continue;
            }
            "none" | "forwards" | "backwards" | "both" => {
                if fill.is_some() {
                    return Err(format!("{entry:?} names two fill modes"));
                }
                fill = Some(fill_of(token));
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
        fill: fill.unwrap_or_default(),
    })
}

fn direction_of(token: &str) -> Direction {
    match token {
        "reverse" => Direction::Reverse,
        "alternate" => Direction::Alternate,
        "alternate-reverse" => Direction::AlternateReverse,
        _ => Direction::Normal,
    }
}

fn fill_of(token: &str) -> Fill {
    match token {
        "forwards" => Fill::Forwards,
        "backwards" => Fill::Backwards,
        "both" => Fill::Both,
        _ => Fill::None,
    }
}

/// The longhands an element may set beside the shorthand.
const LONGHANDS: [&str; 7] = [
    "animation-name",
    "animation-duration",
    "animation-delay",
    "animation-timing-function",
    "animation-iteration-count",
    "animation-direction",
    "animation-fill-mode",
];

/// Parses an element's `animation` with its longhands laid over it, as
/// CSS does: each longhand is a comma-separated list applied entry by
/// entry, repeating when it is shorter than the animation list. Without a
/// shorthand, `animation-name` starts the list and `animation-duration`
/// has to give each entry a length.
///
/// # Errors
///
/// Names the entry or longhand that does not parse.
pub fn parse_animation_spec(
    shorthand: Option<&str>,
    longhands: &[(String, String)],
) -> Result<Vec<Animation>, String> {
    let mut list = match shorthand {
        Some(s) => parse_animations(s)?,
        None => Vec::new(),
    };
    // The last setting of each longhand is the one that counts.
    let mut set: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (property, value) in longhands {
        if !LONGHANDS.contains(&property.as_str()) {
            return Err(format!("{property:?} is not an animation longhand"));
        }
        let parts: Vec<&str> = split_outside_parens(value, ',')
            .into_iter()
            .map(str::trim)
            .collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err(format!("{property}: {value:?} has an empty entry"));
        }
        set.insert(property.as_str(), parts);
    }
    if let Some(names) = set.get("animation-name") {
        if list.is_empty() {
            list = names
                .iter()
                .map(|n| Animation {
                    name: (*n).to_owned(),
                    duration: 0.0,
                    delay: 0.0,
                    easing: Easing::default(),
                    iterations: 1.0,
                    direction: Direction::Normal,
                    fill: Fill::None,
                })
                .collect();
        } else {
            for (i, a) in list.iter_mut().enumerate() {
                names[i % names.len()].clone_into(&mut a.name);
            }
        }
    }
    if list.is_empty() && !set.is_empty() {
        return Err(
            "animation longhands are set with no animation to apply them to; add \
`animation` or `animation-name`"
                .to_owned(),
        );
    }
    for (property, parts) in &set {
        if *property == "animation-name" {
            continue;
        }
        for (i, a) in list.iter_mut().enumerate() {
            let token = parts[i % parts.len()];
            let bad = || format!("{property}: {token:?} is not a value it takes");
            match *property {
                "animation-duration" => {
                    let t = time(token).ok_or_else(bad)?;
                    if t <= 0.0 {
                        return Err(format!("{property}: {token:?} must be more than zero"));
                    }
                    a.duration = t;
                }
                "animation-delay" => a.delay = time(token).ok_or_else(bad)?,
                "animation-timing-function" => a.easing = easing(token).ok_or_else(bad)??,
                "animation-iteration-count" => {
                    a.iterations = if token == "infinite" {
                        f64::INFINITY
                    } else {
                        let n = token.parse::<f64>().map_err(|_| bad())?;
                        if !n.is_finite() || n < 0.0 {
                            return Err(bad());
                        }
                        n
                    };
                }
                "animation-direction" => {
                    if !matches!(
                        token,
                        "normal" | "reverse" | "alternate" | "alternate-reverse"
                    ) {
                        return Err(bad());
                    }
                    a.direction = direction_of(token);
                }
                _ => {
                    if !matches!(token, "none" | "forwards" | "backwards" | "both") {
                        return Err(bad());
                    }
                    a.fill = fill_of(token);
                }
            }
        }
    }
    for a in &list {
        if a.duration <= 0.0 {
            return Err(format!(
                "{:?} has no duration; set animation-duration or use the shorthand",
                a.name
            ));
        }
    }
    Ok(list)
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
            "filter" => {
                v.blur = Some(if value == "none" {
                    0.0
                } else if let Some(args) = call(value, "blur") {
                    match args.as_slice() {
                        [px] => match pixels(px)? {
                            Shift::Px(p) => p.max(0.0),
                            Shift::Percent(_) => {
                                return Err(format!("blur takes a length, not {px:?}"));
                            }
                        },
                        _ => return Err(format!("{value:?}: blur takes one length")),
                    }
                } else {
                    return Err(format!(
                        "{value:?} is not a filter geneva animates; use none or blur(<length>)"
                    ));
                });
            }
            "color" => {
                v.color =
                    Some(Color::from_str(value).map_err(|_| format!("{value:?} is not a colour"))?);
            }
            "text-shadow" => v.text_shadow = Some(text_shadow(value)?),
            "letter-spacing" => v.letter_spacing = Some(spacing(value)?),
            "width" => v.width = Some(pixels(value)?),
            "height" => v.height = Some(pixels(value)?),
            "max-width" => v.max_width = Some(pixels(value)?),
            "min-width" => v.min_width = Some(pixels(value)?),
            "background-position" => v.background_position = Some(position_pair(value)?),
            "clip-path" => v.clip_path = Some(clip_polygon(value)?),
            _ => {
                return Err(format!(
                    "{property:?} cannot be animated; a keyframe sets transform, translate, \
scale, rotate, opacity, filter, color, text-shadow, letter-spacing, width, height, \
max-width, min-width, background-position or clip-path"
                ));
            }
        }
    }
    Ok(v)
}

/// `text-shadow` in a keyframe: `none`, or a list of shadows, each two or
/// three lengths and a colour in any order.
fn text_shadow(value: &str) -> Result<Vec<TextShadow>, String> {
    if value == "none" {
        return Ok(Vec::new());
    }
    split_outside_parens(value, ',')
        .into_iter()
        .map(|one| one_text_shadow(one.trim()))
        .collect()
}

fn one_text_shadow(value: &str) -> Result<TextShadow, String> {
    let mut lengths = Vec::new();
    let mut color = None;
    for token in tokens(value) {
        if let Ok(Shift::Px(p)) = pixels(token) {
            lengths.push(p);
        } else if let Ok(c) = Color::from_str(token) {
            color = Some(c);
        } else {
            return Err(format!(
                "{value:?}: {token:?} is neither a length nor a colour"
            ));
        }
    }
    match lengths.as_slice() {
        [x, y] | [x, y, _] => Ok(TextShadow {
            x: *x,
            y: *y,
            blur: lengths.get(2).copied().unwrap_or(0.0).max(0.0),
            color: color.unwrap_or(Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.5,
            }),
        }),
        _ => Err(format!(
            "{value:?}: a text-shadow is \"<x> <y> [blur] [color]\", a list of them, or none"
        )),
    }
}

/// `clip-path` in a keyframe: `none`, or `polygon()` with its points as
/// pairs of lengths or percentages.
fn clip_polygon(value: &str) -> Result<Vec<[Shift; 2]>, String> {
    if value == "none" {
        return Ok(Vec::new());
    }
    let Some(args) = call(value, "polygon") else {
        return Err(format!(
            "{value:?} is not a clip-path geneva draws; use none or polygon(x y, ...)"
        ));
    };
    let points: Vec<[Shift; 2]> = args
        .iter()
        .map(|pair| match tokens(pair).as_slice() {
            [x, y] => Ok([pixels(x)?, pixels(y)?]),
            _ => Err(format!("{pair:?}: a polygon point is two values")),
        })
        .collect::<Result<_, _>>()?;
    if points.len() < 3 {
        return Err(format!("{value:?}: a polygon takes at least three points"));
    }
    Ok(points)
}

/// `background-position` in a keyframe: one or two lengths, percentages
/// or side keywords.
fn position_pair(value: &str) -> Result<[Shift; 2], String> {
    let one = |t: &str| match t {
        "left" | "top" => Ok(Shift::Percent(0.0)),
        "center" => Ok(Shift::Percent(50.0)),
        "right" | "bottom" => Ok(Shift::Percent(100.0)),
        _ => pixels(t),
    };
    let t = tokens(value);
    match t.as_slice() {
        [x] => Ok([one(x)?, Shift::Percent(50.0)]),
        [x, y] => Ok([one(x)?, one(y)?]),
        _ => Err(format!(
            "invalid background-position {value:?}; write one or two values"
        )),
    }
}

/// Composes two translations. Two of a kind add; a pixel distance and a
/// percentage cannot, so the later one wins, as the last `transform`
/// function of a kind does in a browser when they cannot be folded.
fn add(base: Option<[Shift; 2]>, d: [Shift; 2]) -> [Shift; 2] {
    let b = base.unwrap_or([Shift::Px(0.0), Shift::Px(0.0)]);
    let one = |a: Shift, b: Shift| match (a, b) {
        (Shift::Px(x), Shift::Px(y)) => Shift::Px(x + y),
        (Shift::Percent(x), Shift::Percent(y)) => Shift::Percent(x + y),
        (Shift::Px(0.0), other) | (other, Shift::Px(0.0)) => other,
        (_, later) => later,
    };
    [one(b[0], d[0]), one(b[1], d[1])]
}

fn mul(base: Option<[f64; 2]>, d: [f64; 2]) -> [f64; 2] {
    let b = base.unwrap_or([1.0, 1.0]);
    [b[0] * d[0], b[1] * d[1]]
}

/// One or two distances, as the `translate` property takes.
fn translate_pair(value: &str) -> Result<[Shift; 2], String> {
    let t = tokens(value);
    match t.as_slice() {
        [x] => Ok([pixels(x)?, Shift::Px(0.0)]),
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
        // CSS's identity transform, which is what "to { transform: none }"
        // at the end of a slide means.
        v.translate = Some([Shift::Px(0.0), Shift::Px(0.0)]);
        v.scale = Some([1.0, 1.0]);
        v.rotate = Some(0.0);
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
                let y = two(1).map(pixels).transpose()?.unwrap_or(Shift::Px(0.0));
                v.translate = Some(add(v.translate, [x, y]));
            }
            "translateX" => {
                v.translate = Some(add(v.translate, [pixels(args[0])?, Shift::Px(0.0)]));
            }
            "translateY" => {
                v.translate = Some(add(v.translate, [Shift::Px(0.0), pixels(args[0])?]));
            }
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
    fn a_keyframe_letter_spacing_keeps_its_ems_for_the_element() {
        let at = |s: &str| parse_declarations(s).map(|v| v.letter_spacing);
        assert_eq!(at("letter-spacing: .3em"), Ok(Some(Spacing::Em(0.3))));
        assert_eq!(at("letter-spacing: -2px"), Ok(Some(Spacing::Px(-2.0))));
        assert_eq!(at("letter-spacing: normal"), Ok(Some(Spacing::Px(0.0))));
        assert!(at("letter-spacing: 10%").is_err());
        assert!(at("letter-spacing: 1rem").is_err());
        assert_eq!(Spacing::Em(0.3).to_px(30.0), 9.0);
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
        assert_eq!(v.translate, Some([Shift::Px(-656.0), Shift::Px(0.0)]));
        assert_eq!(v.rotate, Some(90.0));
        assert_eq!(v.opacity, Some(0.0));
        assert_eq!(v.scale, None);

        let v = parse_declarations("translate: 10px 20px; scale: 2; rotate: 45deg").unwrap();
        assert_eq!(v.translate, Some([Shift::Px(10.0), Shift::Px(20.0)]));
        assert_eq!(v.scale, Some([2.0, 2.0]));
        assert_eq!(v.rotate, Some(45.0));

        // Functions of the same kind compose.
        let v =
            parse_declarations("transform: translateX(10px) translateY(20px) scale(2) scaleX(3)")
                .unwrap();
        assert_eq!(v.translate, Some([Shift::Px(10.0), Shift::Px(20.0)]));
        assert_eq!(v.scale, Some([6.0, 2.0]));
    }

    #[test]
    fn translate_takes_percentages_of_the_clips_own_box() {
        let v = parse_declarations("transform: translateX(-100%)").unwrap();
        assert_eq!(v.translate, Some([Shift::Percent(-100.0), Shift::Px(0.0)]));
        assert_eq!(Shift::Percent(-100.0).to_px(Some(560.0)), Some(-560.0));
        assert_eq!(Shift::Percent(-100.0).to_px(None), None);
        assert_eq!(Shift::Px(-8.0).to_px(None), Some(-8.0));

        let v = parse_declarations("translate: -50% 25%").unwrap();
        assert_eq!(
            v.translate,
            Some([Shift::Percent(-50.0), Shift::Percent(25.0)])
        );
    }

    #[test]
    fn transform_none_is_the_identity() {
        let v = parse_declarations("transform: none").unwrap();
        assert_eq!(v.translate, Some([Shift::Px(0.0), Shift::Px(0.0)]));
        assert_eq!(v.scale, Some([1.0, 1.0]));
        assert_eq!(v.rotate, Some(0.0));
    }

    #[test]
    fn rejects_what_it_cannot_animate() {
        for bad in [
            "margin: 1px",
            "transform: skew(10deg)",
            "opacity",
            "transform: translateX()",
        ] {
            assert!(parse_declarations(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn steps_and_linear_are_timing_functions() {
        let a = parse_animations("blink 1s steps(1, end)").unwrap();
        assert_eq!(
            a[0].easing,
            Easing::Steps {
                steps: (1, StepPosition::JumpEnd)
            }
        );
        let a = parse_animations("go 1s steps(3, jump-none)").unwrap();
        assert_eq!(
            a[0].easing,
            Easing::Steps {
                steps: (3, StepPosition::JumpNone)
            }
        );
        // Inputs left out are spread evenly; two on one entry are two
        // points.
        let a = parse_animations("go 1s linear(0, .5 30% 60%, .8, 1)").unwrap();
        assert_eq!(
            a[0].easing,
            Easing::Linear {
                linear: vec![[0.0, 0.0], [0.3, 0.5], [0.6, 0.5], [0.8, 0.8], [1.0, 1.0]]
            }
        );
        assert!(parse_animations("go 1s steps(0)").is_err());
        assert!(parse_animations("go 1s linear(1)").is_err());
    }

    #[test]
    fn a_text_shadow_list_is_a_keyframe_value() {
        let v = parse_declarations(
            "text-shadow: 0 0 4px rgba(239, 251, 238, 0.7), 0 0 12px rgba(0, 238, 225, 0.5)",
        )
        .unwrap();
        let list = v.text_shadow.expect("shadows");
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].blur, 12.0);
        assert_eq!(
            parse_declarations("text-shadow: none").unwrap().text_shadow,
            Some(Vec::new())
        );
        assert!(parse_declarations("text-shadow: 1px").is_err());
    }

    #[test]
    fn a_clip_path_polygon_is_a_keyframe_value() {
        let v =
            parse_declarations("clip-path: polygon(0% 0%, 100% 0%, 100% 200%, 0 260%)").unwrap();
        let p = v.clip_path.expect("a polygon");
        assert_eq!(p.len(), 4);
        assert_eq!(p[2], [Shift::Percent(100.0), Shift::Percent(200.0)]);
        assert_eq!(p[3], [Shift::Px(0.0), Shift::Percent(260.0)]);
        assert_eq!(
            parse_declarations("clip-path: none").unwrap().clip_path,
            Some(Vec::new())
        );
        assert!(parse_declarations("clip-path: circle(50%)").is_err());
        assert!(parse_declarations("clip-path: polygon(0 0, 1px 1px)").is_err());
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
