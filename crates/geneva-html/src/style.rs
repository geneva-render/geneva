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

/// One colour stop of a gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    /// The colour at this point.
    pub color: Color,
    /// Where it sits along the gradient line, 0 to 1. `None` is spaced
    /// evenly between the stops that do say, as CSS does it.
    pub at: Option<f64>,
}

/// Which way a linear gradient runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    /// A CSS angle in degrees: 0 points up, 90 points right.
    Angle(f64),
    /// A corner. Its angle depends on the box's proportions, so it is
    /// worked out when the box is painted rather than when it is parsed.
    Corner {
        /// Towards the top edge rather than the bottom.
        top: bool,
        /// Towards the right edge rather than the left.
        right: bool,
    },
}

/// What fills a box behind its content.
#[derive(Debug, Clone, PartialEq)]
pub enum Background {
    /// One flat colour.
    Color(Color),
    /// A linear ramp across the box.
    Linear {
        /// Which way it runs.
        direction: Direction,
        /// Its stops, in order.
        stops: Vec<Stop>,
    },
    /// A ramp outward from a point.
    Radial {
        /// A circle rather than an ellipse fitted to the box.
        circle: bool,
        /// The centre, as a fraction of the box's width and height.
        at: (f64, f64),
        /// Its stops, in order.
        stops: Vec<Stop>,
    },
}

impl Background {
    /// Whether it marks any pixel at all, so a fully transparent fill
    /// costs nothing to skip.
    #[must_use]
    pub fn visible(&self) -> bool {
        match self {
            Self::Color(c) => c.a > 0.0,
            Self::Linear { stops, .. } | Self::Radial { stops, .. } => {
                stops.iter().any(|s| s.color.a > 0.0)
            }
        }
    }
}

/// A `background-size` or `background-position` value along one axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Extent {
    /// The box's own extent for a size; the start of the box for a
    /// position.
    Auto,
    /// Pixels.
    Px(f64),
    /// A share of the box for a size. For a position, CSS's rule: a
    /// share of the room left once the tile is subtracted from the box,
    /// so `50%` centres the tile and `100%` lands it against the far edge.
    Percent(f64),
}

impl Extent {
    /// The size of a tile along one axis, given the box's.
    #[must_use]
    pub fn size(self, of: f64) -> f64 {
        match self {
            Self::Auto => of,
            Self::Px(v) => v,
            Self::Percent(p) => of * p / 100.0,
        }
    }

    /// Where a tile starts along one axis, given the box's extent and
    /// the tile's.
    #[must_use]
    pub fn position(self, of: f64, tile: f64) -> f64 {
        match self {
            Self::Auto => 0.0,
            Self::Px(v) => v,
            Self::Percent(p) => (of - tile) * p / 100.0,
        }
    }
}

/// A background that fills the glyphs rather than the box, which is
/// what `background-clip: text` asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct TextFill {
    /// The colour or gradient.
    pub background: Background,
    /// The tile's size: `background-size`.
    pub size: (Extent, Extent),
    /// Where the tile starts: `background-position`.
    pub position: (Extent, Extent),
}

/// What painting needs after layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    /// Background fill, if any.
    pub background: Option<Background>,
    /// The size of the background's tile: `background-size`. It repeats
    /// across the box, as in CSS.
    pub background_size: (Extent, Extent),
    /// Where the background's tile starts: `background-position`.
    pub background_position: (Extent, Extent),
    /// `background-clip: text`. The cascade moves such a background onto
    /// the text as a [`TextFill`], so the box itself draws none.
    pub clip_text: bool,
    /// Border colour per side, in CSS order: top, right, bottom, left.
    pub border_color: [Color; 4],
    /// Corner radii in pixels: top-left, top-right, bottom-right, bottom-left.
    pub radius: [f64; 4],
    /// Box shadows, front to back as CSS lists them; `inset` is not read.
    pub shadow: Vec<Shadow>,
    /// Multiplied into everything the box and its children draw.
    pub opacity: f64,
    /// `filter: blur()`, in pixels of standard deviation, over everything
    /// the box and its children draw. Zero is no blur.
    pub blur: f64,
    /// `clip-path: polygon()`: the points, each a share or a length of
    /// the border box, outside which the box and its children draw
    /// nothing.
    pub clip_path: Option<Vec<(Extent, Extent)>>,
    /// `mix-blend-mode`: how the box and its children are mixed with
    /// what is behind them.
    pub blend: Blend,
    /// `transform-origin`: the point an animated transform turns and
    /// scales about, a share or a length of the border box from its
    /// top-left corner. The centre by default.
    pub transform_origin: (Extent, Extent),
    /// `backdrop-filter`: what is done to the picture behind the box,
    /// inside its border box, before the box is drawn over it. Empty for
    /// none.
    pub backdrop: Vec<Filter>,
}

/// One function of a `backdrop-filter`, applied in the order written to
/// sRGB-encoded values, as a browser does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Filter {
    /// `blur(<length>)`: a Gaussian of that standard deviation, in pixels.
    Blur(f64),
    /// `saturate(<number>|<percentage>)`: 1 leaves the colour as it is.
    Saturate(f64),
    /// `brightness(...)`: a factor on every channel.
    Brightness(f64),
    /// `contrast(...)`: a factor on every channel's distance from grey.
    Contrast(f64),
}

impl Filter {
    /// The blur's standard deviation, zero for the other functions.
    #[must_use]
    pub fn blur(self) -> f64 {
        match self {
            Self::Blur(r) => r,
            _ => 0.0,
        }
    }
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            background: None,
            background_size: (Extent::Auto, Extent::Auto),
            background_position: (Extent::Px(0.0), Extent::Px(0.0)),
            clip_text: false,
            border_color: [Color::from_rgba8(0, 0, 0, 0); 4],
            radius: [0.0; 4],
            shadow: Vec::new(),
            opacity: 1.0,
            blur: 0.0,
            clip_path: None,
            blend: Blend::default(),
            transform_origin: (Extent::Percent(50.0), Extent::Percent(50.0)),
            backdrop: Vec::new(),
        }
    }
}

/// How a group's picture is mixed with what is already behind it:
/// `mix-blend-mode`.
///
/// These are the separable modes of the CSS compositing specification
/// that geneva's compositor already implements at the clip level, and
/// they mean the same thing here. The modes it does not have,
/// `color-dodge`, `color-burn`, `hard-light` and `exclusion`, and the
/// four non-separable ones, are refused by name rather than approximated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    /// Laid over what is behind it, which is what a box does by default.
    #[default]
    Normal,
    /// The product of the two colours.
    Multiply,
    /// The inverse product of the inverted colours.
    Screen,
    /// Multiplies dark backdrops and screens light ones.
    Overlay,
    /// The darker of the two, per channel.
    Darken,
    /// The lighter of the two, per channel.
    Lighten,
    /// The absolute difference of the two.
    Difference,
    /// A gentler overlay.
    SoftLight,
}

