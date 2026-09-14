//! Computed styles: the property subset, the cascade, and the mapping
//! onto taffy's layout style.
//!
//! Layout properties go straight into a [`taffy::Style`], so the flexbox
//! and block behaviour is taffy's rather than an approximation of it.
//! What is left over is paint (backgrounds, borders, shadows) and text
//! (font, colour, alignment), which the renderer needs after layout.

use std::str::FromStr;

use geneva_color::Color;
use taffy::geometry::{Rect, Size};
use taffy::style::{
    AlignItems, AlignSelf, BoxSizing, Dimension, Display, FlexDirection, FlexWrap, JustifyContent,
    LengthPercentage, LengthPercentageAuto, Overflow, Position, Style,
};

use crate::css::{Declaration, Stylesheet, parse_declarations};
use crate::dom::Document;

/// The default font size, which `rem` and a bare `%` refer to.
pub const ROOT_FONT_SIZE: f64 = 16.0;

/// How a box's text is aligned inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// Against the start edge.
    #[default]
    Left,
    /// Centred.
    Center,
    /// Against the end edge.
    Right,
}

/// A drop shadow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// Horizontal offset in pixels.
    pub x: f64,
    /// Vertical offset in pixels.
    pub y: f64,
    /// Blur radius in pixels.
    pub blur: f64,
    /// Shadow colour.
    pub color: Color,
}

/// What painting needs after layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    /// Background fill, if any.
    pub background: Option<Color>,
    /// Border colour per side, in CSS order: top, right, bottom, left.
    pub border_color: [Color; 4],
    /// Corner radii in pixels: top-left, top-right, bottom-right, bottom-left.
    pub radius: [f64; 4],
    /// A single box shadow.
    pub shadow: Option<Shadow>,
    /// Multiplied into everything the box and its children draw.
    pub opacity: f64,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            background: None,
            border_color: [Color::from_rgba8(0, 0, 0, 0); 4],
            radius: [0.0; 4],
            shadow: None,
            opacity: 1.0,
        }
    }
}

/// The text properties, all of which inherit.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// Text colour.
    pub color: Color,
    /// Font family or font asset id.
    pub family: Option<String>,
    /// Size in pixels.
    pub size: f64,
    /// Weight from 100 to 900.
    pub weight: u16,
    /// Italic.
    pub italic: bool,
    /// Multiple of the font size.
    pub line_height: f64,
    /// Extra space between characters, in pixels.
    pub letter_spacing: f64,
    /// Horizontal alignment.
    pub align: TextAlign,
    /// Whether runs of whitespace and newlines are kept.
    pub pre: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            color: Color::from_rgba8(0, 0, 0, 255),
            family: None,
            size: ROOT_FONT_SIZE,
            weight: 400,
            italic: false,
            line_height: 1.2,
            letter_spacing: 0.0,
            align: TextAlign::Left,
            pre: false,
        }
    }
}

/// Everything one element's style says.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Computed {
    /// Layout, for taffy.
    pub layout: Style,
    /// Painting.
    pub paint: Paint,
    /// Text, inherited by children.
    pub text: Text,
    /// The `animation` shorthand, kept as written. Nothing here plays it;
    /// the clip that draws the markup does.
    pub animation: Option<String>,
}

/// The user-agent style for the handful of tags that carry one, so a
/// document looks the way it does in a browser without a reset.
fn user_agent(tag: &str) -> &'static str {
    match tag {
        "h1" => "display: block; font-size: 2em; font-weight: 700",
        "h2" => "display: block; font-size: 1.5em; font-weight: 700",
        "h3" => "display: block; font-size: 1.17em; font-weight: 700",
        "b" | "strong" => "font-weight: 700",
        "i" | "em" => "font-style: italic",
        "small" => "font-size: 0.8em",
        _ => "",
    }
}