impl Blend {
    /// The CSS keyword, for a diagnostic that names it back.
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
            Self::Darken => "darken",
            Self::Lighten => "lighten",
            Self::Difference => "difference",
            Self::SoftLight => "soft-light",
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
    /// Multiple of the font size; `None` for `normal`, the font's own
    /// spacing.
    pub line_height: Option<f64>,
    /// Extra space between characters, in pixels.
    pub letter_spacing: f64,
    /// Horizontal alignment.
    pub align: TextAlign,
    /// Whether runs of whitespace and newlines are kept.
    pub pre: bool,
    /// Shadows drawn behind the glyphs, front to back as CSS lists them.
    pub shadow: Vec<Shadow>,
    /// A background clipped to the glyphs, from an ancestor's
    /// `background-clip: text`. It stands in for `color` while it is set.
    /// Boxed so a style with no fill stays small.
    pub fill: Option<Box<TextFill>>,
    /// `-webkit-text-stroke-width` in pixels: a stroke centred on the
    /// glyph outline, so half of it lies outside the glyph.
    pub stroke_width: f64,
    /// `-webkit-text-stroke-color`; `None` is `currentcolor`.
    pub stroke_color: Option<Color>,
    /// Whether the stroke is painted over the fill, as `paint-order:
    /// normal` does; `stroke fill` puts it underneath.
    pub stroke_over_fill: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            color: Color::from_rgba8(0, 0, 0, 255),
            family: None,
            size: ROOT_FONT_SIZE,
            weight: 400,
            italic: false,
            line_height: None,
            letter_spacing: 0.0,
            align: TextAlign::Left,
            pre: false,
            shadow: Vec::new(),
            fill: None,
            stroke_width: 0.0,
            stroke_color: None,
            stroke_over_fill: true,
        }
    }
}

impl Text {
    /// The stroke's colour, `currentcolor` resolved.
    #[must_use]
    pub fn stroke_color(&self) -> Color {
        self.stroke_color.unwrap_or(self.color)
    }
}

/// An element's `animation`, kept as written: the shorthand and any
/// longhands set beside it. Nothing here plays it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnimationSpec {
    /// The `animation` shorthand.
    pub shorthand: Option<String>,
    /// Longhands in cascade order, each `(property, value)`, such as
    /// `("animation-delay", "0.2s, 0.4s")`. A shorthand written later
    /// clears the ones before it, as in CSS.
    pub longhands: Vec<(String, String)>,
}

/// What a frame changes on one element while an animation plays: the
/// properties a keyframe can set, each `None` where the style stands.
/// Applied to a copy of the computed styles before layout, so a box
/// whose width is animated is laid out afresh at each frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Overrides {
    /// `opacity`.
    pub opacity: Option<f64>,
    /// `filter: blur()`, in pixels.
    pub blur: Option<f64>,
    /// `color`.
    pub color: Option<Color>,
    /// `-webkit-text-stroke-color`.
    pub stroke_color: Option<Color>,
    /// `text-shadow`: the list, empty for `none`.
    pub text_shadow: Option<Vec<Shadow>>,
    /// `letter-spacing`, in pixels.
    pub letter_spacing: Option<f64>,
    /// `width`.
    pub width: Option<Extent>,
    /// `height`.
    pub height: Option<Extent>,
    /// `max-width`.
    pub max_width: Option<Extent>,
    /// `min-width`.
    pub min_width: Option<Extent>,
    /// `background-position`.
    pub background_position: Option<(Extent, Extent)>,
    /// `clip-path`: a polygon's points, or empty for `none`.
    pub clip_path: Option<Vec<(Extent, Extent)>>,
}

impl Overrides {
    /// Whether anything here changes where boxes land, so layout has to
    /// run again rather than only painting.
    #[must_use]
    pub fn moves_layout(&self) -> bool {
        self.letter_spacing.is_some()
            || self.width.is_some()
            || self.height.is_some()
            || self.max_width.is_some()
            || self.min_width.is_some()
    }