/// An element that would load something in a browser and loads nothing
/// here, named so that the difference is visible.
fn unfollowed(el: &crate::dom::Element) -> Option<String> {
    match el.tag.as_str() {
        "link" => {
            let rel = el.attrs.get("rel").map_or("", String::as_str);
            rel.eq_ignore_ascii_case("stylesheet").then(|| {
                let href = el.attrs.get("href").map_or("", String::as_str);
                format!(
                    "<link rel=\"stylesheet\" href=\"{href}\">: geneva does not fetch \
stylesheets; put the rules in a <style> element or in the source's \"css\""
                )
            })
        }
        "iframe" | "object" | "embed" | "video" | "canvas" | "svg" => Some(format!(
            "<{}> is not drawn; geneva draws boxes, text and images",
            el.tag
        )),
        _ => None,
    }
}

/// Where a declaration came from, which orders the cascade.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Origin {
    important: bool,
    /// User-agent declarations lose to everything an author writes.
    author: bool,
    specificity: (u32, u32, u32),
    order: usize,
}

/// Computes a style for every node of the document.
///
/// Returns one entry per node id; text nodes get their parent's style,
/// which is what the renderer needs to draw them.
pub fn cascade(doc: &Document, sheet: &Stylesheet) -> (Vec<Computed>, Vec<String>) {
    let mut out = vec![Computed::default(); doc.nodes.len()];
    let mut problems = Vec::new();
    let mut stack = vec![(doc.root, Computed::default())];
    while let Some((id, inherited)) = stack.pop() {
        let mut computed = Computed {
            layout: Style::default(),
            paint: Paint::default(),
            // Only the text properties come down from the parent.
            text: inherited.text.clone(),
            animation: None,
        };
        if let Some(el) = doc.nodes[id].element() {
            // The root is the drawing surface, the way <body> is the page:
            // a block box the size of the box the document is drawn into,
            // which is what a child's percentages resolve against.
            if id == doc.root {
                computed.layout.display = Display::Block;
            }
            // Elements that pull in something geneva does not follow would
            // otherwise draw nothing and say nothing, which is the one
            // thing this crate promises not to do.
            if let Some(what) = unfollowed(el) {
                problems.push(what);
            }
            let mut declarations: Vec<(Origin, Declaration)> = Vec::new();
            for d in parse_declarations(user_agent(&el.tag)) {
                declarations.push((
                    Origin {
                        important: d.important,
                        author: false,
                        specificity: (0, 0, 0),
                        order: 0,
                    },
                    d,
                ));
            }
            for (i, rule) in sheet.rules.iter().enumerate() {
                let Some(best) = rule
                    .selectors
                    .iter()
                    .filter(|s| s.matches(doc, id))
                    .map(crate::css::Selector::specificity)
                    .max()
                else {
                    continue;
                };
                for d in &rule.declarations {
                    declarations.push((
                        Origin {
                            important: d.important,
                            author: true,
                            specificity: best,
                            order: i,
                        },
                        d.clone(),
                    ));
                }
            }
            if let Some(inline) = &el.style {
                for d in parse_declarations(inline) {
                    declarations.push((
                        Origin {
                            important: d.important,
                            author: true,
                            // A style attribute beats every selector.
                            specificity: (u32::MAX, u32::MAX, u32::MAX),
                            order: usize::MAX,
                        },
                        d,
                    ));
                }
            }
            declarations.sort_by_key(|(o, _)| *o);

            // Font size first: `em` lengths everywhere else refer to it.
            for (_, d) in &declarations {
                if matches!(d.property.as_str(), "font-size" | "font") {
                    let _ = apply(&d.property, &d.value, &mut computed, inherited.text.size);
                }
            }
            // `em` in `font-size` itself refers to the parent's size, so
            // the pass above settled it; the value it lands on again below
            // is discarded.
            let em = computed.text.size;
            for (_, d) in &declarations {
                if let Err(e) = apply(&d.property, &d.value, &mut computed, em) {
                    let where_ = el.id.as_deref().map_or_else(
                        || format!("<{}>", el.tag),
                        |i| format!("<{} id=\"{i}\">", el.tag),
                    );
                    problems.push(format!("{where_}: {e}"));
                }
            }
            computed.text.size = em;
        }
        for child in doc.children(id).iter().rev() {
            stack.push((*child, computed.clone()));
        }
        out[id] = computed;
    }
    (out, problems)
}