    /// Whether nothing is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
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
    /// The element's `animation`, kept as written. Nothing here plays
    /// it; the clip that draws the markup does, or the renderer for an
    /// element inside it.
    pub animation: Option<AnimationSpec>,
    /// Whether `position` was written as `absolute` or `relative`. taffy
    /// has no `static`, so the style alone cannot say, and `z-index`
    /// applies only to a box that is positioned or a flex item.
    pub positioned: bool,
    /// `z-index`, when it is an integer rather than `auto`.
    pub z_index: Option<i32>,
    /// Whether `display: flex` was written. A box that did not ask for it
    /// is still laid out as a flex container (taffy has no inline
    /// layout), but text with an inline element inside it is set as one
    /// line of text, as a browser sets a paragraph; asked for, the text
    /// and the element are flex items of their own, as they are there.
    pub flex_written: bool,
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

/// Elements that are never laid out.
const UNSEEN: &[&str] = &["head", "link", "meta", "script", "style", "title"];

/// Whether an element sits in a line of text, as an image or an inline
/// tag does, rather than making a block.
fn inline_level(tag: &str) -> bool {
    tag == "img" || crate::layout::INLINE.contains(&tag)
}

/// An element that a browser would render with something of its own and
/// geneva does not draw, named so that the difference is visible. A
/// `<link rel="stylesheet">` is not here: those are read.
fn unfollowed(el: &crate::dom::Element) -> Option<String> {
    match el.tag.as_str() {
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
pub fn cascade(doc: &Document, sheet: &Stylesheet) -> (Vec<Computed>, Vec<String>, Vec<bool>) {
    let mut out = vec![Computed::default(); doc.nodes.len()];
    let mut problems = Vec::new();
    // Which rules reached something, so a caller can name the ones that
    // parsed and then styled nothing.
    let mut used = vec![false; sheet.rules.len()];
    let mut stack = vec![(doc.root, Computed::default())];
    while let Some((id, inherited)) = stack.pop() {
        let mut computed = Computed {
            layout: Style::default(),
            paint: Paint::default(),
            // Only the text properties come down from the parent.
            text: inherited.text.clone(),
            animation: None,
            positioned: false,
            z_index: None,
            flex_written: false,
        };
        if let Some(el) = doc.nodes[id].element() {
            // CSS's default is content-box; taffy's is border-box, so it
            // is set here rather than inherited from the layout default.
            computed.layout.box_sizing = BoxSizing::ContentBox;
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
                used[i] = true;
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
                    // The property is named here rather than in each
                    // message, so every one of them carries it. Without it
                    // a reader gets the value that was refused and no way
                    // to tell which declaration it came from.
                    problems.push(format!("{where_}: {} {e}", d.property));
                }
            }
            computed.text.size = em;
            // Taffy's default display is flex, CSS's is block (or inline).
            // An element that says nothing and holds block-level elements
            // is a block, so they stack as in a browser rather than stand
            // in a row. One holding only text and inline elements keeps
            // the row, which is how its pieces sit side by side.
            let written = declarations.iter().any(|(_, d)| d.property == "display");
            if !written
                && id != doc.root
                && !inline_level(&el.tag)
                && doc.children(id).iter().any(|c| {
                    doc.nodes[*c]
                        .element()
                        .is_some_and(|e| !inline_level(&e.tag) && !UNSEEN.contains(&e.tag.as_str()))
                })
            {
                computed.layout.display = Display::Block;
            }
            // `background-clip: text` fills the glyphs, this element's
            // and its descendants', with what would have filled the
            // box. The text properties are what come down to them.
            if computed.paint.clip_text {
                if let Some(bg) = computed.paint.background.take() {
                    computed.text.fill = Some(Box::new(TextFill {
                        background: bg,
                        size: computed.paint.background_size,
                        position: computed.paint.background_position,
                    }));
                }
            }
        }
        for child in doc.children(id).iter().rev() {
            stack.push((*child, computed.clone()));
        }
        out[id] = computed;
    }
    (out, problems, used)
}

/// The styles with a frame's overrides applied. Text properties reach the
/// element's descendants where they were inherited from it: a descendant
/// that set its own keeps it, as it would under the cascade.
#[must_use]
pub fn overridden(
    doc: &Document,
    styles: &[Computed],
    overrides: &std::collections::BTreeMap<crate::dom::NodeId, Overrides>,
) -> Vec<Computed> {
    let mut out = styles.to_vec();
    for (id, o) in overrides {
        let Some(before) = styles.get(*id).cloned() else {
            continue;
        };
        let target = &mut out[*id];
        if let Some(v) = o.opacity {
            target.paint.opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = o.blur {
            target.paint.blur = v.max(0.0);
        }
        if let Some(v) = o.width {
            target.layout.size.width = dimension_of(v);
        }
        if let Some(v) = o.height {
            target.layout.size.height = dimension_of(v);
        }
        if let Some(v) = o.max_width {
            target.layout.max_size.width = dimension_of(v);
        }
        if let Some(v) = o.min_width {
            target.layout.min_size.width = dimension_of(v);
        }
        if let Some(v) = o.background_position {
            target.paint.background_position = v;
        }
        if let Some(v) = &o.clip_path {
            target.paint.clip_path = (!v.is_empty()).then(|| v.clone());
        }
        text_override(&mut out[*id].text, &before.text, o);
        // The element's own text style went to its descendants when the
        // cascade ran; the same values are updated there.
        let mut stack: Vec<crate::dom::NodeId> = doc.children(*id).to_vec();
        while let Some(n) = stack.pop() {
            text_override(&mut out[n].text, &before.text, o);
            stack.extend(doc.children(n).iter().copied());
        }
    }
    out
}

/// One node's text style under an override, changing only what still
/// matches the animated element's own value, which is what it inherited.
fn text_override(text: &mut Text, from: &Text, o: &Overrides) {
    if let Some(c) = o.color {
        if text.color == from.color {
            text.color = c;
        }
    }
    if let Some(c) = o.stroke_color {
        if text.stroke_color() == from.stroke_color() {
            text.stroke_color = Some(c);
        }
    }
    if let Some(sh) = &o.text_shadow {
        if text.shadow == from.shadow {
            text.shadow.clone_from(sh);
        }
    }
    if let Some(v) = o.letter_spacing {
        if text.letter_spacing == from.letter_spacing {
            text.letter_spacing = v;
        }
    }
    if let (Some(p), Some(fill)) = (o.background_position, text.fill.as_mut()) {
        if from
            .fill
            .as_ref()
            .is_some_and(|f| f.position == fill.position)
        {
            fill.position = p;
        }
    }
}

/// A taffy dimension as an [`Extent`], for a frame that has to start
/// from the value the style already has. Anything taffy can say that
/// CSS's `auto`, a length or a percentage cannot is read as `auto`.
#[must_use]
pub fn extent_of(d: Dimension) -> Extent {
    let raw = d.into_raw();
    if raw.tag() == taffy::CompactLength::LENGTH_TAG {
        Extent::Px(f64::from(raw.value()))
    } else if raw.tag() == taffy::CompactLength::PERCENT_TAG {
        Extent::Percent(f64::from(raw.value()) * 100.0)
    } else {
        Extent::Auto
    }
}

fn dimension_of(e: Extent) -> Dimension {
    match e {
        Extent::Auto => Dimension::auto(),
        Extent::Px(v) => Dimension::length(v as f32),
        Extent::Percent(p) => Dimension::percent((p / 100.0) as f32),
    }
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
            c.flex_written = l == "flex";
            c.layout.display = match l {
                "flex" => Display::Flex,
                "block" => Display::Block,
                "none" => Display::None,
                _ => return unsupported(v, "flex, block or none"),
            };
        }
        "position" => {
            c.layout.position = match l {
                "relative" | "static" => Position::Relative,
                "absolute" => Position::Absolute,
                _ => return unsupported(v, "static, relative or absolute"),
            };
            c.positioned = l != "static";
        }
        "z-index" => {
            c.z_index =
                match l {
                    "auto" => None,
                    _ => Some(l.parse::<i32>().map_err(|_| {
                        format!("\"z-index\" takes auto or a whole number, not {v:?}")
                    })?),
                };
        }
        "box-sizing" => {
            c.layout.box_sizing = match l {
                "border-box" => BoxSizing::BorderBox,
                "content-box" => BoxSizing::ContentBox,
                _ => return unsupported(v, "border-box or content-box"),
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
                return unsupported(v, "solid or none");
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
                _ => return unsupported(v, "row, row-reverse, column or column-reverse"),
            };
        }
        "flex-wrap" => {
            c.layout.flex_wrap = match l {
                "nowrap" => FlexWrap::NoWrap,
                "wrap" => FlexWrap::Wrap,
                "wrap-reverse" => FlexWrap::WrapReverse,
                _ => return unsupported(v, "nowrap, wrap or wrap-reverse"),
            };
        }
        "justify-content" => c.layout.justify_content = Some(justify(l, v)?),
        "align-items" => c.layout.align_items = Some(align(l, v)?),
        "align-self" => c.layout.align_self = Some(align(l, v)?),
        "align-content" => c.layout.align_content = Some(justify(l, v)?),
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

        "animation" => {
            c.animation = Some(AnimationSpec {
                shorthand: Some(v.to_owned()),
                longhands: Vec::new(),
            });
        }
        "animation-name"
        | "animation-duration"
        | "animation-delay"
        | "animation-timing-function"
        | "animation-iteration-count"
        | "animation-direction"
        | "animation-fill-mode" => {
            c.animation
                .get_or_insert_with(AnimationSpec::default)
                .longhands
                .push((property.to_owned(), v.to_owned()));
        }
        "transform-origin" => c.paint.transform_origin = transform_origin(v, em)?,
        "clip-path" => {
            c.paint.clip_path = if l == "none" {
                None
            } else if let Some(args) = function(l, v, "polygon") {
                let points: Vec<(Extent, Extent)> = top_level_commas(args)
                    .iter()
                    .map(|pair| match parts(pair).as_slice() {
                        [x, y] => Ok((extent(x, em)?, extent(y, em)?)),
                        _ => Err(format!("{pair:?}: a polygon point is two values")),
                    })
                    .collect::<Result<_, _>>()?;
                if points.len() < 3 {
                    return Err(format!("{v:?}: a polygon takes at least three points"));
                }
                Some(points)
            } else {
                return unsupported(v, "none or polygon(x y, ...)");
            };
        }
        "backdrop-filter" | "-webkit-backdrop-filter" => c.paint.backdrop = filters(v, em)?,
        "filter" => {
            c.paint.blur = if l == "none" {
                0.0
            } else if let Some(arg) = function(l, v, "blur") {
                pixels(arg, em)?.max(0.0)
            } else {
                return unsupported(v, "none or blur(<length>)");
            };
        }
        "background" | "background-color" => c.paint.background = Some(background(v)?),
        "background-size" => c.paint.background_size = background_size(v, em)?,
        "background-position" => c.paint.background_position = background_position(v, em)?,
        "background-clip" | "-webkit-background-clip" => {
            c.paint.clip_text = match l {
                "text" => true,
                "border-box" => false,
                _ => return unsupported(v, "text or border-box"),
            };
        }
        "opacity" => c.paint.opacity = number(v)?.clamp(0.0, 1.0),
        "mix-blend-mode" => {
            c.paint.blend = match l {
                "normal" => Blend::Normal,
                "multiply" => Blend::Multiply,
                "screen" => Blend::Screen,
                "overlay" => Blend::Overlay,
                "darken" => Blend::Darken,
                "lighten" => Blend::Lighten,
                "difference" => Blend::Difference,
                "soft-light" => Blend::SoftLight,
                _ => {
                    return unsupported(
                        v,
                        "normal, multiply, screen, overlay, darken, lighten, difference or soft-light",
                    );
                }
            };
        }
        "box-shadow" => c.paint.shadow = shadow(v, em)?,

        "color" | "-webkit-text-fill-color" => c.text.color = color(v)?,
        "text-shadow" => c.text.shadow = shadow(v, em)?,
        "font-family" => c.text.family = Some(family(v)),
        "font-size" => c.text.size = size_or_percent(v, em)?,
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
            c.text.line_height = if let Some(p) = percent(v) {
                Some(p / 100.0)
            } else if l == "normal" {
                None
            } else if l.ends_with("px") || l.ends_with("em") || l.ends_with("rem") {
                Some(pixels(v, em)? / c.text.size.max(1.0))
            } else {
                Some(number(v)?)
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
                _ => return unsupported(v, "left, center or right"),
            };
        }
        "white-space" => c.text.pre = matches!(l, "pre" | "pre-wrap" | "break-spaces"),
        "-webkit-text-stroke" => {
            let (mut width, mut stroke) = (0.0, None);
            for token in parts(v) {
                match stroke_width(token, em) {
                    Ok(w) => width = w,
                    Err(_) => stroke = stroke_color(token)?,
                }
            }
            c.text.stroke_width = width;
            c.text.stroke_color = stroke;
        }
        "-webkit-text-stroke-width" => c.text.stroke_width = stroke_width(v.trim(), em)?,
        "-webkit-text-stroke-color" => c.text.stroke_color = stroke_color(v.trim())?,
        "paint-order" => c.text.stroke_over_fill = fill_before_stroke(l)?,
        _ => {
            return Err(
                "is not a property geneva draws; see the timeline reference for the list"
                    .to_owned(),
            );
        }
    }
    Ok(())
}

fn unsupported<T>(value: &str, expected: &str) -> Result<T, String> {
    Err(format!("{value:?} is not one of {expected}"))
}

fn overflow(l: &str) -> Result<Overflow, String> {
    match l {
        "visible" => Ok(Overflow::Visible),
        "hidden" | "clip" => Ok(Overflow::Hidden),
        "scroll" | "auto" => Ok(Overflow::Scroll),
        _ => unsupported(l, "visible, hidden or scroll"),
    }
}

fn justify(l: &str, v: &str) -> Result<JustifyContent, String> {
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
                v,
                "flex-start, flex-end, center, space-between, space-around, space-evenly or stretch",
            );
        }
    })
}

fn align(l: &str, v: &str) -> Result<AlignItems, String> {
    Ok(match l {
        "flex-start" | "start" => AlignSelf::FlexStart,
        "flex-end" | "end" => AlignSelf::FlexEnd,
        "center" => AlignSelf::Center,
        "baseline" => AlignSelf::Baseline,
        "stretch" => AlignSelf::Stretch,
        _ => {
            return unsupported(v, "flex-start, flex-end, center, baseline or stretch");
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

/// A font size: a length, or a percentage of the inherited size, which is
/// what `em` refers to here.
fn size_or_percent(value: &str, em: f64) -> Result<f64, String> {
    match percent(value) {
        Some(p) => Ok(em * p / 100.0),
        None => pixels(value, em),
    }
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

/// A stroke width: a length, or `thin`, `medium` or `thick` (1, 3 and
/// 5 px, as browsers draw them).
fn stroke_width(token: &str, em: f64) -> Result<f64, String> {
    match token.to_ascii_lowercase().as_str() {
        "thin" => Ok(1.0),
        "medium" => Ok(3.0),
        "thick" => Ok(5.0),
        _ => Ok(pixels(token, em)?.max(0.0)),
    }
}

/// A stroke colour; `currentcolor` is `None`, the text's own colour.
fn stroke_color(token: &str) -> Result<Option<Color>, String> {
    if token.eq_ignore_ascii_case("currentcolor") {
        Ok(None)
    } else {
        color(token).map(Some)
    }
}

/// Whether `paint-order` puts the fill before the stroke. The keywords
/// listed come first in the order given and the rest follow in the
/// default order, fill, stroke, markers; markers draw nothing on text.
fn fill_before_stroke(l: &str) -> Result<bool, String> {
    if l.trim() == "normal" {
        return Ok(true);
    }
    let mut order: Vec<&str> = Vec::new();
    for token in l.split_whitespace() {
        if !matches!(token, "fill" | "stroke" | "markers") || order.contains(&token) {
            return unsupported(l, "normal, or fill, stroke and markers in some order");
        }
        order.push(token);
    }
    for token in ["fill", "stroke", "markers"] {
        if !order.contains(&token) {
            order.push(token);
        }
    }
    let at = |k: &str| order.iter().position(|t| *t == k);
    Ok(at("fill") < at("stroke"))
}

fn family(value: &str) -> String {
    // The whole list, written plainly: the renderer falls back through
    // it a character at a time.
    font_list(value).join(", ")
}

/// The families of a `font-family` list, in order, unquoted:
/// `Inter, "Noto Sans Arabic", sans-serif` gives the three names.
#[must_use]
pub fn font_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|f| f.trim().trim_matches(['"', '\'']).trim().to_owned())
        .filter(|f| !f.is_empty())
        .collect()
}

/// Whether a family name is one of CSS's generic families, which name
/// whatever the machine has rather than a font.
#[must_use]
pub fn is_generic_family(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "serif"
            | "sans-serif"
            | "monospace"
            | "cursive"
            | "fantasy"
            | "system-ui"
            | "ui-serif"
            | "ui-sans-serif"
            | "ui-monospace"
            | "ui-rounded"
            | "emoji"
            | "math"
    )
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
    // The shorthand resets what it leaves out: no `/line-height` is
    // `normal`.
    c.text.line_height = match line {
        Some(l) if l.ends_with("px") => Some(pixels(l, em)? / c.text.size.max(1.0)),
        Some(l) => Some(number(l)?),
        None => None,
    };
    let rest = p[i + 1..].join(" ");
    if !rest.trim().is_empty() {
        c.text.family = Some(family(&rest));
    }
    Ok(())
}

/// `box-shadow: 0 2px 8px #0008, 0 0 2em #fff8`: a list, front to back.
/// `inset` is not drawn.
/// A background: one colour, or a gradient.
///
/// # Errors
///
/// Names what it could not read: a gradient geneva does not draw, a
/// `url()`, or a colour that is not one.
pub fn background(value: &str) -> Result<Background, String> {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    if let Some(rest) = function(&lower, v, "linear-gradient") {
        return linear_gradient(rest);
    }
    if let Some(rest) = function(&lower, v, "radial-gradient") {
        return radial_gradient(rest);
    }
    if lower.starts_with("repeating-") || lower.starts_with("conic-gradient") {
        return Err(format!(
            "{v:?}: geneva draws linear-gradient and radial-gradient, not this one"
        ));
    }
    if lower.starts_with("url(") {
        return Err(format!("{v:?}: a background image is not drawn"));
    }
    Ok(Background::Color(color(v)?))
}

/// A length or a percentage as an [`Extent`].
fn extent(token: &str, em: f64) -> Result<Extent, String> {
    match percent(token) {
        Some(p) => Ok(Extent::Percent(p)),
        None => pixels(token, em).map(Extent::Px),
    }
}

/// `transform-origin`: one to three values, keywords (`left`, `center`,
/// `right`, `top`, `bottom`), lengths or percentages. One value is the
/// horizontal one (or the vertical, for `top` and `bottom`), the other
/// centred; two are horizontal then vertical, unless the keywords say
/// otherwise; a third, the depth, is ignored.
fn transform_origin(value: &str, em: f64) -> Result<(Extent, Extent), String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Axis {
        Either,
        X,
        Y,
    }
    let one = |t: &str| -> Result<(Extent, Axis), String> {
        Ok(match t.to_ascii_lowercase().as_str() {
            "left" => (Extent::Percent(0.0), Axis::X),
            "right" => (Extent::Percent(100.0), Axis::X),
            "top" => (Extent::Percent(0.0), Axis::Y),
            "bottom" => (Extent::Percent(100.0), Axis::Y),
            "center" => (Extent::Percent(50.0), Axis::Either),
            _ => (extent(t, em)?, Axis::Either),
        })
    };
    let centre = Extent::Percent(50.0);
    let tokens = parts(value);
    match tokens.as_slice() {
        [a] => {
            let (v, axis) = one(a)?;
            Ok(if axis == Axis::Y {
                (centre, v)
            } else {
                (v, centre)
            })
        }
        [a, b] | [a, b, _] => {
            let ((va, aa), (vb, ab)) = (one(a)?, one(b)?);
            if aa == Axis::Y || ab == Axis::X {
                if aa == Axis::X || ab == Axis::Y {
                    return Err(format!("{value:?}: both values are on the same side"));
                }
                Ok((vb, va))
            } else {
                Ok((va, vb))
            }
        }
        _ => Err(format!(
            "{value:?}: transform-origin takes one to three values"
        )),
    }
}

/// `background-size`: one or two of `auto`, a length or a percentage.
/// One value sets the width and leaves the height `auto`. The keywords
/// `cover` and `contain`, and more than one layer, are not drawn.
fn background_size(value: &str, em: f64) -> Result<(Extent, Extent), String> {
    if value.contains(',') {
        return Err(format!(
            "{value:?}: one background layer is drawn, not a list"
        ));
    }
    let one = |t: &str| -> Result<Extent, String> {
        let l = t.to_ascii_lowercase();
        match l.as_str() {
            "auto" => Ok(Extent::Auto),
            "cover" | "contain" => Err(format!(
                "{value:?}: {t} is not drawn; give a length or a percentage"
            )),
            _ => match percent(t) {
                Some(p) => Ok(Extent::Percent(p)),
                None => pixels(t, em).map(Extent::Px),
            },
        }
    };
    match parts(value).as_slice() {
        [w] => Ok((one(w)?, Extent::Auto)),
        [w, h] => Ok((one(w)?, one(h)?)),
        _ => Err(format!("{value:?}: a size is one or two values")),
    }
}

/// `background-position`: one or two of a length, a percentage or a side
/// keyword. One value sets the horizontal and centres the vertical, as
/// CSS does. More than one layer is not drawn.
fn background_position(value: &str, em: f64) -> Result<(Extent, Extent), String> {
    if value.contains(',') {
        return Err(format!(
            "{value:?}: one background layer is drawn, not a list"
        ));
    }
    let one = |t: &str| -> Result<Extent, String> {
        match t.to_ascii_lowercase().as_str() {
            "left" | "top" => Ok(Extent::Percent(0.0)),
            "center" => Ok(Extent::Percent(50.0)),
            "right" | "bottom" => Ok(Extent::Percent(100.0)),
            _ => match percent(t) {
                Some(p) => Ok(Extent::Percent(p)),
                None => pixels(t, em).map(Extent::Px),
            },
        }
    };
    let p = parts(value);
    // A lone side keyword names its axis; `top` alone is the vertical.
    let vertical_first = p.len() == 1
        && matches!(p[0].to_ascii_lowercase().as_str(), "top" | "bottom")
        || p.len() == 2 && matches!(p[0].to_ascii_lowercase().as_str(), "top" | "bottom")
        || p.len() == 2 && matches!(p[1].to_ascii_lowercase().as_str(), "left" | "right");
    match p.as_slice() {
        [a] if vertical_first => Ok((Extent::Percent(50.0), one(a)?)),
        [a] => Ok((one(a)?, Extent::Percent(50.0))),
        [a, b] if vertical_first => Ok((one(b)?, one(a)?)),
        [a, b] => Ok((one(a)?, one(b)?)),
        _ => Err(format!("{value:?}: a position is one or two values")),
    }
}

/// The inside of `name(...)`, keeping the original case of the argument.
/// A `backdrop-filter` list: `none`, or `blur()`, `saturate()`,
/// `brightness()` and `contrast()` in any order and number.
pub fn filters(value: &str, em: f64) -> Result<Vec<Filter>, String> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    let amount = |arg: &str| -> Result<f64, String> {
        let arg = arg.trim();
        match percent(arg) {
            Some(p) => Ok(p / 100.0),
            None => number(arg),
        }
        .map(|a| a.max(0.0))
    };
    parts(v)
        .into_iter()
        .map(|f| {
            let l = f.to_ascii_lowercase();
            if let Some(arg) = function(&l, f, "blur") {
                Ok(Filter::Blur(pixels(arg.trim(), em)?.max(0.0)))
            } else if let Some(arg) = function(&l, f, "saturate") {
                Ok(Filter::Saturate(amount(arg)?))
            } else if let Some(arg) = function(&l, f, "brightness") {
                Ok(Filter::Brightness(amount(arg)?))
            } else if let Some(arg) = function(&l, f, "contrast") {
                Ok(Filter::Contrast(amount(arg)?))
            } else {
                unsupported(f, "blur(), saturate(), brightness() or contrast()")
            }
        })
        .collect()
}