/// Applies one declaration. An unknown property is an error rather than
/// something quietly ignored.
#[allow(clippy::too_many_lines)]
fn apply(property: &str, value: &str, c: &mut Computed, em: f64) -> Result<(), String> {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    let l = lower.as_str();
    match property {
        "display" => {
            c.layout.display = match l {
                "flex" => Display::Flex,
                "block" => Display::Block,
                "none" => Display::None,
                _ => return unsupported(property, v, "flex, block or none"),
            };
        }
        "position" => {
            c.layout.position = match l {
                "relative" | "static" => Position::Relative,
                "absolute" => Position::Absolute,
                _ => return unsupported(property, v, "relative or absolute"),
            };
        }
        "box-sizing" => {
            c.layout.box_sizing = match l {
                "border-box" => BoxSizing::BorderBox,
                "content-box" => BoxSizing::ContentBox,
                _ => return unsupported(property, v, "border-box or content-box"),
            };
        }
        "overflow" => {
            let o = overflow(l)?;
            c.layout.overflow = taffy::Point { x: o, y: o };
        }
        "overflow-x" => c.layout.overflow.x = overflow(l)?,
        "overflow-y" => c.layout.overflow.y = overflow(l)?,

        "width" => c.layout.size.width = dimension(v, em)?,
        "height" => c.layout.size.height = dimension(v, em)?,
        "min-width" => c.layout.min_size.width = dimension(v, em)?,
        "min-height" => c.layout.min_size.height = dimension(v, em)?,
        "max-width" => c.layout.max_size.width = dimension(v, em)?,
        "max-height" => c.layout.max_size.height = dimension(v, em)?,
        "aspect-ratio" => c.layout.aspect_ratio = Some(ratio(v)? as f32),

        "margin" => c.layout.margin = sides(v, em, length_percentage_auto)?,
        "margin-top" => c.layout.margin.top = length_percentage_auto(v, em)?,
        "margin-right" => c.layout.margin.right = length_percentage_auto(v, em)?,
        "margin-bottom" => c.layout.margin.bottom = length_percentage_auto(v, em)?,
        "margin-left" => c.layout.margin.left = length_percentage_auto(v, em)?,

        "padding" => c.layout.padding = sides(v, em, length_percentage)?,
        "padding-top" => c.layout.padding.top = length_percentage(v, em)?,
        "padding-right" => c.layout.padding.right = length_percentage(v, em)?,
        "padding-bottom" => c.layout.padding.bottom = length_percentage(v, em)?,
        "padding-left" => c.layout.padding.left = length_percentage(v, em)?,

        "top" => c.layout.inset.top = length_percentage_auto(v, em)?,
        "right" => c.layout.inset.right = length_percentage_auto(v, em)?,
        "bottom" => c.layout.inset.bottom = length_percentage_auto(v, em)?,
        "left" => c.layout.inset.left = length_percentage_auto(v, em)?,
        "inset" => c.layout.inset = sides(v, em, length_percentage_auto)?,

        "border" => border_shorthand(v, em, c, [true; 4])?,
        "border-top" => border_shorthand(v, em, c, [true, false, false, false])?,
        "border-right" => border_shorthand(v, em, c, [false, true, false, false])?,
        "border-bottom" => border_shorthand(v, em, c, [false, false, true, false])?,
        "border-left" => border_shorthand(v, em, c, [false, false, false, true])?,
        "border-width" => c.layout.border = sides(v, em, length_percentage)?,
        "border-color" => {
            let s: Rect<Color> = sides(v, em, |t, _| color(t))?;
            c.paint.border_color = [s.top, s.right, s.bottom, s.left];
        }
        "border-style" => {
            if l != "solid" && l != "none" {
                return unsupported(property, v, "solid or none");
            }
        }
        "border-radius" => {
            let r: Rect<f64> = sides(v, em, pixels)?;
            // A Rect built from the CSS shorthand order is top, right,
            // bottom, left; radii go round from the top left corner.
            c.paint.radius = [r.top, r.right, r.bottom, r.left];
        }

        "flex-direction" => {
            c.layout.flex_direction = match l {
                "row" => FlexDirection::Row,
                "row-reverse" => FlexDirection::RowReverse,
                "column" => FlexDirection::Column,
                "column-reverse" => FlexDirection::ColumnReverse,
                _ => return unsupported(property, v, "row, row-reverse, column or column-reverse"),
            };
        }
        "flex-wrap" => {
            c.layout.flex_wrap = match l {
                "nowrap" => FlexWrap::NoWrap,
                "wrap" => FlexWrap::Wrap,
                "wrap-reverse" => FlexWrap::WrapReverse,
                _ => return unsupported(property, v, "nowrap, wrap or wrap-reverse"),
            };
        }
        "justify-content" => c.layout.justify_content = Some(justify(l, property, v)?),
        "align-items" => c.layout.align_items = Some(align(l, property, v)?),
        "align-self" => c.layout.align_self = Some(align(l, property, v)?),
        "align-content" => c.layout.align_content = Some(justify(l, property, v)?),
        "gap" => {
            let s: Rect<LengthPercentage> = sides(v, em, length_percentage)?;
            c.layout.gap = Size {
                width: s.right,
                height: s.top,
            };
        }
        "row-gap" => c.layout.gap.height = length_percentage(v, em)?,
        "column-gap" => c.layout.gap.width = length_percentage(v, em)?,
        "flex-grow" => c.layout.flex_grow = number(v)? as f32,
        "flex-shrink" => c.layout.flex_shrink = number(v)? as f32,
        "flex-basis" => c.layout.flex_basis = dimension(v, em)?,
        "flex" => flex_shorthand(v, em, c)?,
        "order" => {} // Ordering is not implemented; the source order stands.

        "animation" => c.animation = Some(v.to_owned()),
        "background" | "background-color" => c.paint.background = Some(color(v)?),
        "opacity" => c.paint.opacity = number(v)?.clamp(0.0, 1.0),
        "box-shadow" => c.paint.shadow = shadow(v, em)?,

        "color" => c.text.color = color(v)?,
        "font-family" => c.text.family = Some(family(v)),
        "font-size" => c.text.size = pixels(v, em)?,
        "font-weight" => {
            c.text.weight = match l {
                "normal" => 400,
                "bold" | "bolder" => 700,
                "lighter" => 300,
                _ => number(v)?.clamp(100.0, 900.0) as u16,
            };
        }
        "font-style" => c.text.italic = l == "italic" || l == "oblique",
        "font" => font_shorthand(v, em, c)?,
        "line-height" => {
            c.text.line_height = if l.ends_with("px") {
                pixels(v, em)? / c.text.size.max(1.0)
            } else if l == "normal" {
                1.2
            } else {
                number(v)?
            };
        }
        "letter-spacing" => {
            c.text.letter_spacing = if l == "normal" { 0.0 } else { pixels(v, em)? }
        }
        "text-align" => {
            c.text.align = match l {
                "left" | "start" => TextAlign::Left,
                "center" => TextAlign::Center,
                "right" | "end" => TextAlign::Right,
                _ => return unsupported(property, v, "left, center or right"),
            };
        }
        "white-space" => c.text.pre = matches!(l, "pre" | "pre-wrap" | "break-spaces"),
        _ => {
            return Err(format!(
                "{property:?} is not a property geneva draws; see the timeline reference for the list"
            ));
        }
    }
    Ok(())
}

fn unsupported<T>(property: &str, value: &str, expected: &str) -> Result<T, String> {
    Err(format!("{property}: {value:?} is not one of {expected}"))
}

fn overflow(l: &str) -> Result<Overflow, String> {
    match l {
        "visible" => Ok(Overflow::Visible),
        "hidden" | "clip" => Ok(Overflow::Hidden),
        "scroll" | "auto" => Ok(Overflow::Scroll),
        _ => unsupported("overflow", l, "visible, hidden or scroll"),
    }
}

fn justify(l: &str, property: &str, v: &str) -> Result<JustifyContent, String> {
    Ok(match l {
        "flex-start" | "start" => JustifyContent::FlexStart,
        "flex-end" | "end" => JustifyContent::FlexEnd,
        "center" => JustifyContent::Center,
        "space-between" => JustifyContent::SpaceBetween,
        "space-around" => JustifyContent::SpaceAround,
        "space-evenly" => JustifyContent::SpaceEvenly,
        "stretch" => JustifyContent::Stretch,
        _ => {
            return unsupported(
                property,
                v,
                "flex-start, flex-end, center, space-between, space-around, space-evenly or stretch",
            );
        }
    })
}

fn align(l: &str, property: &str, v: &str) -> Result<AlignItems, String> {
    Ok(match l {
        "flex-start" | "start" => AlignSelf::FlexStart,
        "flex-end" | "end" => AlignSelf::FlexEnd,
        "center" => AlignSelf::Center,
        "baseline" => AlignSelf::Baseline,
        "stretch" => AlignSelf::Stretch,
        _ => {
            return unsupported(
                property,
                v,
                "flex-start, flex-end, center, baseline or stretch",
            );
        }
    })
}