fn function<'a>(lower: &str, value: &'a str, name: &str) -> Option<&'a str> {
    let head = format!("{name}(");
    if !lower.starts_with(&head) || !lower.ends_with(')') {
        return None;
    }
    Some(&value[head.len()..value.len() - 1])
}

/// Splits on commas that are not inside brackets, so `rgba(0, 0, 0, .5)`
/// stays in one piece.
fn top_level_commas(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(text[start..i].trim());
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(text[start..].trim());
    out
}

fn linear_gradient(args: &str) -> Result<Background, String> {
    let parts = top_level_commas(args);
    let (direction, rest) = match parts.first() {
        Some(first) if is_direction(first) => (parse_direction(first)?, &parts[1..]),
        // CSS defaults to a ramp running down the box.
        _ => (Direction::Angle(180.0), &parts[..]),
    };
    Ok(Background::Linear {
        direction,
        stops: stops(rest)?,
    })
}

fn radial_gradient(args: &str) -> Result<Background, String> {
    let parts = top_level_commas(args);
    let (mut circle, mut at) = (false, (0.5, 0.5));
    let rest = match parts.first() {
        Some(first) if is_radial_shape(first) => {
            let lower = first.to_ascii_lowercase();
            for word in [
                "closest-side",
                "closest-corner",
                "farthest-side",
                "farthest-corner",
            ] {
                if lower.contains(word) {
                    return Err(format!(
                        "{first:?}: a radial-gradient is sized to the farthest corner; {word} is not drawn"
                    ));
                }
            }
            circle = lower.starts_with("circle");
            if let Some(position) = lower.split(" at ").nth(1) {
                at = position_fraction(position)?;
            }
            &parts[1..]
        }
        _ => &parts[..],
    };
    Ok(Background::Radial {
        circle,
        at,
        stops: stops(rest)?,
    })
}

fn is_direction(first: &str) -> bool {
    let l = first.to_ascii_lowercase();
    l.starts_with("to ") || l.ends_with("deg") || l.ends_with("turn") || l.ends_with("rad")
}

fn is_radial_shape(first: &str) -> bool {
    let l = first.to_ascii_lowercase();
    l.starts_with("circle") || l.starts_with("ellipse") || l.starts_with("at ")
}

fn parse_direction(text: &str) -> Result<Direction, String> {
    let l = text.trim().to_ascii_lowercase();
    if let Some(sides) = l.strip_prefix("to ") {
        let (mut top, mut right, mut bottom, mut left) = (false, false, false, false);
        for word in sides.split_whitespace() {
            match word {
                "top" => top = true,
                "right" => right = true,
                "bottom" => bottom = true,
                "left" => left = true,
                _ => return Err(format!("{text:?}: expected to top, right, bottom or left")),
            }
        }
        return Ok(match (top, right, bottom, left) {
            (true, false, false, false) => Direction::Angle(0.0),
            (false, true, false, false) => Direction::Angle(90.0),
            (false, false, true, false) => Direction::Angle(180.0),
            (false, false, false, true) => Direction::Angle(270.0),
            (true, right @ (true | false), false, _) if right || left => {
                Direction::Corner { top: true, right }
            }
            (false, right @ (true | false), true, _) if right || left => {
                Direction::Corner { top: false, right }
            }
            _ => return Err(format!("{text:?}: expected a side or a corner")),
        });
    }
    let degrees = if let Some(n) = l.strip_suffix("deg") {
        n.trim().parse::<f64>().map_err(|_| angle_error(text))?
    } else if let Some(n) = l.strip_suffix("turn") {
        n.trim().parse::<f64>().map_err(|_| angle_error(text))? * 360.0
    } else if let Some(n) = l.strip_suffix("rad") {
        n.trim()
            .parse::<f64>()
            .map_err(|_| angle_error(text))?
            .to_degrees()
    } else {
        return Err(angle_error(text));
    };
    Ok(Direction::Angle(degrees))
}

fn angle_error(text: &str) -> String {
    format!("{text:?}: expected an angle such as 45deg, or a side such as to right")
}

/// A `50% 40%` position, as fractions of the box.
fn position_fraction(text: &str) -> Result<(f64, f64), String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let axis = |word: &str, vertical: bool| -> Result<f64, String> {
        Ok(match word {
            "left" | "top" => 0.0,
            "center" => 0.5,
            "right" | "bottom" => 1.0,
            other => match percent(other) {
                Some(p) => p / 100.0,
                None => {
                    let _ = vertical;
                    return Err(format!("{other:?}: expected a percentage or a keyword"));
                }
            },
        })
    };
    match words.len() {
        1 => {
            let a = axis(words[0], false)?;
            Ok((a, 0.5))
        }
        2 => Ok((axis(words[0], false)?, axis(words[1], true)?)),
        _ => Err(format!("{text:?}: expected a position such as 50% 40%")),
    }
}

/// Colour stops, with the positions CSS leaves out filled in: the first
/// at 0, the last at 1, and the rest spread evenly between the ones that
/// do say. A position that goes backwards is pulled up to the one before
/// it, as CSS does, so the ramp never runs in reverse.
fn stops(parts: &[&str]) -> Result<Vec<Stop>, String> {
    if parts.len() < 2 {
        return Err("a gradient needs at least two colour stops".to_owned());
    }
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        // A colour can carry spaces of its own, as `rgba(0, 238, 225, .5)`
        // does once its commas have been protected, so the positions are
        // taken off the end and whatever is left is the colour.
        let mut rest = part.trim();
        let mut positions = Vec::new();
        while let Some((head, tail)) = rest.rsplit_once(char::is_whitespace) {
            match percent(tail.trim()) {
                Some(v) if positions.len() < 2 => {
                    positions.push(v / 100.0);
                    rest = head.trim_end();
                }
                _ => break,
            }
        }
        positions.reverse();
        if rest.is_empty() {
            return Err(format!("{part:?}: expected a colour"));
        }
        let fill = color(rest)?;
        match positions.len() {
            // CSS lets one colour carry two positions, which is the same
            // as writing it twice: a band of flat colour between them.
            2 => {
                out.push(Stop {
                    color: fill,
                    at: Some(positions[0]),
                });
                out.push(Stop {
                    color: fill,
                    at: Some(positions[1]),
                });
            }
            1 => out.push(Stop {
                color: fill,
                at: Some(positions[0]),
            }),
            _ => out.push(Stop {
                color: fill,
                at: None,
            }),
        }
    }
    if out[0].at.is_none() {
        out[0].at = Some(0.0);
    }
    let last = out.len() - 1;
    if out[last].at.is_none() {
        out[last].at = Some(1.0);
    }
    let mut i = 0;
    while i < out.len() {
        if out[i].at.is_some() {
            i += 1;
            continue;
        }
        let before = i - 1;
        let mut after = i;
        while out[after].at.is_none() {
            after += 1;
        }
        let from = out[before].at.unwrap_or(0.0);
        let to = out[after].at.unwrap_or(1.0);
        let step = (to - from) / (after - before) as f64;
        for (n, slot) in out[i..after].iter_mut().enumerate() {
            slot.at = Some(from + step * (n + 1) as f64);
        }
        i = after;
    }
    let mut running = f64::NEG_INFINITY;
    for stop in &mut out {
        let at = stop.at.unwrap_or(0.0).max(running);
        stop.at = Some(at);
        running = at;
    }
    Ok(out)
}