/// Splits a value into whitespace-separated parts, keeping `f(a, b)` whole.
fn parts(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start: Option<usize> = None;
    for (i, ch) in value.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if ch.is_whitespace() && depth == 0 {
            if let Some(s) = start.take() {
                out.push(&value[s..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(&value[s..]);
    }
    out
}

/// The one-to-four-value shorthand: top, right, bottom, left.
fn sides<T: Copy>(
    value: &str,
    em: f64,
    one: impl Fn(&str, f64) -> Result<T, String>,
) -> Result<Rect<T>, String> {
    let p = parts(value);
    let v: Vec<T> = p.iter().map(|t| one(t, em)).collect::<Result<_, _>>()?;
    Ok(match v.as_slice() {
        [a] => Rect {
            top: *a,
            right: *a,
            bottom: *a,
            left: *a,
        },
        [a, b] => Rect {
            top: *a,
            right: *b,
            bottom: *a,
            left: *b,
        },
        [a, b, c] => Rect {
            top: *a,
            right: *b,
            bottom: *c,
            left: *b,
        },
        [a, b, c, d] => Rect {
            top: *a,
            right: *b,
            bottom: *c,
            left: *d,
        },
        _ => return Err(format!("{value:?} takes one to four values")),
    })
}

fn number(token: &str) -> Result<f64, String> {
    token
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{token:?} is not a number"))
}

fn ratio(value: &str) -> Result<f64, String> {
    match value.split_once('/') {
        Some((a, b)) => {
            let (a, b) = (number(a)?, number(b)?);
            if b == 0.0 {
                return Err(format!("{value:?} divides by zero"));
            }
            Ok(a / b)
        }
        None => number(value),
    }
}

/// A length in pixels; `em` and `rem` resolve against the font size.
fn pixels(token: &str, em: f64) -> Result<f64, String> {
    let t = token.trim();
    for (suffix, unit) in [("px", 1.0), ("rem", ROOT_FONT_SIZE), ("em", em)] {
        if let Some(body) = t.strip_suffix(suffix) {
            return Ok(number(body)? * unit);
        }
    }
    if t == "0" {
        return Ok(0.0);
    }
    number(t).map_err(|_| format!("{token:?} is not a length; write it like \"12px\" or \"1.5em\""))
}

fn percent(token: &str) -> Option<f64> {
    token.trim().strip_suffix('%').and_then(|b| number(b).ok())
}

fn dimension(value: &str, em: f64) -> Result<Dimension, String> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("auto") {
        return Ok(Dimension::auto());
    }
    if let Some(p) = percent(v) {
        return Ok(Dimension::percent(p as f32 / 100.0));
    }
    Ok(Dimension::length(pixels(v, em)? as f32))
}

fn length_percentage(value: &str, em: f64) -> Result<LengthPercentage, String> {
    if let Some(p) = percent(value) {
        return Ok(LengthPercentage::percent(p as f32 / 100.0));
    }
    Ok(LengthPercentage::length(pixels(value, em)? as f32))
}

fn length_percentage_auto(value: &str, em: f64) -> Result<LengthPercentageAuto, String> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("auto") {
        return Ok(LengthPercentageAuto::auto());
    }
    if let Some(p) = percent(v) {
        return Ok(LengthPercentageAuto::percent(p as f32 / 100.0));
    }
    Ok(LengthPercentageAuto::length(pixels(v, em)? as f32))
}

fn color(value: &str) -> Result<Color, String> {
    Color::from_str(value.trim()).map_err(|e| format!("{value:?} is not a colour: {e}"))
}

fn family(value: &str) -> String {
    // Only the first family is used; there is no font matching to fall
    // back through, and the renderer already falls back on its own.
    value
        .split(',')
        .next()
        .unwrap_or(value)
        .trim()
        .trim_matches(['"', '\''])
        .to_owned()
}

/// `border: 5px solid #4ade80`, in any order, on the named sides.
fn border_shorthand(
    value: &str,
    em: f64,
    c: &mut Computed,
    sides: [bool; 4],
) -> Result<(), String> {
    let mut width: Option<f64> = None;
    let mut fill: Option<Color> = None;
    for token in parts(value) {
        let lower = token.to_ascii_lowercase();
        if lower == "solid" {
            continue;
        }
        if lower == "none" {
            width = Some(0.0);
            continue;
        }
        if let Ok(w) = pixels(token, em) {
            width = Some(w);
            continue;
        }
        fill = Some(color(token)?);
    }
    let w = LengthPercentage::length(width.unwrap_or(0.0) as f32);
    let slots = [
        (&mut c.layout.border.top, 0),
        (&mut c.layout.border.right, 1),
        (&mut c.layout.border.bottom, 2),
        (&mut c.layout.border.left, 3),
    ];
    for (slot, i) in slots {
        if sides[i] {
            *slot = w;
        }
    }
    if let Some(f) = fill {
        for (i, on) in sides.iter().enumerate() {
            if *on {
                c.paint.border_color[i] = f;
            }
        }
    }
    Ok(())
}

/// `flex: 1`, `flex: 1 1 auto`, `flex: none`.
fn flex_shorthand(value: &str, em: f64, c: &mut Computed) -> Result<(), String> {
    let p = parts(value);
    match p.as_slice() {
        [one] if one.eq_ignore_ascii_case("none") => {
            c.layout.flex_grow = 0.0;
            c.layout.flex_shrink = 0.0;
            c.layout.flex_basis = Dimension::auto();
        }
        [one] if one.eq_ignore_ascii_case("auto") => {
            c.layout.flex_grow = 1.0;
            c.layout.flex_shrink = 1.0;
            c.layout.flex_basis = Dimension::auto();
        }
        [grow] => {
            c.layout.flex_grow = number(grow)? as f32;
            c.layout.flex_shrink = 1.0;
            c.layout.flex_basis = Dimension::length(0.0);
        }
        [grow, second] => {
            c.layout.flex_grow = number(grow)? as f32;
            match number(second) {
                Ok(shrink) => c.layout.flex_shrink = shrink as f32,
                Err(_) => c.layout.flex_basis = dimension(second, em)?,
            }
        }
        [grow, shrink, basis] => {
            c.layout.flex_grow = number(grow)? as f32;
            c.layout.flex_shrink = number(shrink)? as f32;
            c.layout.flex_basis = dimension(basis, em)?;
        }
        _ => return Err(format!("{value:?} is not a flex shorthand")),
    }
    Ok(())
}

/// `font: 700 32px/1.25 Liberation Sans`.
fn font_shorthand(value: &str, em: f64, c: &mut Computed) -> Result<(), String> {
    let p = parts(value);
    let mut i = 0;
    while i < p.len() {
        let lower = p[i].to_ascii_lowercase();
        match lower.as_str() {
            "italic" | "oblique" => c.text.italic = true,
            "normal" => {}
            "bold" => c.text.weight = 700,
            _ => break,
        }
        i += 1;
        if let Ok(w) = p.get(i).map_or(Err(String::new()), |t| number(t)) {
            if (100.0..=900.0).contains(&w) && p.get(i + 1).is_some() {
                c.text.weight = w as u16;
                i += 1;
            }
        }
    }
    if i < p.len() {
        if let Ok(w) = number(p[i]) {
            if (100.0..=900.0).contains(&w) {
                c.text.weight = w as u16;
                i += 1;
            }
        }
    }
    let Some(size_part) = p.get(i) else {
        return Err(format!("{value:?} has no font size"));
    };
    let (size, line) = match size_part.split_once('/') {
        Some((s, l)) => (s, Some(l)),
        None => (*size_part, None),
    };
    c.text.size = pixels(size, em)?;
    if let Some(l) = line {
        c.text.line_height = if l.ends_with("px") {
            pixels(l, em)? / c.text.size.max(1.0)
        } else {
            number(l)?
        };
    }
    let rest = p[i + 1..].join(" ");
    if !rest.trim().is_empty() {
        c.text.family = Some(family(&rest));
    }
    Ok(())
}

/// `box-shadow: 0 2px 8px #0008`. `inset` and multiple shadows are not
/// drawn.
fn shadow(value: &str, em: f64) -> Result<Option<Shadow>, String> {
    if value.trim().eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    if value.contains(',') {
        return Err("only one box-shadow is drawn".to_owned());
    }
    if value.to_ascii_lowercase().contains("inset") {
        return Err("an inset box-shadow is not drawn".to_owned());
    }
    let p = parts(value);
    let mut lengths = Vec::new();
    let mut fill = Color::from_rgba8(0, 0, 0, 128);
    for token in p {
        match pixels(token, em) {
            Ok(v) if lengths.len() < 3 => lengths.push(v),
            _ => fill = color(token)?,
        }
    }
    if lengths.len() < 2 {
        return Err(format!(
            "{value:?} needs an x and y offset, and optionally a blur radius"
        ));
    }
    Ok(Some(Shadow {
        x: lengths[0],
        y: lengths[1],
        blur: lengths.get(2).copied().unwrap_or(0.0),
        color: fill,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parse_stylesheet;
    use crate::dom::parse;

    fn styled(html: &str) -> (Document, Vec<Computed>, Vec<String>) {
        let doc = parse(html).unwrap();
        let sheet = parse_stylesheet(&doc.style).unwrap();
        let (styles, problems) = cascade(&doc, &sheet);
        (doc, styles, problems)
    }

    #[test]
    fn the_cascade_orders_by_specificity_then_source() {
        let (doc, styles, problems) = styled(
            "<style>p { color: red } .a { color: green } #x { color: blue } \
             p { color: black }</style><p class=a id=x>hi</p>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let p = doc.children(doc.root)[0];
        assert_eq!(styles[p].text.color.to_hex(), "#0000ff");
    }

    #[test]
    fn an_inline_style_beats_every_selector() {
        let (doc, styles, _) =
            styled("<style>#x { color: blue !important }</style><p id=x style='color: red'>hi</p>");
        let p = doc.children(doc.root)[0];
        // !important still wins, as it does in a browser.
        assert_eq!(styles[p].text.color.to_hex(), "#0000ff");

        let (doc, styles, _) =
            styled("<style>#x { color: blue }</style><p id=x style='color: red'>hi</p>");
        let p = doc.children(doc.root)[0];
        assert_eq!(styles[p].text.color.to_hex(), "#ff0000");
    }

    #[test]
    fn text_properties_inherit_and_layout_does_not() {
        let (doc, styles, _) = styled(
            "<style>.card { color: #ff8800; font-size: 20px; padding: 4px }</style>\
             <div class=card><p>hi</p></div>",
        );
        let card = doc.children(doc.root)[0];
        let p = doc.children(card)[0];
        assert_eq!(styles[p].text.color.to_hex(), "#ff8800");
        assert_eq!(styles[p].text.size, 20.0);
        assert_eq!(styles[p].layout.padding.top, LengthPercentage::length(0.0));
        assert_eq!(
            styles[card].layout.padding.top,
            LengthPercentage::length(4.0)
        );
    }

    #[test]
    fn em_resolves_against_this_elements_font_size() {
        let (doc, styles, _) = styled(
            "<style>.a { font-size: 20px; padding: 2em } h1 { padding: 1em }</style>\
             <div class=a><h1>x</h1></div>",
        );
        let a = doc.children(doc.root)[0];
        let h1 = doc.children(a)[0];
        assert_eq!(styles[a].layout.padding.top, LengthPercentage::length(40.0));
        // h1 is 2em of its inherited 20px, so its own em is 40px.
        assert_eq!(styles[h1].text.size, 40.0);
        assert_eq!(
            styles[h1].layout.padding.top,
            LengthPercentage::length(40.0)
        );
    }

    #[test]
    fn shorthands_expand() {
        let (doc, styles, problems) = styled(
            "<style>.a { margin: 1px 2px 3px 4px; border: 5px solid #4ade80; \
             border-radius: 8px; flex: 1 0 auto; font: italic 700 32px/1.25 Liberation Sans; \
             box-shadow: 0 2px 8px #00000088 }</style><div class=a></div>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let a = doc.children(doc.root)[0];
        let s = &styles[a];
        assert_eq!(s.layout.margin.top, LengthPercentageAuto::length(1.0));
        assert_eq!(s.layout.margin.left, LengthPercentageAuto::length(4.0));
        assert_eq!(s.layout.border.left, LengthPercentage::length(5.0));
        assert_eq!(s.paint.border_color[3].to_hex(), "#4ade80");
        assert_eq!(s.paint.radius, [8.0; 4]);
        assert_eq!(s.layout.flex_grow, 1.0);
        assert_eq!(s.layout.flex_shrink, 0.0);
        assert!(s.text.italic);
        assert_eq!(s.text.weight, 700);
        assert_eq!(s.text.size, 32.0);
        assert!((s.text.line_height - 1.25).abs() < 1e-9);
        assert_eq!(s.text.family.as_deref(), Some("Liberation Sans"));
        let sh = s.paint.shadow.unwrap();
        assert_eq!((sh.x, sh.y, sh.blur), (0.0, 2.0, 8.0));
    }

    #[test]
    fn flexbox_properties_reach_taffy() {
        let (doc, styles, problems) = styled(
            "<style>.a { display: flex; flex-direction: column; justify-content: space-between; \
             align-items: center; flex-wrap: wrap; gap: 4px 12px }</style><div class=a></div>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let s = &styles[doc.children(doc.root)[0]];
        assert_eq!(s.layout.display, Display::Flex);
        assert_eq!(s.layout.flex_direction, FlexDirection::Column);
        assert_eq!(s.layout.justify_content, Some(JustifyContent::SpaceBetween));
        assert_eq!(s.layout.align_items, Some(AlignItems::Center));
        assert_eq!(s.layout.flex_wrap, FlexWrap::Wrap);
        assert_eq!(s.layout.gap.height, LengthPercentage::length(4.0));
        assert_eq!(s.layout.gap.width, LengthPercentage::length(12.0));
    }

    #[test]
    fn an_unsupported_property_is_named() {
        let (_, _, problems) = styled("<style>.a { float: left }</style><div class=a></div>");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("\"float\""), "{problems:?}");
        assert!(problems[0].contains("<div>"), "{problems:?}");
    }

    #[test]
    fn user_agent_styles_lose_to_the_author() {
        let (doc, styles, _) = styled("<style>h1 { font-weight: 400 }</style><h1>x</h1>");
        let h1 = doc.children(doc.root)[0];
        assert_eq!(styles[h1].text.weight, 400);
        assert_eq!(styles[h1].text.size, 32.0);
    }
}

#[cfg(test)]
mod unfollowed_tests {
    use super::*;
    use crate::css::parse_stylesheet;
    use crate::dom::parse;

    fn problems(html: &str) -> Vec<String> {
        let doc = parse(html).unwrap();
        let sheet = parse_stylesheet(&doc.style).unwrap();
        cascade(&doc, &sheet).1
    }

    #[test]
    fn a_stylesheet_link_is_named_rather_than_ignored() {
        let p = problems("<link rel='stylesheet' href='house.css'><div></div>");
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("house.css"), "{p:?}");
        assert!(p[0].contains("does not fetch"), "{p:?}");

        // A link that is not a stylesheet loads nothing in a browser either.
        assert!(problems("<link rel='icon' href='x.png'>").is_empty());
    }

    #[test]
    fn elements_with_their_own_renderer_are_named() {
        for tag in ["iframe", "svg", "canvas", "video"] {
            let p = problems(&format!("<{tag}></{tag}>"));
            assert_eq!(p.len(), 1, "{tag}");
            assert!(p[0].contains(tag), "{p:?}");
        }
    }
}