fn shadow(value: &str, em: f64) -> Result<Vec<Shadow>, String> {
    if value.trim().eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    if value.to_ascii_lowercase().contains("inset") {
        return Err("is not drawn when it is inset".to_owned());
    }
    // A comma outside brackets separates shadows; inside, it is part
    // of a colour such as rgba(0, 0, 0, 0.5).
    top_level_commas(value)
        .iter()
        .map(|one| {
            let mut lengths = Vec::new();
            let mut fill = Color::from_rgba8(0, 0, 0, 128);
            for token in parts(one) {
                match pixels(token, em) {
                    Ok(v) if lengths.len() < 3 => lengths.push(v),
                    _ => fill = color(token)?,
                }
            }
            if lengths.len() < 2 {
                return Err(format!(
                    "{one:?} needs an x and y offset, and optionally a blur radius"
                ));
            }
            Ok(Shadow {
                x: lengths[0],
                y: lengths[1],
                blur: lengths.get(2).copied().unwrap_or(0.0),
                color: fill,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parse_stylesheet;
    use crate::dom::parse;

    fn styled(html: &str) -> (Document, Vec<Computed>, Vec<String>) {
        let doc = parse(html).unwrap();
        let sheet = parse_stylesheet(&doc.style).unwrap();
        let (styles, problems, _) = cascade(&doc, &sheet);
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
    fn a_text_stroke_inherits_and_paint_order_puts_it_under() {
        let (doc, styles, problems) = styled(
            "<style>.a { color: #fff; font-size: 40px; -webkit-text-stroke: 0.1em black } \
             .b { -webkit-text-stroke-color: currentcolor; color: #f00; paint-order: stroke fill } \
             .c { -webkit-text-stroke: thick; paint-order: markers } \
             .d { -webkit-text-stroke: #0f0 2px; paint-order: stroke }</style>\
             <div class=a><p>x</p><p class=b>x</p><p class=c>x</p><p class=d>x</p></div>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let a = doc.children(doc.root)[0];
        let ps = doc.children(a);
        // Inherited from .a, as `color` is; em is the element's size.
        assert_eq!(styles[a].text.stroke_width, 4.0);
        assert_eq!(styles[ps[0]].text.stroke_width, 4.0);
        assert_eq!(styles[ps[0]].text.stroke_color().to_hex(), "#000000");
        assert!(styles[ps[0]].text.stroke_over_fill);
        // currentcolor follows the element's own colour.
        assert_eq!(styles[ps[1]].text.stroke_color().to_hex(), "#ff0000");
        assert!(!styles[ps[1]].text.stroke_over_fill);
        // A shorthand with no colour resets it to currentcolor; markers
        // first leaves fill before stroke.
        assert_eq!(styles[ps[2]].text.stroke_width, 5.0);
        assert_eq!(styles[ps[2]].text.stroke_color().to_hex(), "#ffffff");
        assert!(styles[ps[2]].text.stroke_over_fill);
        // Colour first, width second; `stroke` alone puts it first.
        assert_eq!(styles[ps[3]].text.stroke_width, 2.0);
        assert_eq!(styles[ps[3]].text.stroke_color().to_hex(), "#00ff00");
        assert!(!styles[ps[3]].text.stroke_over_fill);

        let (_, _, problems) = styled("<style>p { paint-order: fill fill }</style><p>x</p>");
        assert_eq!(problems.len(), 1, "{problems:?}");
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
    fn font_size_and_line_height_take_percentages() {
        let (doc, styles, problems) = styled(
            "<style>.a { font-size: 20px } .b { font-size: 150%; line-height: 200% }</style>\
             <div class=a><p class=b>x</p></div>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let b = doc.children(doc.children(doc.root)[0])[0];
        assert_eq!(styles[b].text.size, 30.0);
        assert!((styles[b].text.line_height.unwrap() - 2.0).abs() < 1e-9);
    }

    /// Every mode the compositor implements is taken by its CSS name,
    /// and one it does not is refused by name rather than approximated
    /// with something that looks similar.
    #[test]
    fn mix_blend_mode_takes_the_modes_the_compositor_has() {
        for (keyword, expected) in [
            ("multiply", Blend::Multiply),
            ("screen", Blend::Screen),
            ("overlay", Blend::Overlay),
            ("darken", Blend::Darken),
            ("lighten", Blend::Lighten),
            ("difference", Blend::Difference),
            ("soft-light", Blend::SoftLight),
            ("normal", Blend::Normal),
        ] {
            let (doc, styles, problems) = styled(&format!(
                "<style>.a {{ mix-blend-mode: {keyword} }}</style><div class=a></div>"
            ));
            assert!(problems.is_empty(), "{keyword}: {problems:?}");
            let a = doc.children(doc.root)[0];
            assert_eq!(styles[a].paint.blend, expected, "{keyword}");
            assert_eq!(expected.keyword(), keyword);
        }
    }

    /// `transform-origin` in its one-, two- and three-value forms, with
    /// keywords in either order.
    #[test]
    fn transform_origin_takes_keywords_lengths_and_percentages() {
        use Extent::{Percent as P, Px};
        for (value, expected) in [
            ("50% 72%", (P(50.0), P(72.0))),
            ("0 100%", (Px(0.0), P(100.0))),
            ("left", (P(0.0), P(50.0))),
            ("bottom", (P(50.0), P(100.0))),
            ("bottom right", (P(100.0), P(100.0))),
            ("right top", (P(100.0), P(0.0))),
            ("10px 2em 5px", (Px(10.0), Px(32.0))),
        ] {
            let (doc, styles, problems) = styled(&format!(
                "<style>.a {{ font-size: 16px; transform-origin: {value} }}</style><div class=a></div>"
            ));
            assert!(problems.is_empty(), "{value}: {problems:?}");
            let a = doc.children(doc.root)[0];
            assert_eq!(styles[a].paint.transform_origin, expected, "{value}");
        }
        let (_, _, problems) =
            styled("<style>.a { transform-origin: left right }</style><div class=a></div>");
        assert!(!problems.is_empty(), "two horizontal keywords");
    }

    /// The modes geneva does not have. A browser would draw these; this
    /// says so rather than drawing something else and staying quiet.
    #[test]
    fn a_mode_geneva_does_not_have_is_refused_by_name() {
        for keyword in [
            "color-dodge",
            "color-burn",
            "hard-light",
            "exclusion",
            "luminosity",
        ] {
            let (doc, styles, problems) = styled(&format!(
                "<style>.a {{ mix-blend-mode: {keyword} }}</style><div class=a></div>"
            ));
            assert!(
                problems.iter().any(|p| p.contains(keyword)),
                "{keyword} was not named: {problems:?}"
            );
            let a = doc.children(doc.root)[0];
            assert_eq!(styles[a].paint.blend, Blend::Normal, "{keyword}");
        }
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
        assert!((s.text.line_height.unwrap() - 1.25).abs() < 1e-9);
        assert_eq!(s.text.family.as_deref(), Some("Liberation Sans"));
        let sh = s.paint.shadow[0];
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
        assert!(problems[0].contains("float"), "{problems:?}");
        assert!(problems[0].contains("<div>"), "{problems:?}");
    }

    /// Naming the property is what turns a refused declaration into one
    /// someone can find. It holds for a value the property cannot take,
    /// not only for a property that is not drawn at all: without it a
    /// reader gets the value back and no way to tell which of an
    /// element's declarations produced it.
    #[test]
    fn a_refused_value_names_the_property_it_came_from() {
        let (_, _, problems) =
            styled("<style>.a { border-radius: 50% }</style><div class=a></div>");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("border-radius"), "{problems:?}");
        assert!(problems[0].contains("50%"), "{problems:?}");
    }

    #[test]
    fn user_agent_styles_lose_to_the_author() {
        let (doc, styles, _) = styled("<style>h1 { font-weight: 400 }</style><h1>x</h1>");
        let h1 = doc.children(doc.root)[0];
        assert_eq!(styles[h1].text.weight, 400);
        assert_eq!(styles[h1].text.size, 32.0);
    }

    #[test]
    fn a_text_shadow_is_read_onto_the_text_not_the_box() {
        let (doc, styles, problems) =
            styled("<style>p { text-shadow: 2px 3px 9px #00EEE1 }</style><p>hi</p>");
        assert!(problems.is_empty(), "{problems:?}");
        let p = doc.children(doc.root)[0];
        let shadow = styles[p].text.shadow[0];
        assert_eq!((shadow.x, shadow.y, shadow.blur), (2.0, 3.0, 9.0));
        assert_eq!(shadow.color.to_hex(), "#00eee1");
        assert!(styles[p].paint.shadow.is_empty(), "the box keeps its own");
    }

    #[test]
    fn a_shadow_keeps_a_colour_with_commas_in_it() {
        let (doc, styles, problems) = styled(
            "<style>p { text-shadow: 0 0 8px rgba(0, 238, 225, 0.5); \
             box-shadow: 0 2px 4px rgb(0, 0, 0) }</style><p>hi</p>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let p = doc.children(doc.root)[0];
        let glow = styles[p].text.shadow[0];
        assert_eq!(glow.blur, 8.0);
        assert!((glow.color.a - 0.5).abs() < 0.01);
        assert_eq!(styles[p].paint.shadow.len(), 1);
    }

    #[test]
    fn a_shadow_list_is_read_in_order() {
        let (doc, styles, problems) = styled(
            "<style>p { text-shadow: 0 0 1px red, 0 0 2px rgba(0, 0, 255, 0.5); \
             box-shadow: 0 1px 2px #000, inset 0 0 1px #fff }</style><p>hi</p>",
        );
        let p = doc.children(doc.root)[0];
        assert_eq!(styles[p].text.shadow.len(), 2);
        assert_eq!(styles[p].text.shadow[0].blur, 1.0);
        assert_eq!(styles[p].text.shadow[1].blur, 2.0);
        assert_eq!(
            problems.len(),
            1,
            "the inset box-shadow is named: {problems:?}"
        );
        assert!(styles[p].paint.shadow.is_empty());
    }

    #[test]
    fn a_clipped_background_moves_onto_the_text() {
        let (doc, styles, problems) = styled(
            "<style>p { background: linear-gradient(90deg, #ff0000, #0000ff); \
             -webkit-background-clip: text; background-clip: text; \
             -webkit-text-fill-color: transparent }</style><p>hi</p>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let p = doc.children(doc.root)[0];
        assert!(styles[p].paint.background.is_none(), "the box draws none");
        let text = doc.children(p)[0];
        let fill = styles[text]
            .text
            .fill
            .as_ref()
            .expect("the text has the fill");
        assert!(matches!(fill.background, Background::Linear { .. }));
        assert_eq!(styles[text].text.color.a, 0.0);
    }

    #[test]
    fn a_background_size_and_position_are_read() {
        let (doc, styles, problems) = styled(
            "<style>p { background: #ff0000; background-size: 971px 100%; \
             background-position: -20px 0 } q { background-position: center }</style>\
             <p>hi</p><q>x</q>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let p = doc.children(doc.root)[0];
        assert_eq!(
            styles[p].paint.background_size,
            (Extent::Px(971.0), Extent::Percent(100.0))
        );
        assert_eq!(
            styles[p].paint.background_position,
            (Extent::Px(-20.0), Extent::Px(0.0))
        );
        let q = doc.children(doc.root)[1];
        assert_eq!(
            styles[q].paint.background_position,
            (Extent::Percent(50.0), Extent::Percent(50.0))
        );
        let (_, _, problems) = styled("<style>p { background-size: cover }</style><p>hi</p>");
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn a_linear_gradient_keeps_its_angle_and_stops() {
        let (doc, styles, problems) = styled(
            "<style>p { background: linear-gradient(45deg, #ff0000, #0000ff 80%) }</style><p>hi</p>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let p = doc.children(doc.root)[0];
        let Some(Background::Linear { direction, stops }) = &styles[p].paint.background else {
            panic!(
                "expected a linear gradient, got {:?}",
                styles[p].paint.background
            );
        };
        assert_eq!(*direction, Direction::Angle(45.0));
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].at, Some(0.0));
        assert_eq!(stops[1].at, Some(0.8));
    }

    #[test]
    fn a_gradient_with_no_direction_runs_down_the_box() {
        let (doc, styles, _) =
            styled("<style>p { background: linear-gradient(#000000, #ffffff) }</style><p>hi</p>");
        let p = doc.children(doc.root)[0];
        let Some(Background::Linear { direction, .. }) = &styles[p].paint.background else {
            panic!("expected a linear gradient");
        };
        assert_eq!(*direction, Direction::Angle(180.0));
    }

    #[test]
    fn a_corner_waits_for_the_box_to_know_its_angle() {
        let (doc, styles, _) = styled(
            "<style>p { background: linear-gradient(to bottom right, #000000, #ffffff) }</style><p>hi</p>",
        );
        let p = doc.children(doc.root)[0];
        let Some(Background::Linear { direction, .. }) = &styles[p].paint.background else {
            panic!("expected a linear gradient");
        };
        assert_eq!(
            *direction,
            Direction::Corner {
                top: false,
                right: true
            }
        );
    }

    #[test]
    fn stops_that_say_nothing_are_spread_evenly() {
        let (doc, styles, _) = styled(
            "<style>p { background: linear-gradient(#000000, #111111, #222222, #ffffff) }</style><p>hi</p>",
        );
        let p = doc.children(doc.root)[0];
        let Some(Background::Linear { stops, .. }) = &styles[p].paint.background else {
            panic!("expected a linear gradient");
        };
        let at: Vec<f64> = stops.iter().map(|s| s.at.unwrap_or(-1.0)).collect();
        assert_eq!(at, vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
    }

    #[test]
    fn a_stop_that_goes_backwards_is_pulled_up_to_the_one_before_it() {
        let (doc, styles, _) = styled(
            "<style>p { background: linear-gradient(#000000 60%, #ffffff 20%) }</style><p>hi</p>",
        );
        let p = doc.children(doc.root)[0];
        let Some(Background::Linear { stops, .. }) = &styles[p].paint.background else {
            panic!("expected a linear gradient");
        };
        assert_eq!(stops[0].at, Some(0.6));
        assert_eq!(stops[1].at, Some(0.6), "never runs in reverse");
    }

    #[test]
    fn a_radial_gradient_keeps_its_shape_and_centre() {
        let (doc, styles, _) = styled(
            "<style>p { background: radial-gradient(circle at 30% 70%, #ffffff, #00000000) }</style><p>hi</p>",
        );
        let p = doc.children(doc.root)[0];
        let Some(Background::Radial { circle, at, .. }) = &styles[p].paint.background else {
            panic!("expected a radial gradient");
        };
        assert!(*circle);
        assert_eq!(*at, (0.3, 0.7));
    }

    #[test]
    fn a_colour_is_still_a_colour() {
        let (doc, styles, _) = styled("<style>p { background: #123456 }</style><p>hi</p>");
        let p = doc.children(doc.root)[0];
        let Some(Background::Color(c)) = &styles[p].paint.background else {
            panic!("expected a flat colour");
        };
        assert_eq!(c.to_hex(), "#123456");
    }

    #[test]
    fn the_gradients_geneva_does_not_draw_are_named() {
        for value in [
            "conic-gradient(#000000, #ffffff)",
            "repeating-linear-gradient(45deg, #000000, #ffffff)",
            "radial-gradient(circle closest-side, #000000, #ffffff)",
        ] {
            let (_, _, problems) = styled(&format!(
                "<style>p {{ background: {value} }}</style><p>hi</p>"
            ));
            assert_eq!(problems.len(), 1, "{value} should be reported once");
        }
    }

    #[test]
    fn a_gradient_needs_two_stops() {
        let (_, _, problems) =
            styled("<style>p { background: linear-gradient(45deg, #ff0000) }</style><p>hi</p>");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("two colour stops"), "{problems:?}");
    }

    #[test]
    fn a_colour_with_commas_survives_the_split() {
        let (doc, styles, problems) = styled(
            "<style>p { background: linear-gradient(90deg, rgba(0, 238, 225, 0.5), #ffffff) }</style><p>hi</p>",
        );
        assert!(problems.is_empty(), "{problems:?}");
        let p = doc.children(doc.root)[0];
        let Some(Background::Linear { stops, .. }) = &styles[p].paint.background else {
            panic!("expected a linear gradient");
        };
        assert_eq!(stops.len(), 2);
        assert!((f64::from(stops[0].color.a) - 0.5).abs() < 0.01);
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
    fn elements_with_their_own_renderer_are_named() {
        for tag in ["iframe", "svg", "canvas", "video"] {
            let p = problems(&format!("<{tag}></{tag}>"));
            assert_eq!(p.len(), 1, "{tag}");
            assert!(p[0].contains(tag), "{p:?}");
        }
    }
}

/// The border box the style declares for an element, resolved against a
/// containing block. `None` on an axis the style leaves to the content,
/// where nothing short of laying it out can say how big it is.
pub fn declared_box(style: &Computed, block: (f32, f32)) -> (Option<f32>, Option<f32>) {
    use taffy::{MaybeResolve, ResolveOrZero};

    let zero = |l: LengthPercentage, r: f32| l.resolve_or_zero(Some(r), |_, _| 0.0);
    // CSS resolves a percentage padding or border against the containing
    // block's inline size on both axes.
    let inline = block.0;
    let extra =
        |lead: LengthPercentage, trail: LengthPercentage| zero(lead, inline) + zero(trail, inline);
    let pad_x = extra(style.layout.padding.left, style.layout.padding.right)
        + extra(style.layout.border.left, style.layout.border.right);
    let pad_y = extra(style.layout.padding.top, style.layout.padding.bottom)
        + extra(style.layout.border.top, style.layout.border.bottom);
    let border_box = style.layout.box_sizing == BoxSizing::BorderBox;
    let axis = |d: Dimension, reference: f32, extra: f32| {
        d.maybe_resolve(Some(reference), |_, _| 0.0)
            .map(|v| if border_box { v } else { v + extra })
    };
    (
        axis(style.layout.size.width, block.0, pad_x),
        axis(style.layout.size.height, block.1, pad_y),
    )
}
