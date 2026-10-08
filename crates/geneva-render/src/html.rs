//! Measuring and painting the display list `geneva-html` produces.
//!
//! Layout asks two questions this crate can answer, how big a run of text
//! is and how big an image is, and then hands back boxes with
//! absolute coordinates. Painting them needs nothing the compositor does
//! not already do: a rounded rectangle is a signed distance field, and
//! text goes through the same engine every other text source uses.

use std::collections::{BTreeMap, HashMap, HashSet};

use geneva_color::{Color, LinearRgba, Transfer};
use geneva_html::layout::Rectangle;
use geneva_html::style::{Extent, TextFill};
use geneva_html::{Content, Group, Laid, Measure, Painted, Prepared, Text};
use geneva_timeline::motion::{NodeMotion, Transform};
use geneva_timeline::schema::{
    BlendMode, FontSynthesis, Shadow, Shadows, Stroke, TextAlign, TextSource, TextStyle,
};
use geneva_timeline::{Animated, FillTrack, OutlinePaint, ResolvedHtml, ResolvedText};
use rayon::prelude::*;

use crate::assets::Image;
use crate::backdrop::{Backdrop, Filter};
use crate::fill::{Fill, encoded};
use crate::text::TextEngine;

mod layers;

pub use layers::{MarkupGroup, MarkupItem, MarkupLayers, MarkupRun, render_layers};

/// The engine and the images an HTML box needs, gathered before layout so
/// that neither borrow fights the other.
struct Context<'a> {
    text: &'a mut TextEngine,
    images: &'a HashMap<String, Image>,
    /// Sizes already measured, keyed by the run and the width it was
    /// measured at; flex asks the same question several times, and an
    /// animated source asks it again every frame.
    memo: &'a mut Measured,
}

/// Text sizes by the run, its style and the width it was measured at.
/// Colour is not in the key, so a word changing colour is not measured
/// again.
type Measured = HashMap<(String, String, u32), (f32, f32)>;

/// How many sizes a source keeps between frames. A size that animates
/// (a font size, letter spacing) adds keys every frame; past this many
/// they are dropped and measured again as asked.
const MEASURED_KEEP: usize = 4096;

/// Turns an HTML text style into the text source the engine draws, so
/// markup takes the same shaping, fallback and colour path as a text clip.
fn as_text_source(text: &str, style: &Text, max_width: f64) -> ResolvedText {
    let mut source = ResolvedText::constant(
        text.to_owned(),
        TextSource {
            text: Some(text.to_owned()),
            words: None,
            highlight: None,
            style: text_style_of(style),
            max_width: None,
            align: Some(match style.align {
                geneva_html::TextAlign::Left => TextAlign::Left,
                geneva_html::TextAlign::Center => TextAlign::Center,
                geneva_html::TextAlign::Right => TextAlign::Right,
            }),
            line_height: style.line_height,
            padding: None,
            background: None,
            radius: None,
            // A CSS stroke is centred on the outline; the engine's
            // outline is the part outside the glyph, half of it.
            outline: (style.stroke_width > 0.0).then(|| Stroke {
                color: style.stroke_color().into(),
                width: style.stroke_width / 2.0,
            }),
            shadow: (!style.shadow.is_empty()).then(|| {
                Shadows(
                    style
                        .shadow
                        .iter()
                        .map(|s| Shadow {
                            color: Some(Animated::Constant(s.color.into())),
                            x: Animated::Constant(s.x),
                            y: Animated::Constant(s.y),
                            blur: Animated::Constant(s.blur),
                        })
                        .collect(),
                )
            }),
        },
        max_width,
    );
    source.browser_lines = true;
    source.rtl = style.rtl();
    source.nowrap = !style.white_space.wraps();
    source.outline_paint = if style.stroke_over_fill {
        OutlinePaint::StrokeOver
    } else {
        OutlinePaint::StrokeUnder
    };
    source
}

/// An HTML text style as the style block a text source carries.
fn text_style_of(style: &Text) -> TextStyle {
    TextStyle {
        font: style.family.clone(),
        size: Some(style.size),
        weight: Some(style.weight),
        italic: Some(style.italic),
        color: Some(Animated::Constant(style.color.into())),
        fill: None,
        letter_spacing: Some(style.letter_spacing),
        synthesis: FontSynthesis {
            weight: style.synthesis.0,
            style: style.synthesis.1,
        },
    }
}

/// The pieces of a run of text as the engine takes them.
fn engine_runs(runs: &[(String, Text)]) -> Vec<(String, TextStyle, LinearRgba)> {
    runs.iter()
        .map(|(piece, style)| (piece.clone(), text_style_of(style), style.color.to_linear()))
        .collect()
}

/// A key that distinguishes two runs with different styles.
/// The width a text is measured at: a run with no limit, or one that
/// never breaks, at a width nothing will reach, which is max-content.
fn measure_limit(style: &Text, width: Option<f32>) -> f32 {
    width
        .filter(|w| *w > 0.0 && style.white_space.wraps())
        .unwrap_or(1.0e5)
}

/// A text style as it is measured. A shadow or stroke pads the rendered
/// image, so measuring with one would move the text it is drawn behind;
/// CSS lays text out as though they were not there, and so does this.
/// Alignment does not change where lines break, and it is left out: in
/// the measuring width a centred line sits tens of thousands of pixels
/// along, where an `f32` holds positions to about 1/256 px, and the
/// width read back came out short of the line (91.0 for 91.0008), so the
/// box was made too narrow and the paint broke the line.
fn plain_for_measure(style: &Text) -> Text {
    let mut plain = style.clone();
    plain.shadow.clear();
    plain.stroke_width = 0.0;
    plain.align = geneva_html::TextAlign::Left;
    plain
}

fn style_key(style: &Text) -> String {
    format!(
        "{}|{}|{}|{}|{:?}|{}|{:?}|{}",
        style.family.as_deref().unwrap_or(""),
        style.size,
        style.weight,
        style.italic,
        style.line_height,
        style.letter_spacing,
        style.align,
        style.white_space.wraps()
    )
}

impl Context<'_> {
    /// The min-content width of some pieces of text: the widest word,
    /// since a line breaks between words and not inside one.
    fn longest_word<'a>(&mut self, pieces: impl Iterator<Item = (&'a str, &'a Text)>) -> f32 {
        let mut widest = 0.0f32;
        for (piece, style) in pieces {
            for word in piece.split_whitespace() {
                widest = widest.max(self.text(word, style, None).0);
            }
        }
        widest
    }
}

impl Measure for Context<'_> {
    fn text(&mut self, text: &str, style: &Text, width: Option<f32>) -> (f32, f32) {
        if text.trim().is_empty() {
            return (0.0, 0.0);
        }
        // Min-content is asked for as a width of zero: the text at the
        // width of its longest word, or all of it when it never breaks.
        if width == Some(0.0) && style.white_space.wraps() {
            let widest = self.longest_word(std::iter::once((text, style)));
            return self.text(text, style, Some(widest.max(1.0)));
        }
        let limit = measure_limit(style, width);
        let key = (text.to_owned(), style_key(style), limit.to_bits());
        if let Some(size) = self.memo.get(&key) {
            return *size;
        }
        let plain = plain_for_measure(style);
        let source = as_text_source(text, &plain, f64::from(limit));
        let image = self.text.render(&source, 0.0);
        let size = (image.width as f32, image.height as f32);
        self.memo.insert(key, size);
        size
    }

    fn rich(&mut self, runs: &[(String, Text)], style: &Text, width: Option<f32>) -> (f32, f32) {
        if runs.iter().all(|(piece, _)| piece.trim().is_empty()) {
            return (0.0, 0.0);
        }
        if width == Some(0.0) && style.white_space.wraps() {
            let widest = self.longest_word(runs.iter().map(|(p, s)| (p.as_str(), s)));
            return self.rich(runs, style, Some(widest.max(1.0)));
        }
        let limit = measure_limit(style, width);
        let mut text = String::new();
        let mut styles = style_key(style);
        for (piece, s) in runs {
            text.push_str(piece);
            text.push('\u{1f}');
            styles.push('/');
            styles.push_str(&style_key(s));
        }
        let key = (text, styles, limit.to_bits());
        if let Some(size) = self.memo.get(&key) {
            return *size;
        }
        let plain = plain_for_measure(style);
        let source = as_text_source("", &plain, f64::from(limit));
        let image = self.text.render_runs(&source, &engine_runs(runs), 0.0);
        let size = (image.width as f32, image.height as f32);
        self.memo.insert(key, size);
        size
    }

    fn image(&mut self, src: &str) -> Option<(f32, f32)> {
        self.images
            .get(src)
            .map(|i| (i.width as f32, i.height as f32))
    }
}

/// Parses the markup and its styles. Problems the resolver already
/// reported are dropped here; this is the second read of the same text.
pub fn prepare(html: &ResolvedHtml) -> Result<Prepared, String> {
    geneva_html::prepare(&html.html, &html.css, &html.linked).map_err(|e| e.to_string())
}

/// Parses, lays out and paints an HTML source at `t` seconds into its
/// clip. Nothing here depends on time unless an element inside carries
/// an animation; then its style at `t` is laid over the document before
/// layout, and the element is painted as a group and composited with the
/// transform, opacity and blur the animation gives it.
pub fn render(
    html: &ResolvedHtml,
    prepared: &Prepared,
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
    t: f64,
    cache: &mut GroupCache,
) -> Result<(Image, Vec<Backdrop>), String> {
    render_into(html, prepared, text, images, t, cache, None)
}

/// [`render`], drawing into `spare` when it is a picture of the same size
/// handed back from an earlier frame: only what that frame marked (its
/// content rectangle) is cleared.
#[allow(clippy::too_many_arguments)]
pub fn render_into(
    html: &ResolvedHtml,
    prepared: &Prepared,
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
    t: f64,
    cache: &mut GroupCache,
    spare: Option<Image>,
) -> Result<(Image, Vec<Backdrop>), String> {
    let (laid, transforms) = lay_out(html, prepared, text, images, t, &mut cache.measured)?;
    let mut surface = paint(&laid, &transforms, text, images, cache, spare);
    to_linear(&mut surface);
    Ok((surface, backdrops(&laid, &transforms)))
}

/// The boxes with a `backdrop-filter`, where they land in the clip's box
/// at this moment: each group they are in moves their rectangle (to its
/// bounding box, when the group turns) and multiplies its opacity in.
fn backdrops(laid: &Laid, transforms: &[Option<Transform>]) -> Vec<Backdrop> {
    let mut out = Vec::new();
    for b in &laid.boxes {
        if b.paint.backdrop.is_empty() || b.opacity <= 0.0 {
            continue;
        }
        let r = b.rect;
        let mut corners = [
            (f64::from(r[0]), f64::from(r[1])),
            (f64::from(r[0] + r[2]), f64::from(r[1])),
            (f64::from(r[0]), f64::from(r[1] + r[3])),
            (f64::from(r[0] + r[2]), f64::from(r[1] + r[3])),
        ];
        let mut opacity = f64::from(b.opacity);
        let mut scale = 1.0f64;
        let mut group = b.group;
        while let Some(g) = group {
            let gr = &laid.groups[g];
            opacity *= f64::from(gr.opacity);
            if let Some(tr) = transforms.get(g).and_then(Option::as_ref) {
                let centre = gr.pivot;
                for c in &mut corners {
                    *c = forward(tr, centre, *c);
                }
                scale *= f64::midpoint(tr.scale[0].abs(), tr.scale[1].abs());
            }
            group = gr.parent;
        }
        if opacity <= 0.0 {
            continue;
        }
        let x0 = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let y0 = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        let x1 = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let y1 = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max);
        out.push(Backdrop {
            rect: [x0, y0, x1 - x0, y1 - y0],
            radius: b.paint.radius.map(|r| r * scale),
            filters: b
                .paint
                .backdrop
                .iter()
                .map(|f| match *f {
                    Filter::Blur(r) => Filter::Blur(r * scale),
                    other => other,
                })
                .collect(),
            opacity,
        });
    }
    out
}

/// Lays the document out at `t`, with the animated styles of the moment
/// over it, and samples each group's transform against the box it
/// settled on: what painting starts from, whoever composites.
fn lay_out(
    html: &ResolvedHtml,
    prepared: &Prepared,
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
    t: f64,
    measured: &mut Measured,
) -> Result<(Laid, Vec<Option<Transform>>), String> {
    if measured.len() > MEASURED_KEEP {
        measured.clear();
    }
    let mut context = Context {
        text,
        images,
        memo: measured,
    };
    let motions: HashMap<usize, &NodeMotion> = html.motion.iter().map(|m| (m.node, m)).collect();
    let mut overrides = BTreeMap::new();
    for m in &html.motion {
        let Some(style) = prepared.styles.get(m.node) else {
            continue;
        };
        let sampled = m.sample(t, style, (0.0, 0.0));
        if !sampled.overrides.is_empty() {
            overrides.insert(m.node, sampled.overrides);
        }
    }
    let mut laid = prepared.layout_with(
        html.width.map(|w| w as f32),
        html.height.map(|h| h as f32),
        &mut context,
        &overrides,
    )?;
    // A keyframe that names another box (`anchor()`) is worked out
    // against where the boxes landed, and the frame laid out again with
    // it. The box it moves is not one the others depend on, so one more
    // pass settles it.
    if html.motion.iter().any(NodeMotion::uses_anchors) {
        let anchors = prepared.anchors(&laid);
        for m in html.motion.iter().filter(|m| m.uses_anchors()) {
            let Some(style) = prepared.styles.get(m.node) else {
                continue;
            };
            let frame = geneva_timeline::motion::AnchorFrame {
                anchors: &anchors,
                origin: prepared.containing_origin(&laid, m.node),
            };
            let sampled = m.sample_anchored(t, style, (0.0, 0.0), Some(&frame));
            overrides.insert(m.node, sampled.overrides);
        }
        laid = prepared.layout_with(
            html.width.map(|w| w as f32),
            html.height.map(|h| h as f32),
            &mut context,
            &overrides,
        )?;
    }
    // A transform is sampled against the box the element settled on,
    // which a percentage in a translation is a share of.
    let transforms: Vec<Option<Transform>> = laid
        .groups
        .iter()
        .map(|g| {
            let size = (f64::from(g.rect[2]), f64::from(g.rect[3]));
            let style = &prepared.styles[g.node];
            let Some(m) = motions.get(&g.node) else {
                let raw = style.paint.transform.as_deref()?;
                return geneva_timeline::motion::static_transform(raw, size);
            };
            m.sample(t, style, size)
                .transform
                .filter(|tr| !tr.is_identity())
        })
        .collect();
    Ok((laid, transforms))
}

/// A pixel of the painter's working space, which is a browser's: sRGB-
/// encoded channels premultiplied by alpha. Everything inside a markup
/// box (gradients, translucent boxes, shadows, blur, text) blends there,
/// so a page looks as it does in a browser, and the finished box is
/// turned into linear light once, for the compositor.
pub(crate) fn encode_pixel(p: LinearRgba) -> LinearRgba {
    if p.a <= 0.0 {
        return LinearRgba::TRANSPARENT;
    }
    let enc = |v: f32| srgb_from_linear(v / p.a) * p.a;
    LinearRgba {
        r: enc(p.r),
        g: enc(p.g),
        b: enc(p.b),
        a: p.a,
    }
}

pub(crate) fn decode_pixel(p: LinearRgba) -> LinearRgba {
    if p.a <= 0.0 {
        return LinearRgba::TRANSPARENT;
    }
    let dec = |v: f32| srgb_to_linear(v / p.a) * p.a;
    LinearRgba {
        r: dec(p.r),
        g: dec(p.g),
        b: dec(p.b),
        a: p.a,
    }
}

/// A linear-light image as the painter works with it.
pub(crate) fn to_encoded(mut image: Image) -> Image {
    for p in &mut image.pixels {
        *p = encode_pixel(*p);
    }
    image
}

/// The sRGB curve on `[0, 1]` read off a table, for the whole surface
/// once a frame. Linear between entries this close, the table is within
/// 1e-7 of the curve; a value outside the range takes the curve itself.
fn srgb_to_linear(v: f32) -> f32 {
    const STEPS: usize = 4096;
    static TABLE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    if !(0.0..=1.0).contains(&v) {
        return Transfer::Srgb.to_linear(f64::from(v)) as f32;
    }
    let table = TABLE.get_or_init(|| {
        (0..=STEPS)
            .map(|i| Transfer::Srgb.to_linear(i as f64 / STEPS as f64) as f32)
            .collect()
    });
    let at = v * STEPS as f32;
    let i = (at as usize).min(STEPS - 1);
    let f = at - i as f32;
    table[i] + (table[i + 1] - table[i]) * f
}

/// The sRGB curve's inverse on `[0, 1]`, read off a table, for every
/// pixel of text each time it is drawn. The curve is steepest just
/// above its linear toe, and there the table is within 2e-5 of it, a
/// fiftieth of a step of 10-bit video; a value outside the range takes
/// the curve itself.
fn srgb_from_linear(v: f32) -> f32 {
    const STEPS: usize = 4096;
    static TABLE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    if !(0.0..=1.0).contains(&v) {
        return Transfer::Srgb.from_linear(f64::from(v)) as f32;
    }
    let table = TABLE.get_or_init(|| {
        (0..=STEPS)
            .map(|i| Transfer::Srgb.from_linear(i as f64 / STEPS as f64) as f32)
            .collect()
    });
    let at = v * STEPS as f32;
    let i = (at as usize).min(STEPS - 1);
    let f = at - i as f32;
    table[i] + (table[i + 1] - table[i]) * f
}

/// The painter's image back in linear light: its content rectangle,
/// since everything outside it is transparent, which converts to itself.
/// A caption's box is the size of the frame, its words a strip of it.
fn to_linear(image: &mut Image) {
    let width = image.width as usize;
    let [cx, cy, cw, ch] = image.content.unwrap_or([0, 0, image.width, image.height]);
    let (x0, x1) = (cx as usize, ((cx + cw) as usize).min(width));
    let rows = cy as usize..((cy + ch) as usize).min(image.height as usize);
    if x0 >= x1 || rows.is_empty() || width == 0 {
        return;
    }
    image.pixels[rows.start * width..rows.end * width]
        .par_chunks_mut(width)
        .for_each(|row| {
            for p in &mut row[x0..x1] {
                *p = decode_pixel(*p);
            }
        });
}

/// The rectangle one box can touch: its own, grown by what its shadows
/// spill. Everything else a box draws is inside it.
fn reach_of(painted: &Painted) -> Bounds {
    let reach = |s: &geneva_html::style::Shadow| s.blur.abs() + s.x.abs().max(s.y.abs()) + 1.0;
    let furthest = |list: &[geneva_html::style::Shadow]| list.iter().map(reach).fold(0.0, f64::max);
    let box_shadow = furthest(&painted.paint.shadow);
    // A stroke reaches half its width out, and a miter at a sharp corner
    // up to four times that; its shadow reaches as far again.
    let text_shadow = match &painted.content {
        Content::Text { style, .. } | Content::Rich { style, .. } => {
            let stroke = style.stroke_width / 2.0 * OutlinePaint::StrokeUnder.reach();
            stroke + furthest(&style.shadow) + f64::from(u8::from(stroke > 0.0))
        }
        _ => 0.0,
    };
    let grow = box_shadow.max(text_shadow);
    [
        f64::from(painted.rect[0]) - grow,
        f64::from(painted.rect[1]) - grow,
        f64::from(painted.rect[0] + painted.rect[2]) + grow,
        f64::from(painted.rect[1] + painted.rect[3]) + grow,
    ]
}

/// A rectangle as its left, top, right and bottom edges.
type Bounds = [f64; 4];

/// One rectangle per group, where a group has one.
type GroupBounds = Vec<Option<Bounds>>;

fn union(a: Option<Bounds>, r: Bounds) -> Bounds {
    match a {
        None => r,
        Some(o) => [
            o[0].min(r[0]),
            o[1].min(r[1]),
            o[2].max(r[2]),
            o[3].max(r[3]),
        ],
    }
}

fn padded(r: Bounds, by: f64) -> Bounds {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

/// A group's rectangle after its transform, about its pivot
/// (`transform-origin`).
fn transformed(r: Bounds, centre: (f64, f64), tr: &Transform) -> Bounds {
    let mut out: Option<Bounds> = None;
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (px, py) = forward(tr, centre, (x, y));
        out = Some(union(out, [px, py, px, py]));
    }
    out.unwrap_or(r)
}

/// Where a point of the group lands: scaled and turned about the pivot,
/// then moved. `rotate` is clockwise on a screen whose y grows down.
fn forward(tr: &Transform, centre: (f64, f64), p: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (
        (p.0 - centre.0) * tr.scale[0],
        (p.1 - centre.1) * tr.scale[1],
    );
    let (sin, cos) = tr.rotate.to_radians().sin_cos();
    (
        centre.0 + dx * cos - dy * sin + tr.translate[0],
        centre.1 + dx * sin + dy * cos + tr.translate[1],
    )
}

/// The point of the group that lands at `p`: [`forward`] undone.
fn inverse(tr: &Transform, centre: (f64, f64), p: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (
        p.0 - centre.0 - tr.translate[0],
        p.1 - centre.1 - tr.translate[1],
    );
    let (sin, cos) = tr.rotate.to_radians().sin_cos();
    let (rx, ry) = (dx * cos + dy * sin, -dx * sin + dy * cos);
    (centre.0 + rx / tr.scale[0], centre.1 + ry / tr.scale[1])
}

/// The buffer each group needs (`own`, in its own coordinates: what its
/// boxes and children reach, padded for its blur, and no more than can
/// show through its parent's buffer and its transform) and where its
/// picture lands once blurred and transformed (`placed`). Groups are
/// listed parent before child, so a backward walk settles every child's
/// reach before its parent's, and a forward walk then cuts each buffer
/// down to the part its parent can show.
fn group_bounds(
    laid: &Laid,
    transforms: &[Option<Transform>],
    surface: (f64, f64),
) -> (GroupBounds, GroupBounds, GroupBounds) {
    let n = laid.groups.len();
    let mut own: GroupBounds = vec![None; n];
    let mut placed: GroupBounds = vec![None; n];
    // What each group covers before its parent's window cuts it down:
    // the most a cached picture of it can usefully hold.
    let mut natural: GroupBounds = vec![None; n];
    for b in &laid.boxes {
        if b.opacity <= 0.0 {
            continue;
        }
        if let Some(g) = b.group {
            own[g] = Some(union(own[g], reach_of(b)));
        }
    }
    for g in (0..n).rev() {
        let group = &laid.groups[g];
        let Some(r) = own[g] else {
            continue;
        };
        if group.opacity <= 0.0 {
            continue;
        }
        let mut r = padded(r, group.blur * 3.0);
        // A `clip-path` cuts the blurred picture before the transform, so
        // nothing lands outside its polygon, and a polygon that covers
        // nothing (the start of a wipe) hides the group and all in it.
        if let Some(points) = &group.clip_path {
            match polygon_reach(points).and_then(|p| intersect(r, p)) {
                Some(cut) => r = cut,
                None => {
                    own[g] = None;
                    continue;
                }
            }
        }
        if let Some(tr) = &transforms[g] {
            r = transformed(r, group.pivot, tr);
        }
        placed[g] = Some(r);
        if let Some(p) = group.parent {
            own[p] = Some(union(own[p], r));
        }
    }
    // What a group paints outside the window its parent shows is never
    // seen, so its buffer stops there: the window (the surface, or the
    // parent's buffer, cut to the group's clip) taken back through the
    // transform, and padded for the blur, which reads that far outside
    // the pixels it lands on.
    for g in 0..n {
        let group = &laid.groups[g];
        let Some(r) = own[g] else {
            continue;
        };
        let mut window = match group.parent {
            None => Some([0.0, 0.0, surface.0, surface.1]),
            Some(p) => own[p],
        };
        if let (Some(w), Some((clip, _))) = (window, group.clip) {
            window = intersect(w, rect_bounds(clip));
        }
        let reach = group.blur * 3.0;
        let visible = window.and_then(|w| {
            let seen = match &transforms[g] {
                Some(tr) if tr.scale[0] == 0.0 || tr.scale[1] == 0.0 => return None,
                Some(tr) => inverted(w, group.pivot, tr),
                None => w,
            };
            // Only what is inside the polygon shows, and the blur reads
            // no further than its reach outside that.
            let seen = match &group.clip_path {
                Some(points) => intersect(seen, polygon_reach(points)?)?,
                None => seen,
            };
            Some(padded(seen, reach))
        });
        natural[g] = Some(padded(r, reach));
        own[g] = visible.and_then(|v| intersect(padded(r, reach), v));
    }
    (own, placed, natural)
}

/// The rectangle a `clip-path` polygon, in the surface's pixels, can
/// cover, or `None` for one that covers nothing: fewer than three points,
/// or all of them on one line, as a wipe's polygon is before it opens.
fn polygon_reach(points: &[(f64, f64)]) -> Option<Bounds> {
    let &first = points.first()?;
    let other = points.iter().find(|p| **p != first)?;
    let (dx, dy) = (other.0 - first.0, other.1 - first.1);
    let flat = points
        .iter()
        .all(|p| (dx * (p.1 - first.1) - dy * (p.0 - first.0)).abs() <= 1e-9);
    if points.len() < 3 || flat {
        return None;
    }
    let mut out: Option<Bounds> = None;
    for &(x, y) in points {
        out = Some(union(out, [x, y, x, y]));
    }
    out
}

/// Whether a `clip-path` polygon covers every pixel of `r` (left, top,
/// right, bottom, in whole pixels) whole, so that masking by it changes
/// nothing there. Answered only for a convex polygon, which is what a
/// wipe or an iris is once it is open: the rectangle is inside when its
/// four corners are. Anything else answers no, which is always safe.
fn polygon_covers(points: &[(f64, f64)], r: [i64; 4]) -> bool {
    // Repeated points turn nothing; leave them out.
    let mut ring: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &p in points {
        if ring.last() != Some(&p) {
            ring.push(p);
        }
    }
    while ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    let n = ring.len();
    if n < 3 {
        return false;
    }
    // Convex: every corner turns the same way, and the turns add up to
    // one full turn, which rules out a star whose corners all turn alike.
    let mut sign = 0.0f64;
    let mut turned = 0.0f64;
    for i in 0..n {
        let (a, b, c) = (ring[i], ring[(i + 1) % n], ring[(i + 2) % n]);
        let (ux, uy) = (b.0 - a.0, b.1 - a.1);
        let (vx, vy) = (c.0 - b.0, c.1 - b.1);
        let cross = ux * vy - uy * vx;
        if cross.abs() > 1e-9 {
            if sign != 0.0 && cross.signum() != sign {
                return false;
            }
            sign = cross.signum();
        }
        turned += cross.atan2(ux * vx + uy * vy);
    }
    if sign == 0.0 || (turned.abs() - std::f64::consts::TAU).abs() > 1e-6 {
        return false;
    }
    let corners = [
        (r[0] as f64, r[1] as f64),
        (r[2] as f64, r[1] as f64),
        (r[2] as f64, r[3] as f64),
        (r[0] as f64, r[3] as f64),
    ];
    corners.iter().all(|&(x, y)| {
        (0..n).all(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            ((b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)) * sign >= -1e-9
        })
    })
}

fn rect_bounds(rect: Rectangle) -> Bounds {
    [
        f64::from(rect[0]),
        f64::from(rect[1]),
        f64::from(rect[0] + rect[2]),
        f64::from(rect[1] + rect[3]),
    ]
}

/// The overlap of two rectangles, or nothing where they do not meet.
fn intersect(a: Bounds, b: Bounds) -> Option<Bounds> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// The rectangle of the group that lands on `r`: [`transformed`] undone,
/// as a bounding box.
fn inverted(r: Bounds, centre: (f64, f64), tr: &Transform) -> Bounds {
    let mut out: Option<Bounds> = None;
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (px, py) = inverse(tr, centre, (x, y));
        out = Some(union(out, [px, py, px, py]));
    }
    out.unwrap_or(r)
}

/// The integer rectangle a group's buffer covers, and whether bounding it
/// cut anything off.
///
/// A group can reach past the surface and be moved back onto it, so the
/// buffer is not clamped to the surface; it is bounded so that a runaway
/// scale cannot ask for the world. The bound is an area rather than a
/// rectangle: the old one was three surfaces wide by three tall, so its
/// area is the budget here, and a group costs no more than it used to.
/// Dropping the shape is what matters. A news crawl is a few hundred
/// pixels tall and many thousands wide, well inside the area and far
/// outside a rectangle two surfaces across, and it used to paint nothing
/// at all once it passed that edge.
pub(crate) fn bound_group(b: Bounds, surface: (u32, u32)) -> ([i64; 4], bool) {
    let (w, h) = (f64::from(surface.0), f64::from(surface.1));
    let natural = [b[0].floor(), b[1].floor(), b[2].ceil(), b[3].ceil()];
    let area = (natural[2] - natural[0]).max(0.0) * (natural[3] - natural[1]).max(0.0);
    if area <= 9.0 * w * h {
        return (
            [
                natural[0] as i64,
                natural[1] as i64,
                natural[2] as i64,
                natural[3] as i64,
            ],
            false,
        );
    }
    // Over the budget: keep the rectangle around the surface that the
    // bound has always kept, and say that the rest was dropped.
    let kept = [
        natural[0].clamp(-w, 2.0 * w) as i64,
        natural[1].clamp(-h, 2.0 * h) as i64,
        natural[2].clamp(-w, 2.0 * w) as i64,
        natural[3].clamp(-h, 2.0 * h) as i64,
    ];
    (kept, true)
}

/// A group's buffer while its boxes are painted. `origin` is where its
/// top-left pixel sits on the surface.
struct Layer {
    group: usize,
    image: Image,
    origin: (i64, i64),
    /// Whether the group's blur is already in the pixels, as it is in a
    /// picture kept blurred between frames.
    blurred: bool,
    /// A group painted straight into the buffer under it, which has no
    /// pixels of its own (see [`passes_through`]).
    through: bool,
    /// Whether anything has been painted or composited into it yet.
    touched: bool,
}

impl Layer {
    /// A buffer covering `bounds`, or nothing for a group whose picture
    /// would not show.
    fn over(group: usize, bounds: Option<Bounds>, surface: (u32, u32)) -> Self {
        let Some(b) = bounds else {
            return Self {
                group,
                image: Image {
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                    content: None,
                },
                origin: (0, 0),
                blurred: false,
                through: false,
                touched: false,
            };
        };
        let ([x0, y0, x1, y1], _) = bound_group(b, surface);
        let width = (x1 - x0).max(0) as u32;
        let height = (y1 - y0).max(0) as u32;
        Self {
            group,
            image: Image {
                width,
                height,
                pixels: transparent(width as usize * height as usize),
                content: None,
            },
            origin: (x0, y0),
            blurred: false,
            through: false,
            touched: false,
        }
    }

    /// A group painted straight into the buffer under it.
    fn through(group: usize) -> Self {
        let mut layer = Self::over(group, None, (0, 0));
        layer.through = true;
        layer
    }

    /// Its edges on the surface: left, top, right, bottom.
    fn bounds(&self) -> [i64; 4] {
        [
            self.origin.0,
            self.origin.1,
            self.origin.0 + i64::from(self.image.width),
            self.origin.1 + i64::from(self.image.height),
        ]
    }
}

/// A group's picture, painted once and kept while nothing in it
/// changes. `origin` is where its top-left pixel sits on the surface,
/// as a [`Layer`]'s does.
struct Cached {
    /// The boxes that produced the picture. They are compared, not
    /// hashed: equality is exact, costs nothing when the first field
    /// differs, and there is no bit pattern of an `f32` to get right.
    boxes: Vec<Painted>,
    image: Image,
    origin: (i64, i64),
    /// Which painting this is, counted over the cache: a renderer that
    /// keeps a copy of the picture (a texture) tells one painting from
    /// the next by it. Blurring the picture in place counts as a new
    /// painting.
    generation: u64,
    /// The blur already in the picture, zero for none.
    blurred: f64,
    /// The blur the group asked for the last time it was shown. A blur
    /// that asks the same twice in a row is holding still, and the
    /// picture is blurred in place and kept that way: blurring is the
    /// dear part of a big soft element, and its picture never changes.
    asked: f64,
    /// The last frame this picture was wanted, for choosing which to
    /// let go when a new one does not fit.
    used: u64,
    /// The picture blurred by a radius the group asked for lately (see
    /// [`blur_step`]), for a blur that changes from frame to frame:
    /// frames whose radii round the same share it.
    soft: Option<(f64, Image)>,
}

impl Cached {
    /// What the pictures hold, in bytes.
    fn bytes(&self) -> usize {
        let soft = self.soft.as_ref().map_or(0, |(_, i)| i.pixels.len());
        (self.image.pixels.len() + soft) * size_of::<LinearRgba>()
    }

    /// Whether the picture covers `want`, in surface pixels.
    fn covers(&self, want: [i64; 4]) -> bool {
        let (w, h) = (i64::from(self.image.width), i64::from(self.image.height));
        want[0] >= self.origin.0
            && want[1] >= self.origin.1
            && want[2] <= self.origin.0 + w
            && want[3] <= self.origin.1 + h
    }

    /// The part of the picture covering `want`, as a layer to composite.
    fn window(&self, group: usize, want: [i64; 4]) -> Layer {
        let mut layer = self.window_of(&self.image, group, want);
        layer.blurred = self.blurred > 0.0;
        layer
    }

    /// The part of `image`, the picture or its blurred copy, covering
    /// `want`.
    fn window_of(&self, image: &Image, group: usize, want: [i64; 4]) -> Layer {
        let width = (want[2] - want[0]).max(0) as u32;
        let height = (want[3] - want[1]).max(0) as u32;
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        let stride = image.width as usize;
        for y in 0..i64::from(height) {
            let row = (want[1] + y - self.origin.1) as usize * stride;
            let from = row + (want[0] - self.origin.0) as usize;
            pixels.extend_from_slice(&image.pixels[from..from + width as usize]);
        }
        Layer {
            group,
            image: Image {
                width,
                height,
                pixels,
                content: None,
            },
            origin: (want[0], want[1]),
            blurred: false,
            through: false,
            touched: true,
        }
    }
}

/// A group that keeps missing the cache, and the boxes it had the last
/// time it did.
#[derive(Default)]
struct Strikes {
    count: u8,
    last: Vec<Painted>,
}

/// Pictures of groups that do not change from frame to frame, held
/// between calls to [`render`] for one markup source.
///
/// A group's boxes are painted in the surface's coordinates and only
/// then moved by its transform, so an element that drifts and scales
/// paints the same pixels every frame; what changes is the window its
/// parent can show, which slides and shrinks across them. The picture
/// is therefore painted over the whole of what the group covers rather
/// than over the window one frame needs, and each frame copies its
/// window out. Painting the lot costs about three windows and then
/// holds for the length of the animation, where a picture cut to one
/// frame's window would be wrong by the next.
#[derive(Default)]
pub struct GroupCache {
    /// Whether a group's picture was cut down to the bound at any frame
    /// painted through this cache. Read once at the end of a clip: a
    /// crawl that overruns does it on most of its frames, and the reader
    /// wants to be told once.
    clipped: bool,
    entries: HashMap<usize, Cached>,
    /// Misses in a row, per group, and past [`STRIKES`] the group is
    /// not painted into the cache again until its boxes hold still.
    give_up: HashMap<usize, Strikes>,
    bytes: usize,
    /// Frames painted through this cache, for [`Cached::used`].
    frame: u64,
    /// Paintings so far, so that each picture kept has a number of its
    /// own.
    generation: u64,
    /// Text sizes measured on earlier frames: layout runs every frame
    /// for a source with an animation inside, and the words are the same.
    measured: Measured,
}

/// Pictures below this many pixels are not worth keeping, unless they
/// hold text: the copy out of the cache would cost about what painting
/// their boxes does.
const MIN_PIXELS: i64 = 64 * 64;

/// What one markup source may hold. Each thread rendering frames has a
/// cache of its own, so this is what one costs.
const BUDGET: usize = 96 << 20;

/// How many times in a row a group may be painted without its picture
/// being used again before it is given up on. An element whose own boxes
/// animate, a letter changing colour, never reuses one, and painting it
/// a size larger than the frame needs is worse than not trying. Once its
/// boxes are the same two frames running it is tried again: the
/// animation is over, and the element holds still for the rest.
const STRIKES: u8 = 3;

impl GroupCache {
    /// Forgets everything, freeing the pictures.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.give_up.clear();
        self.measured.clear();
        self.bytes = 0;
    }

    /// Records that a group was cut down to the bound.
    pub(crate) fn note_clipped(&mut self) {
        self.clipped = true;
    }

    /// Whether any group painted through this cache was cut down to the
    /// bound, clearing the flag so the next clip starts clean. What was
    /// cut is not drawn, and the reader is told rather than left to find
    /// a blank strip in the middle of a render.
    pub fn take_clipped(&mut self) -> bool {
        std::mem::take(&mut self.clipped)
    }

    fn insert(&mut self, group: usize, mut entry: Cached) {
        if let Some(old) = self.entries.remove(&group) {
            self.bytes -= old.bytes();
        }
        self.bytes += entry.bytes();
        entry.generation = self.generation;
        entry.used = self.frame;
        self.generation += 1;
        self.entries.insert(group, entry);
    }

    /// Lets go of pictures not wanted this frame, the longest unused
    /// first, until `bytes` more fit in the budget. Whether they now do.
    fn make_room(&mut self, bytes: usize) -> bool {
        while self.bytes + bytes > crate::limits::cache_budget(BUDGET, 0.125) {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(_, c)| c.used < self.frame)
                .min_by_key(|(_, c)| c.used)
                .map(|(g, _)| *g)
            else {
                return false;
            };
            let gone = self.entries.remove(&oldest).expect("found above");
            self.bytes -= gone.bytes();
        }
        true
    }
}

impl GroupCache {
    /// The part of group `g`'s picture covering `want`, blurred by
    /// `sigma` (already through [`blur_step`]) when the group asks for a
    /// blur that is not in the picture: from the blurred copy kept for
    /// that radius, made now if the radius is new. A picture much larger
    /// than the window (an element that travels) is not blurred whole;
    /// its window is, when it is composited.
    fn window(&mut self, g: usize, want: [i64; 4], sigma: f64) -> Option<Layer> {
        let entry = self.entries.get(&g)?;
        if sigma <= 0.0 || entry.blurred > 0.0 {
            return Some(entry.window(g, want));
        }
        let window = (want[2] - want[0]) * (want[3] - want[1]);
        let whole = i64::from(entry.image.width) * i64::from(entry.image.height);
        if whole > 2 * window {
            return Some(entry.window(g, want));
        }
        if entry.soft.as_ref().is_none_or(|(at, _)| *at != sigma) {
            let bytes = entry.image.pixels.len() * size_of::<LinearRgba>();
            let entry = self.entries.get_mut(&g).expect("found above");
            if let Some((_, old)) = entry.soft.take() {
                self.bytes -= old.pixels.len() * size_of::<LinearRgba>();
            }
            if !self.make_room(bytes) {
                return Some(self.entries[&g].window(g, want));
            }
            let entry = self.entries.get_mut(&g).expect("kept: wanted this frame");
            let mut soft = entry.image.clone();
            let (w, h) = (soft.width as usize, soft.height as usize);
            crate::blur::blur_pixels_reduced(&mut soft.pixels, w, h, sigma);
            entry.soft = Some((sigma, soft));
            self.bytes += bytes;
        }
        let entry = &self.entries[&g];
        let (_, soft) = entry.soft.as_ref().expect("made above");
        let mut layer = entry.window_of(soft, g, want);
        layer.blurred = true;
        Some(layer)
    }
}

/// Blur radii are rounded to this many pixels: a quarter of a pixel of
/// blur is not visible, and frames whose radii round the same share a
/// blurred picture.
const BLUR_STEP: f64 = 0.25;

/// A `filter: blur()` radius as the painter blurs by: rounded to
/// [`BLUR_STEP`], so that under an eighth of a pixel is no blur at all.
fn blur_step(sigma: f64) -> f64 {
    (sigma / BLUR_STEP).round() * BLUR_STEP
}

/// The integer buffer a group needs, as [`Layer::over`] would bound it.
fn buffer_rect(bounds: Option<Bounds>, surface: (u32, u32)) -> Option<[i64; 4]> {
    let (r, _) = bound_group(bounds?, surface);
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

/// A group whose picture can be kept: one with no group inside it, so
/// that its picture is its own boxes and nothing else. A group's own
/// transform is applied when it is composited rather than when it is
/// painted, so it does not stop its picture being kept, which is the
/// point: a drifting element paints the same pixels every frame.
///
/// A group that holds other groups is not kept. Its picture is those
/// groups composited into it, each with its own opacity, blur and
/// transform, so reusing it would mean reusing theirs; and the boxes
/// that would have to be compared for it are the whole subtree's.
fn is_leaf(laid: &Laid, g: usize) -> bool {
    !laid.groups.iter().any(|other| other.parent == Some(g))
}

/// Paints, or repaints, the groups whose pictures are worth keeping, and
/// returns which ones the caller may take from the cache. Boxes before
/// `first` are under something that hides them (see [`first_seen`]) and
/// are left out.
#[allow(clippy::too_many_arguments)]
fn fill_cache(
    cache: &mut GroupCache,
    laid: &Laid,
    first: usize,
    own: &[Option<Bounds>],
    natural: &[Option<Bounds>],
    surface: (u32, u32),
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
) -> HashSet<usize> {
    cache.frame += 1;
    let mut candidates = Vec::new();
    for (g, bounds) in own.iter().enumerate() {
        let Some(want) = buffer_rect(*bounds, surface) else {
            continue;
        };
        if !is_leaf(laid, g) {
            continue;
        }
        let boxes: Vec<Painted> = laid.boxes[first.min(laid.boxes.len())..]
            .iter()
            .filter(|b| b.group == Some(g) && b.opacity > 0.0)
            .cloned()
            .collect();
        if boxes.is_empty() {
            continue;
        }
        // Text is shaped and drawn by the engine, which costs far more
        // than copying the picture, however small: a word that has
        // finished arriving is kept, not drawn again every frame.
        let text = boxes
            .iter()
            .any(|b| matches!(b.content, Content::Text { .. } | Content::Rich { .. }));
        if !text && (want[2] - want[0]) * (want[3] - want[1]) < MIN_PIXELS {
            continue;
        }
        // Wanted this frame, fresh or not, so that making room for
        // another picture never lets it go.
        if let Some(c) = cache.entries.get_mut(&g) {
            c.used = cache.frame;
        }
        candidates.push((g, want, boxes));
    }
    let mut served = HashSet::new();
    for (g, want, boxes) in candidates {
        let blur = blur_step(laid.groups[g].blur);
        // Only the boxes are compared. A group's own opacity, blur and
        // clips are read when its picture is composited rather than when
        // it is painted, so they change nothing in the picture, and its
        // transform is the whole point: the picture stays put and the
        // transform moves it. The one exception is a blur already
        // blurred into the picture, which has to be the blur asked for.
        let fresh = cache.entries.get(&g).is_some_and(|c| {
            c.covers(want) && c.boxes == boxes && (c.blurred == 0.0 || c.blurred == blur)
        });
        if fresh {
            cache.give_up.remove(&g);
        } else {
            let strikes = cache.give_up.entry(g).or_default();
            if strikes.last == boxes {
                strikes.count = 0;
            }
            if strikes.count >= STRIKES {
                strikes.last = boxes;
                continue;
            }
            strikes.count += 1;
            strikes.last.clone_from(&boxes);
            // The whole of what the group covers, not just the window
            // this frame needs. The window slides and shrinks as the
            // element drifts and scales, so one that fits the frame
            // would be wrong by the next; painting the lot once costs
            // about three of those and then never again.
            let Some(region) = natural[g] else {
                continue;
            };
            let ([x0, y0, x1, y1], _) = bound_group(region, surface);
            let pixels = ((x1 - x0).max(0) * (y1 - y0).max(0)) as usize;
            if let Some(old) = cache.entries.remove(&g) {
                cache.bytes -= old.bytes();
            }
            if !cache.make_room(pixels * size_of::<LinearRgba>()) {
                continue;
            }
            let mut layer = Layer::over(g, Some(region), surface);
            for b in &boxes {
                let moved = shifted(b, layer.origin);
                paint_one(&mut layer.image, &moved, text, images);
            }
            let entry = Cached {
                boxes,
                image: layer.image,
                origin: layer.origin,
                generation: 0,
                blurred: 0.0,
                asked: -1.0,
                used: 0,
                soft: None,
            };
            // The clamp in `Layer::over` can cut a picture that reaches
            // far past the surface down to less than the frame asks for.
            if !entry.covers(want) {
                continue;
            }
            cache.insert(g, entry);
        }
        let entry = cache.entries.get_mut(&g).expect("fresh or kept above");
        if blur > 0.0 && entry.blurred == 0.0 && entry.asked == blur {
            let (w, h) = (entry.image.width as usize, entry.image.height as usize);
            crate::blur::blur_pixels_reduced(&mut entry.image.pixels, w, h, blur);
            entry.blurred = blur;
            if let Some((_, soft)) = entry.soft.take() {
                cache.bytes -= soft.pixels.len() * size_of::<LinearRgba>();
            }
            entry.generation = cache.generation;
            cache.generation += 1;
        }
        entry.asked = blur;
        served.insert(g);
    }
    served
}

/// The groups a box is inside, outermost first.
fn chain_of(laid: &Laid, group: Option<usize>) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut g = group;
    while let Some(i) = g {
        chain.push(i);
        g = laid.groups[i].parent;
    }
    chain.reverse();
    chain
}

/// A box moved so that a buffer's origin is at zero.
fn shifted(b: &Painted, origin: (i64, i64)) -> Painted {
    let (dx, dy) = (origin.0 as f32, origin.1 as f32);
    let mut out = b.clone();
    out.rect[0] -= dx;
    out.rect[1] -= dy;
    out.content_rect[0] -= dx;
    out.content_rect[1] -= dy;
    if let Some((rect, _)) = out.clip.as_mut() {
        rect[0] -= dx;
        rect[1] -= dy;
    }
    out
}

/// What the surface can be marked in: its own boxes and the top-level
/// groups where they land, as the surface's content rectangle. The
/// compositor reads that rather than all of it.
fn content_of(laid: &Laid, placed: &[Option<Bounds>], surface: (u32, u32)) -> Option<[u32; 4]> {
    let (width, height) = surface;
    let mut touched: Option<Bounds> = None;
    for b in &laid.boxes {
        if b.group.is_none() && b.opacity > 0.0 {
            touched = Some(union(touched, reach_of(b)));
        }
    }
    for (g, group) in laid.groups.iter().enumerate() {
        if group.parent.is_none() {
            if let Some(r) = placed[g] {
                touched = Some(union(touched, r));
            }
        }
    }
    touched.map(|[x0, y0, x1, y1]| {
        let x = x0.floor().clamp(0.0, f64::from(width)) as u32;
        let y = y0.floor().clamp(0.0, f64::from(height)) as u32;
        let right = x1.ceil().clamp(0.0, f64::from(width)) as u32;
        let bottom = y1.ceil().clamp(0.0, f64::from(height)) as u32;
        [x, y, right.saturating_sub(x), bottom.saturating_sub(y)]
    })
}

/// Paints the display list in order. Boxes on the surface are painted
/// straight onto it; a group's boxes go into a buffer of its own, which
/// is composited into whatever holds the group when its last box is
/// done, under the group's opacity, blur, transform and outside clip.
///
/// Two things are left out that would change nothing in the picture:
/// every box under the last one that covers the whole surface in an
/// opaque colour (see [`first_seen`]), and the buffer of a group that
/// would be laid down exactly as it was painted (see [`passes_through`]),
/// whose boxes and inner groups go straight into what holds it.
fn paint(
    laid: &Laid,
    transforms: &[Option<Transform>],
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
    cache: &mut GroupCache,
    spare: Option<Image>,
) -> Image {
    let width = laid.size.0.ceil().max(1.0) as u32;
    let height = laid.size.1.ceil().max(1.0) as u32;
    let (own, placed, natural) =
        group_bounds(laid, transforms, (f64::from(width), f64::from(height)));
    // Whether any group asks for more than the bound allows. Checked
    // here rather than in `Layer::over`, which runs on worker threads and
    // has nowhere to put the answer.
    for b in own.iter().flatten() {
        if bound_group(*b, (width, height)).1 {
            cache.clipped = true;
        }
    }
    let content = content_of(laid, &placed, (width, height));
    let mut surface = match spare {
        Some(mut used)
            if (used.width, used.height) == (width, height)
                && used.pixels.len() == width as usize * height as usize =>
        {
            if let Some([x, y, w, h]) = used.content {
                let stride = width as usize;
                used.pixels[y as usize * stride..(y + h) as usize * stride]
                    .par_chunks_mut(stride)
                    .for_each(|row| {
                        row[x as usize..(x + w) as usize].fill(LinearRgba::TRANSPARENT);
                    });
            }
            used.content = content;
            used
        }
        _ => Image {
            width,
            height,
            pixels: transparent(width as usize * height as usize),
            content,
        },
    };
    let first = first_seen(laid, transforms, (width, height));
    let served = fill_cache(
        cache,
        laid,
        first,
        &own,
        &natural,
        (width, height),
        text,
        images,
    );
    let blends = blends_inside(laid);
    // Whether anything has been laid on the surface yet.
    let mut surface_touched = false;
    let mut stack: Vec<Layer> = Vec::new();
    // A group taken from the cache: its own boxes are already in the
    // picture, so they are not painted again.
    let mut serving: Option<usize> = None;
    // A group that would not show (no opacity, no size, nothing in it)
    // is skipped whole, boxes and inner groups alike.
    let mut skipping: Option<usize> = None;
    for b in &laid.boxes[first.min(laid.boxes.len())..] {
        let chain = chain_of(laid, b.group);
        while let Some(top) = stack.last() {
            if chain.contains(&top.group) {
                break;
            }
            let layer = stack.pop().expect("checked above");
            if skipping == Some(layer.group) {
                skipping = None;
                continue;
            }
            if serving == Some(layer.group) {
                serving = None;
            }
            close(
                layer,
                laid,
                transforms,
                &placed,
                &mut stack,
                &mut surface,
                &mut surface_touched,
            );
        }
        for g in chain {
            if stack.iter().any(|l| l.group == g) {
                continue;
            }
            let group = &laid.groups[g];
            let flat = transforms[g]
                .as_ref()
                .is_some_and(|tr| tr.scale[0] == 0.0 || tr.scale[1] == 0.0);
            let hidden = skipping.is_some() || group.opacity <= 0.0 || flat || own[g].is_none();
            if hidden {
                skipping.get_or_insert(g);
                stack.push(Layer::over(g, None, (width, height)));
                continue;
            }
            let ready = (serving.is_none() && served.contains(&g))
                .then(|| buffer_rect(own[g], (width, height)))
                .flatten()
                .and_then(|want| cache.window(g, want, blur_step(group.blur)));
            if let Some(layer) = ready {
                serving = Some(g);
                stack.push(layer);
                continue;
            }
            let (target, touched) = match stack.iter().rev().find(|l| !l.through) {
                Some(l) => (l.bounds(), l.touched),
                None => ([0, 0, i64::from(width), i64::from(height)], surface_touched),
            };
            if passes_through(group, transforms[g].as_ref(), target, blends[g] && touched) {
                stack.push(Layer::through(g));
            } else {
                stack.push(Layer::over(g, own[g], (width, height)));
            }
        }
        if skipping.is_some() || serving.is_some() || b.opacity <= 0.0 {
            continue;
        }
        match stack.iter_mut().rev().find(|l| !l.through) {
            Some(layer) => {
                let moved = shifted(b, layer.origin);
                paint_one(&mut layer.image, &moved, text, images);
                layer.touched |= marks(b);
            }
            None => {
                paint_one(&mut surface, b, text, images);
                surface_touched |= marks(b);
            }
        }
    }
    while let Some(layer) = stack.pop() {
        if skipping == Some(layer.group) {
            skipping = None;
            continue;
        }
        if serving == Some(layer.group) {
            serving = None;
        }
        close(
            layer,
            laid,
            transforms,
            &placed,
            &mut stack,
            &mut surface,
            &mut surface_touched,
        );
    }
    surface
}

/// Where painting can start: the last box that covers the whole surface
/// in an opaque colour, laid straight onto it, since nothing painted
/// before that box can show. The first box when there is no such box.
///
/// A box qualifies when its background is one colour with full alpha, it
/// has full opacity, its rounded rectangle and any clip on it cover every
/// pixel of the surface whole, and each group it is inside is laid down
/// as painted: full opacity, no blur, no blend mode, no transform, and
/// any clip or `clip-path` covering the surface too. That is a panel
/// wiped in over the last one, once the wipe is done.
fn first_seen(laid: &Laid, transforms: &[Option<Transform>], surface: (u32, u32)) -> usize {
    let whole = [0, 0, i64::from(surface.0), i64::from(surface.1)];
    let opaque = |b: &Painted| {
        let solid = matches!(
            &b.paint.background,
            Some(geneva_html::style::Background::Color(c)) if c.a >= 1.0
        );
        solid
            && b.opacity >= 1.0
            && RoundRect::new(b.rect, b.paint.radius).covers(whole)
            && clip_of(b).is_none_or(|c| c.covers(whole))
            && chain_of(laid, b.group).iter().all(|&g| {
                let group = &laid.groups[g];
                group.opacity >= 1.0
                    && group.blur <= 0.0
                    && group.blend == geneva_html::Blend::Normal
                    && group.mask.is_none()
                    && transforms[g].is_none()
                    && group
                        .clip
                        .is_none_or(|(rect, radius)| RoundRect::new(rect, radius).covers(whole))
                    && group
                        .clip_path
                        .as_ref()
                        .is_none_or(|points| polygon_covers(points, whole))
            })
    };
    laid.boxes.iter().rposition(opaque).unwrap_or(0)
}

/// Whether a group can skip its buffer and have its boxes and inner
/// groups painted straight into `target` (the buffer that holds it, as
/// left, top, right and bottom on the surface): whether laying the
/// buffer down would leave exactly what was painted in it. That takes
/// full opacity, no blur, no blend mode and no transform, and any clip
/// or `clip-path` it has covering the whole of the target, so that
/// nothing painted there would have been cut.
///
/// A group with a blend mode somewhere inside it blends against what its
/// own buffer holds, which starts out empty. Painted straight, the same
/// element would blend against whatever is already in the target, so
/// such a group goes straight only while the target is still empty
/// (`isolated` false): an intro's stage whose blurred shapes are mixed
/// in `screen`, drawn first on the surface.
fn passes_through(
    group: &Group,
    transform: Option<&Transform>,
    target: [i64; 4],
    isolated: bool,
) -> bool {
    !isolated
        && group.opacity >= 1.0
        && group.blur <= 0.0
        && group.blend == geneva_html::Blend::Normal
        && transform.is_none()
        && group
            .clip
            .is_none_or(|(rect, radius)| RoundRect::new(rect, radius).covers(target))
        && group.mask.is_none()
        && group
            .clip_path
            .as_ref()
            .is_none_or(|points| polygon_covers(points, target))
}

/// For each group, whether a group inside it (at any depth) has a blend
/// mode.
fn blends_inside(laid: &Laid) -> Vec<bool> {
    let mut inside = vec![false; laid.groups.len()];
    for group in &laid.groups {
        if group.blend == geneva_html::Blend::Normal {
            continue;
        }
        let mut up = group.parent;
        while let Some(p) = up {
            inside[p] = true;
            up = laid.groups[p].parent;
        }
    }
    inside
}

/// Whether painting a box can leave a mark: anything in it to draw.
fn marks(b: &Painted) -> bool {
    b.paint
        .background
        .as_ref()
        .is_some_and(geneva_html::style::Background::visible)
        || b.paint.shadow.iter().any(|s| s.color.a > 0.0)
        || (b.border.iter().any(|w| *w > 0.0) && b.paint.border_color.iter().any(|c| c.a > 0.0))
        || !matches!(b.content, Content::Empty)
}

/// `n` transparent pixels. A frame's worth is cleared on every thread at
/// once rather than on one.
fn transparent(n: usize) -> Vec<LinearRgba> {
    if n < 4 * ROWS_PER_JOB {
        return vec![LinearRgba::TRANSPARENT; n];
    }
    let mut pixels = Vec::with_capacity(n);
    (0..n)
        .into_par_iter()
        .with_min_len(ROWS_PER_JOB)
        .map(|_| LinearRgba::TRANSPARENT)
        .collect_into_vec(&mut pixels);
    pixels
}

/// Composites a finished group into the buffer under it that is not
/// itself going straight through, or the surface; a group going
/// straight through has nothing to composite.
fn close(
    layer: Layer,
    laid: &Laid,
    transforms: &[Option<Transform>],
    placed: &[Option<Bounds>],
    stack: &mut [Layer],
    surface: &mut Image,
    surface_touched: &mut bool,
) {
    if layer.through {
        return;
    }
    let group = &laid.groups[layer.group];
    let tr = transforms[layer.group].as_ref();
    let landing = placed[layer.group];
    match stack.iter_mut().rev().find(|l| !l.through) {
        Some(parent) => {
            let origin = parent.origin;
            parent.touched = true;
            composite(&mut parent.image, origin, layer, group, tr, landing);
        }
        None => {
            *surface_touched = true;
            composite(surface, (0, 0), layer, group, tr, landing);
        }
    }
}

/// One box onto a buffer.
fn paint_one(
    image: &mut Image,
    b: &Painted,
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
) {
    paint_shadow(image, b);
    paint_box(image, b);
    match &b.content {
        Content::Empty => {}
        Content::Text { text: run, style } => paint_text(image, b, run, style, text, None),
        Content::Rich { runs, style } => {
            let joined: String = runs.iter().map(|(piece, _)| piece.as_str()).collect();
            paint_text(image, b, &joined, style, text, Some(runs));
        }
        Content::Image { src } => {
            if let Some(source) = images.get(src) {
                paint_image(image, b, source);
            }
        }
    }
}

/// Lays a group's buffer into `dst`, whose top-left pixel sits at
/// `dst_origin` on the surface, under the group's opacity, blur,
/// transform and the clip an ancestor outside it imposes.
fn composite(
    dst: &mut Image,
    dst_origin: (i64, i64),
    mut layer: Layer,
    group: &Group,
    tr: Option<&Transform>,
    landing: Option<Bounds>,
) {
    if layer.image.width == 0 || layer.image.height == 0 {
        return;
    }
    let sigma = blur_step(group.blur);
    if sigma > 0.0 && !layer.blurred {
        let (w, h) = (layer.image.width as usize, layer.image.height as usize);
        crate::blur::blur_pixels_reduced(&mut layer.image.pixels, w, h, sigma);
    }
    // CSS clips after filtering and before the transform, so the polygon
    // is applied in the buffer, where the group's own pixels are. A
    // polygon that holds the whole buffer, a wipe once it is done, would
    // change nothing.
    if let Some(points) = &group.clip_path
        && !polygon_covers(points, layer.bounds())
    {
        mask_polygon(&mut layer.image, layer.origin, points);
    }
    if let Some(mask) = &group.mask {
        mask_image(&mut layer.image, layer.origin, mask);
    }
    let opacity = group.opacity.clamp(0.0, 1.0);
    // Chosen once for the group: a group that does not blend takes the
    // same path it always did, down to the instruction.
    let blend = blend_mode(group.blend);
    let lay: fn(&mut [LinearRgba], usize, LinearRgba, f32, BlendMode) =
        if blend == BlendMode::Normal {
            |row, x, texel, a, _| over(row, x, texel, a)
        } else {
            mix
        };
    // Most pixels of a full-frame group are inside the clip whole, and
    // skip its distance test.
    let clip = group
        .clip
        .map(|(rect, radius)| RoundRect::new(rect, radius));
    let clip = |px: f64, py: f64| -> f32 { clip_at(clip.as_ref(), px, py) };
    let src = &layer.image;
    let (sw, sh) = (i64::from(src.width), i64::from(src.height));
    let (dw, dh) = (i64::from(dst.width), i64::from(dst.height));
    let Some(tr) = tr else {
        // Straight on: pixel for pixel, offset by the two origins. Rows
        // are independent, so they go in parallel.
        let (ox, oy) = (layer.origin.0 - dst_origin.0, layer.origin.1 - dst_origin.1);
        let (x0, x1) = (ox.max(0), (sw + ox).min(dw));
        let (y0, y1) = (oy.max(0), (sh + oy).min(dh));
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        rows(dst, y0, y1).for_each(|(dy, row)| {
            let src_row = &src.pixels[((dy - oy) * sw) as usize..][..sw as usize];
            let py = (dy + dst_origin.1) as f64 + 0.5;
            for dx in x0..x1 {
                let texel = src_row[(dx - ox) as usize];
                if texel.a <= 0.0 {
                    continue;
                }
                let px = (dx + dst_origin.0) as f64 + 0.5;
                lay(row, dx as usize, texel, opacity * clip(px, py), blend);
            }
        });
        return;
    };
    // Through the transform: each destination pixel inside where the
    // group lands is mapped back into the buffer and sampled there. A
    // group drawn smaller is sampled more than once per pixel, so its
    // edges do not alias.
    let Some(l) = landing else {
        return;
    };
    let centre = group.pivot;
    let x0 = (l[0].floor() as i64 - dst_origin.0).max(0);
    let y0 = (l[1].floor() as i64 - dst_origin.1).max(0);
    let x1 = (l[2].ceil() as i64 - dst_origin.0).min(dw);
    let y1 = (l[3].ceil() as i64 - dst_origin.1).min(dh);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // The inverse is affine, so it is held as two linear expressions in
    // the destination pixel rather than rebuilt from the rotation and the
    // scale at every tap:
    //     u = ax * px + bx * py + cx
    //     v = ay * px + by * py + cy
    // in the buffer's own pixels, the buffer's origin folded into the
    // constants. The trigonometry and the two divisions happen once for
    // the whole group. Along a row `py` is fixed, so the part that
    // changes is one add per pixel.
    let [ax, bx, cx, ay, by, cy] = affine_inverse(tr, centre, layer.origin);
    let (nx, ny) = taps_of(tr);
    let norm = 1.0 / (nx * ny) as f32;
    let (fw, fh) = (sw as f64, sh as f64);

    // Rows are independent, so they go in parallel. Only the multi-tap
    // path below fills the row's buffer of sums, and an empty vector has
    // not allocated, so a group taking one tap a pixel never pays for it.
    rows(dst, y0, y1).for_each_init(Vec::<LinearRgba>::new, |sums, (dy, row)| {
        let py_row = (dy + dst_origin.1) as f64;
        if nx == 1 && ny == 1 {
            // A group drawn at its own size or larger, which is most of
            // them: one tap a pixel, written straight out, with the
            // sampling point carried along the row.
            let py = py_row + 0.5;
            let px0 = (x0 + dst_origin.0) as f64 + 0.5;
            let mut u = ax * px0 + bx * py + cx;
            let mut v = ay * px0 + by * py + cy;
            for dx in x0..x1 {
                if u >= 0.0 && v >= 0.0 && u < fw && v < fh {
                    let texel = src.sample(u, v);
                    if texel.a > 0.0 {
                        let px = (dx + dst_origin.0) as f64 + 0.5;
                        lay(row, dx as usize, texel, opacity * clip(px, py), blend);
                    }
                }
                u += ax;
                v += ay;
            }
            return;
        }
        // Drawn smaller: several taps a pixel, so its edges do not
        // alias. The taps of a whole row are gathered before the row is
        // written, which keeps the same one add a pixel per tap.
        sums.clear();
        sums.resize((x1 - x0) as usize, LinearRgba::TRANSPARENT);
        for j in 0..ny {
            let py = py_row + (j as f64 + 0.5) / ny as f64;
            let (row_u, row_v) = (bx * py + cx, by * py + cy);
            for i in 0..nx {
                let px0 = (x0 + dst_origin.0) as f64 + (i as f64 + 0.5) / nx as f64;
                let mut u = ax * px0 + row_u;
                let mut v = ay * px0 + row_v;
                for sum in sums.iter_mut() {
                    if u >= 0.0 && v >= 0.0 && u < fw && v < fh {
                        let s = src.sample(u, v);
                        sum.r += s.r;
                        sum.g += s.g;
                        sum.b += s.b;
                        sum.a += s.a;
                    }
                    u += ax;
                    v += ay;
                }
            }
        }
        for (k, sum) in sums.iter().enumerate() {
            if sum.a <= 0.0 {
                continue;
            }
            let dx = x0 + k as i64;
            let texel = sum.scaled(norm);
            let px = (dx + dst_origin.0) as f64 + 0.5;
            lay(
                row,
                dx as usize,
                texel,
                opacity * clip(px, py_row + 0.5),
                blend,
            );
        }
    });
}

/// The inverse of a group's transform about `centre`, as the six
/// constants of two linear expressions in a surface point `(px, py)`:
///     u = ax * px + bx * py + cx
///     v = ay * px + by * py + cy
/// in the pixels of a buffer whose top-left pixel sits at `origin`,
/// returned as `[ax, bx, cx, ay, by, cy]`. [`inverse`] undone once for
/// the whole group rather than at every tap.
fn affine_inverse(tr: &Transform, centre: (f64, f64), origin: (i64, i64)) -> [f64; 6] {
    let (sin, cos) = tr.rotate.to_radians().sin_cos();
    let (inv_sx, inv_sy) = (1.0 / tr.scale[0], 1.0 / tr.scale[1]);
    let (ax, bx) = (cos * inv_sx, sin * inv_sx);
    let (ay, by) = (-sin * inv_sy, cos * inv_sy);
    let (tx, ty) = (centre.0 + tr.translate[0], centre.1 + tr.translate[1]);
    let cx = centre.0 - ax * tx - bx * ty - origin.0 as f64;
    let cy = centre.1 - ay * tx - by * ty - origin.1 as f64;
    [ax, bx, cx, ay, by, cy]
}

/// How many taps a pixel takes along each axis under a transform: one
/// for a group drawn at its own size or larger, up to four for one drawn
/// smaller, so its edges do not alias.
fn taps_of(tr: &Transform) -> (usize, usize) {
    let taps = |scale: f64| ((1.0 / scale.abs().max(1e-3)).ceil() as usize).clamp(1, 4);
    (taps(tr.scale[0]), taps(tr.scale[1]))
}

/// Keeps of a buffer whose top-left pixel sits at `origin` what the
/// mask's alpha keeps at each pixel's centre; outside a tile that does
/// not repeat, nothing. The gradient's alpha is read with its stops
/// mixed premultiplied, as a browser mixes them.
fn mask_image(image: &mut Image, origin: (i64, i64), mask: &geneva_html::layout::GroupMask) {
    let w = image.width as usize;
    if w == 0 {
        return;
    }
    let fill = Fill::new_encoded(&mask.image, mask.tile);
    let (tx, ty, tw, th) = mask.tile;
    image
        .pixels
        .par_chunks_mut(w)
        .enumerate()
        .for_each(|(y, row)| {
            let py = (y as i64 + origin.1) as f64 + 0.5;
            let row_in = mask.repeat.1 || (py >= ty && py < ty + th);
            for (x, p) in row.iter_mut().enumerate() {
                if p.a <= 0.0 {
                    continue;
                }
                let px = (x as i64 + origin.0) as f64 + 0.5;
                let inside = row_in && (mask.repeat.0 || (px >= tx && px < tx + tw));
                let keep = if inside { fill.at(px, py).a } else { 0.0 };
                *p = p.scaled(keep);
            }
        });
}

/// Keeps what is inside the polygon, given in the surface's pixels, of a
/// buffer whose top-left pixel sits at `origin`: each pixel is scaled by
/// its [`polygon_coverage`].
fn mask_polygon(image: &mut Image, origin: (i64, i64), points: &[(f64, f64)]) {
    let (w, h) = (image.width as usize, image.height as usize);
    let coverage = polygon_coverage(w, h, origin, points);
    image
        .pixels
        .par_chunks_mut(w.max(1))
        .with_min_len(ROWS_PER_JOB / w.max(1) + 1)
        .zip(coverage.par_chunks(w.max(1)))
        .for_each(|(row, coverage)| {
            for (p, c) in row.iter_mut().zip(coverage) {
                if *c < 1.0 {
                    *p = p.scaled(*c);
                }
            }
        });
}

/// The coverage in `[0, 1]` of a polygon, given in the surface's pixels,
/// over each pixel of a `w` by `h` buffer whose top-left pixel sits at
/// `origin`, row-major. Coverage is measured on four scanlines per row
/// with exact horizontal overlap, and the fill rule is nonzero, as
/// CSS's is. A polygon of fewer than three points covers nothing.
fn polygon_coverage(w: usize, h: usize, origin: (i64, i64), points: &[(f64, f64)]) -> Vec<f32> {
    const SUB: usize = 4;
    let mut out = vec![0f32; w * h];
    if points.len() < 3 || w == 0 || h == 0 {
        return out;
    }
    // Rows are independent, so they go in parallel, each thread with a
    // crossings list of its own, and in jobs big enough to be worth
    // handing to a thread: one row of a 960-wide buffer is 4 kB of work,
    // which rayon spends more than that splitting and scheduling. On
    // this box a frame-wide clip took 3.0 ms on one thread and 12.2 on
    // four before this line.
    out.par_chunks_mut(w)
        .with_min_len(ROWS_PER_JOB / w + 1)
        .enumerate()
        .for_each_init(Vec::<(f64, i32)>::new, |crossings, (y, coverage)| {
            for s in 0..SUB {
                let sy = origin.1 as f64 + y as f64 + (s as f64 + 0.5) / SUB as f64;
                crossings.clear();
                for i in 0..points.len() {
                    let (x0, y0) = points[i];
                    let (x1, y1) = points[(i + 1) % points.len()];
                    if (y0 <= sy) == (y1 <= sy) {
                        continue;
                    }
                    let t = (sy - y0) / (y1 - y0);
                    crossings.push((x0 + t * (x1 - x0), if y1 > y0 { 1 } else { -1 }));
                }
                crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut winding = 0;
                let mut start = 0.0;
                for (x, dir) in crossings.iter() {
                    let was = winding;
                    winding += dir;
                    if was == 0 && winding != 0 {
                        start = *x;
                    } else if was != 0 && winding == 0 {
                        // A span across [start, x], in buffer pixels. The
                        // pixels it covers whole take the same share
                        // whatever their position, so only the pixel at
                        // each end is worked out: the span across a
                        // frame-wide clip is thousands of pixels and two
                        // of them are partial.
                        let a = (start - origin.0 as f64).clamp(0.0, w as f64);
                        let b = (x - origin.0 as f64).clamp(0.0, w as f64);
                        let share = 1.0 / SUB as f32;
                        let (inner, end) = (a.ceil() as usize, b.floor() as usize);
                        for c in coverage.iter_mut().take(end.min(w)).skip(inner) {
                            *c += share;
                        }
                        // The pixel a starts inside, when it does not
                        // start on a boundary.
                        let head = a.floor() as usize;
                        if head < inner && head < w {
                            let overlap = b.min(head as f64 + 1.0) - a;
                            if overlap > 0.0 {
                                coverage[head] += overlap as f32 * share;
                            }
                        }
                        // The pixel b ends inside, unless the head above
                        // already counted that pixel. A span starting on a
                        // boundary has no head, so `end` is its only
                        // partial pixel and it is counted here.
                        if end < w && end >= inner && b > end as f64 {
                            coverage[end] += (b - end as f64) as f32 * share;
                        }
                    }
                }
            }
            for c in coverage.iter_mut() {
                *c = c.clamp(0.0, 1.0);
            }
        });
    out
}

/// How many pixels a parallel job should hold at least. A job smaller
/// than this costs more to hand to a thread than to do.
const ROWS_PER_JOB: usize = 1 << 15;

/// The rows `y0..y1` of an image as a parallel iterator, each with its
/// index. Every painting loop is a loop over rows whose pixels depend
/// on nothing but their own inputs, so the rows can be shared out
/// across threads and the picture is the same whatever the order they
/// finish in. A small box is not worth splitting: each job gets rows
/// enough to hold about 32k pixels.
fn rows(
    image: &mut Image,
    y0: i64,
    y1: i64,
) -> impl IndexedParallelIterator<Item = (i64, &mut [LinearRgba])> {
    let w = image.width as usize;
    // Both ends inside the image: a box wholly below it has no rows.
    let (y0, y1) = (
        (y0.max(0) as usize).min(image.height as usize),
        (y1.max(0) as usize).min(image.height as usize),
    );
    let (from, to) = (y0 * w, y1.max(y0) * w);
    image.pixels[from..to]
        .par_chunks_mut(w.max(1))
        .with_min_len(ROWS_PER_JOB / w.max(1) + 1)
        .enumerate()
        .map(move |(i, row)| (y0 as i64 + i as i64, row))
}

/// A premultiplied pixel over the one at `x` of a row, at a coverage.
#[inline]
fn over(row: &mut [LinearRgba], x: usize, texel: LinearRgba, a: f32) {
    if a <= 0.0 {
        return;
    }
    let d = row[x];
    let inv = 1.0 - texel.a * a;
    row[x] = LinearRgba {
        r: texel.r * a + d.r * inv,
        g: texel.g * a + d.g * inv,
        b: texel.b * a + d.b * inv,
        a: texel.a * a + d.a * inv,
    };
}

/// The same, mixed with what is behind it by a blend mode.
///
/// The group's buffer and the surface are both in the painter's working
/// space, which is sRGB-encoded, and that is the space CSS blends in, so
/// the compositor's own function applies here unchanged.
#[inline]
fn mix(row: &mut [LinearRgba], x: usize, texel: LinearRgba, a: f32, mode: BlendMode) {
    if a <= 0.0 {
        return;
    }
    let src = LinearRgba {
        r: texel.r * a,
        g: texel.g * a,
        b: texel.b * a,
        a: texel.a * a,
    };
    row[x] = crate::cpu::composite(src, row[x], mode);
}

/// A markup blend mode as the compositor's, which implements the same
/// separable modes of the CSS specification under its own names.
pub fn blend_mode(blend: geneva_html::Blend) -> BlendMode {
    match blend {
        geneva_html::Blend::Normal => BlendMode::Normal,
        geneva_html::Blend::Multiply => BlendMode::Multiply,
        geneva_html::Blend::Screen => BlendMode::Screen,
        geneva_html::Blend::Overlay => BlendMode::Overlay,
        geneva_html::Blend::Darken => BlendMode::Darken,
        geneva_html::Blend::Lighten => BlendMode::Lighten,
        geneva_html::Blend::Difference => BlendMode::Difference,
        geneva_html::Blend::SoftLight => BlendMode::SoftLight,
    }
}

fn distance(px: f64, py: f64, rect: Rectangle, radius: [f64; 4]) -> f64 {
    let (w, h) = (f64::from(rect[2]), f64::from(rect[3]));
    if w <= 0.0 || h <= 0.0 {
        return 1.0;
    }
    let cx = px - f64::from(rect[0]) - w / 2.0;
    let cy = py - f64::from(rect[1]) - h / 2.0;
    // A radius never takes more than half the shorter side.
    let limit = (w / 2.0).min(h / 2.0);
    let r = match (cx > 0.0, cy > 0.0) {
        (false, false) => radius[0],
        (true, false) => radius[1],
        (true, true) => radius[2],
        (false, true) => radius[3],
    }
    .clamp(0.0, limit);
    let qx = cx.abs() - (w / 2.0 - r);
    let qy = cy.abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Coverage of a rounded rectangle at a pixel centre, softened over one
/// pixel so edges do not stair-step.
fn coverage(px: f64, py: f64, rect: Rectangle, radius: [f64; 4]) -> f32 {
    (0.5 - distance(px, py, rect, radius)).clamp(0.0, 1.0) as f32
}

/// A rounded rectangle read at many pixels, with the part of it that
/// covers a pixel whole worked out once: everywhere but a half-pixel
/// band at its edge and its rounded corners. A pixel centre inside that
/// part skips the square root and the branches of [`distance`], which is
/// most of the pixels of a big box.
#[derive(Clone, Copy)]
struct RoundRect {
    rect: Rectangle,
    radius: [f64; 4],
    /// Left, top, right and bottom of the part covered whole.
    solid: Bounds,
}

impl RoundRect {
    fn new(rect: Rectangle, radius: [f64; 4]) -> Self {
        let inset = radius.iter().fold(0.5f64, |a, r| a.max(*r));
        Self {
            rect,
            radius,
            solid: [
                f64::from(rect[0]) + inset,
                f64::from(rect[1]) + inset,
                f64::from(rect[0] + rect[2]) - inset,
                f64::from(rect[1] + rect[3]) - inset,
            ],
        }
    }

    /// The coverage at a pixel centre, as [`coverage`] gives it.
    #[inline]
    fn coverage(&self, px: f64, py: f64) -> f32 {
        let s = self.solid;
        if px >= s[0] && py >= s[1] && px <= s[2] && py <= s[3] {
            return 1.0;
        }
        coverage(px, py, self.rect, self.radius)
    }

    /// Whether every pixel of `r` (left, top, right, bottom, in whole
    /// pixels) is covered whole.
    fn covers(&self, r: [i64; 4]) -> bool {
        let s = self.solid;
        r[2] <= r[0]
            || r[3] <= r[1]
            || (s[0] <= r[0] as f64 + 0.5
                && s[1] <= r[1] as f64 + 0.5
                && s[2] >= r[2] as f64 - 0.5
                && s[3] >= r[3] as f64 - 0.5)
    }
}

/// The clip an ancestor imposes on a box, ready to be read per pixel.
fn clip_of(b: &Painted) -> Option<RoundRect> {
    b.clip.map(|(rect, radius)| RoundRect::new(rect, radius))
}

/// A clip as coverage at a pixel centre; no clip covers everything.
#[inline]
fn clip_at(clip: Option<&RoundRect>, px: f64, py: f64) -> f32 {
    clip.map_or(1.0, |c| c.coverage(px, py))
}

/// The rows and columns a rectangle touches, clamped to the image.
fn bounds(image: &Image, rect: Rectangle, grow: f64) -> (u32, u32, u32, u32) {
    let x0 = (f64::from(rect[0]) - grow).floor().max(0.0) as u32;
    let y0 = (f64::from(rect[1]) - grow).floor().max(0.0) as u32;
    let x1 = ((f64::from(rect[0] + rect[2]) + grow).ceil().max(0.0) as u32).min(image.width);
    let y1 = ((f64::from(rect[1] + rect[3]) + grow).ceil().max(0.0) as u32).min(image.height);
    (x0, y0, x1.max(x0), y1.max(y0))
}

/// A colour at an alpha over the pixel at `x` of a row.
#[inline]
fn blend(row: &mut [LinearRgba], x: usize, color: LinearRgba, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let src = LinearRgba {
        r: color.r * alpha,
        g: color.g * alpha,
        b: color.b * alpha,
        a: color.a * alpha,
    };
    let dst = row[x];
    let inv = 1.0 - src.a;
    row[x] = LinearRgba {
        r: src.r + dst.r * inv,
        g: src.g + dst.g * inv,
        b: src.b + dst.b * inv,
        a: src.a + dst.a * inv,
    };
}

/// The inner edge of the border, which is where the padding box starts.
fn inner(b: &Painted) -> (Rectangle, [f64; 4]) {
    let [top, right, bottom, left] = b.border;
    let rect = [
        b.rect[0] + left,
        b.rect[1] + top,
        (b.rect[2] - left - right).max(0.0),
        (b.rect[3] - top - bottom).max(0.0),
    ];
    let radius = [
        (b.paint.radius[0] - f64::from(left.max(top))).max(0.0),
        (b.paint.radius[1] - f64::from(right.max(top))).max(0.0),
        (b.paint.radius[2] - f64::from(right.max(bottom))).max(0.0),
        (b.paint.radius[3] - f64::from(left.max(bottom))).max(0.0),
    ];
    (rect, radius)
}

/// Which border a pixel belongs to, when the four colours differ.
fn side(b: &Painted, px: f64, py: f64) -> usize {
    let [top, right, bottom, left] = b.border.map(|v| f64::from(v).max(0.001));
    let d = [
        (py - f64::from(b.rect[1])) / top,
        (f64::from(b.rect[0] + b.rect[2]) - px) / right,
        (f64::from(b.rect[1] + b.rect[3]) - py) / bottom,
        (px - f64::from(b.rect[0])) / left,
    ];
    let mut best = 0;
    for (i, v) in d.iter().enumerate() {
        if *v < d[best] {
            best = i;
        }
    }
    best
}

/// The tile a background repeats over a box: its `background-size`
/// placed at its `background-position`.
fn tile(
    rect: Rectangle,
    size: (Extent, Extent),
    position: (Extent, Extent),
) -> (f64, f64, f64, f64) {
    let (w, h) = (f64::from(rect[2]), f64::from(rect[3]));
    let (tw, th) = (size.0.size(w).max(1.0), size.1.size(h).max(1.0));
    (
        f64::from(rect[0]) + position.0.position(w, tw),
        f64::from(rect[1]) + position.1.position(h, th),
        tw,
        th,
    )
}

fn paint_box(image: &mut Image, b: &Painted) {
    let has_border =
        b.border.iter().any(|w| *w > 0.0) && b.paint.border_color.iter().any(|c| c.a > 0.0);
    let background = b.paint.background.as_ref().filter(|bg| bg.visible());
    if background.is_none() && !has_border {
        return;
    }
    let fill = background.map(|bg| {
        Fill::new_encoded(
            bg,
            tile(b.rect, b.paint.background_size, b.paint.background_position),
        )
    });
    let (inner_rect, inner_radius) = inner(b);
    let outer = RoundRect::new(b.rect, b.paint.radius);
    let hole = RoundRect::new(inner_rect, inner_radius);
    let clip = clip_of(b);
    let (x0, y0, x1, y1) = bounds(image, b.rect, 1.0);
    rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in x0..x1 {
            let px = f64::from(x) + 0.5;
            let outer = outer.coverage(px, py);
            if outer <= 0.0 {
                continue;
            }
            let clip = clip_at(clip.as_ref(), px, py);
            if clip <= 0.0 {
                continue;
            }
            if let Some(fill) = &fill {
                blend(row, x as usize, fill.at(px, py), outer * clip * b.opacity);
            }
            if !has_border {
                continue;
            }
            let hole = hole.coverage(px, py);
            if outer > hole {
                let colour = b.paint.border_color[side(b, px, py)];
                if colour.a > 0.0 {
                    blend(
                        row,
                        x as usize,
                        encoded(colour),
                        (outer - hole) * clip * b.opacity,
                    );
                }
            }
        }
    });
}

/// The box's shadows, the last in the list laid down first, as CSS
/// paints them.
fn paint_shadow(image: &mut Image, b: &Painted) {
    for shadow in b.paint.shadow.iter().rev() {
        paint_one_shadow(image, b, shadow);
    }
}

fn paint_one_shadow(image: &mut Image, b: &Painted, shadow: &geneva_html::style::Shadow) {
    if shadow.color.a <= 0.0 {
        return;
    }
    let rect = [
        b.rect[0] + shadow.x as f32,
        b.rect[1] + shadow.y as f32,
        b.rect[2],
        b.rect[3],
    ];
    let grow = shadow.blur.max(0.0) + 1.0;
    let (x0, y0, x1, y1) = bounds(image, rect, grow);
    let colour = encoded(shadow.color);
    let clip = clip_of(b);
    // An outer shadow is drawn only outside the box's border edge, as CSS
    // has it: a translucent box does not show its own shadow through it.
    let own = RoundRect::new(b.rect, b.paint.radius);
    let outside = |px: f64, py: f64| 1.0 - own.coverage(px, py);
    if shadow.blur <= 0.0 {
        let shape = RoundRect::new(rect, b.paint.radius);
        rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
            let py = y as f64 + 0.5;
            for x in x0..x1 {
                let px = f64::from(x) + 0.5;
                let a = shape.coverage(px, py) * clip_at(clip.as_ref(), px, py) * outside(px, py);
                blend(row, x as usize, colour, a * b.opacity);
            }
        });
        return;
    }
    // A blurred shadow is the box convolved with a Gaussian whose
    // standard deviation is half the blur, as CSS defines it. For an
    // axis-aligned box that is exact as the product of one ramp per
    // axis, each the difference of two error functions, so a bar
    // thinner than its blur comes out as faint as it should rather than
    // solid to its edge. Corners rounder than the blur do not show
    // through it, so the radii are left out here.
    let sigma = shadow.blur / 2.0;
    let k = 1.0 / (sigma * std::f64::consts::SQRT_2);
    let ramp = |lo: f64, hi: f64, p: f64| 0.5 * (erf((p - lo) * k) - erf((p - hi) * k));
    let (left, top) = (f64::from(rect[0]), f64::from(rect[1]));
    let (right, bottom) = (left + f64::from(rect[2]), top + f64::from(rect[3]));
    rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        let down = ramp(top, bottom, py);
        for x in x0..x1 {
            let px = f64::from(x) + 0.5;
            let a = (ramp(left, right, px) * down).clamp(0.0, 1.0) as f32;
            if a > 0.0 {
                blend(
                    row,
                    x as usize,
                    colour,
                    a * clip_at(clip.as_ref(), px, py) * outside(px, py) * b.opacity,
                );
            }
        }
    });
}

/// The error function, to within 1.5e-7 (Abramowitz and Stegun 7.1.26).
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    sign * (1.0 - poly * (-x * x).exp())
}

/// Draws a box's text. `pieces`, when there are some, are the run in
/// several styles; `run` is then their text joined, and `style` the
/// element's, which the whole run shares.
fn paint_text(
    image: &mut Image,
    b: &Painted,
    run: &str,
    style: &Text,
    engine: &mut TextEngine,
    pieces: Option<&[(String, Text)]>,
) {
    if run.trim().is_empty() {
        return;
    }
    let box_width = f64::from(b.content_rect[2]).max(1.0);
    let mut source = as_text_source(run, style, box_width);
    let pieces = pieces.map(engine_runs);
    let draw = |engine: &mut TextEngine, source: &ResolvedText| match &pieces {
        Some(runs) => engine.render_runs(source, runs, 0.0),
        None => engine.render(source, 0.0),
    };
    let inset = crate::text::inset_for(&source, pieces.as_deref());
    // Where the glyphs sit in the run's box: aligned text that is
    // narrower than its box is shifted along it by a share of the slack.
    let shift = |drawn_width: f64| {
        let slack = box_width - (drawn_width - 2.0 * inset);
        match style.align {
            geneva_html::TextAlign::Left => 0.0,
            geneva_html::TextAlign::Center => slack / 2.0,
            geneva_html::TextAlign::Right => slack,
        }
    };
    if let Some(fill) = &style.fill {
        // The tile sits on the run's box, and the engine's on the text's
        // own, so an aligned run needs its width first to place one on
        // the other. A left-aligned run has no slack to measure.
        let along = match style.align {
            geneva_html::TextAlign::Left => 0.0,
            _ => shift(f64::from(draw(engine, &source).width)),
        };
        source.fill = Some(fill_track(fill, b.content_rect, along));
    }
    // The engine draws in linear light; the page it lands on is not.
    let drawn = to_encoded(draw(engine, &source));
    // The engine leaves room around the glyphs for a stroke and a
    // shadow. Layout did not count it, so painting takes it back off:
    // the glyphs land where they would have with no shadow, and the
    // shadow spills outside the box the way it does on a page.
    let dx = f64::from(b.content_rect[0]) - inset + shift(f64::from(drawn.width));
    let dy = f64::from(b.content_rect[1]) - inset;
    blit(image, b, &drawn, dx, dy, 1.0);
}

/// A markup text fill as the engine takes it: the tile sized against the
/// run's box and placed from the text's own, which starts `along` pixels
/// into the run.
fn fill_track(fill: &TextFill, rect: Rectangle, along: f64) -> FillTrack {
    let (w, h) = (f64::from(rect[2]).max(1.0), f64::from(rect[3]).max(1.0));
    let (tw, th) = (fill.size.0.size(w).max(1.0), fill.size.1.size(h).max(1.0));
    FillTrack::constant(
        fill.background.clone(),
        (Some(tw), Some(th)),
        fill.position.0.position(w, tw) - along,
        fill.position.1.position(h, th),
    )
}

fn paint_image(image: &mut Image, b: &Painted, source: &Image) {
    let rect = b.content_rect;
    if rect[2] <= 0.0 || rect[3] <= 0.0 || source.width == 0 || source.height == 0 {
        return;
    }
    let (x0, y0, x1, y1) = bounds(image, rect, 0.0);
    let sx = f64::from(source.width) / f64::from(rect[2]);
    let sy = f64::from(source.height) / f64::from(rect[3]);
    let clip = clip_of(b);
    rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in x0..x1 {
            let px = f64::from(x) + 0.5;
            let clip = clip_at(clip.as_ref(), px, py);
            if clip <= 0.0 {
                continue;
            }
            let u = (px - f64::from(rect[0])) * sx;
            let v = (py - f64::from(rect[1])) * sy;
            // The sampled pixel is already premultiplied.
            over(row, x as usize, source.sample(u, v), b.opacity * clip);
        }
    });
}

/// Lays a premultiplied image over the canvas at a point, under the box's
/// clip and opacity.
fn blit(image: &mut Image, b: &Painted, src: &Image, dx: f64, dy: f64, scale: f32) {
    let rect = [
        dx as f32,
        dy as f32,
        src.width as f32 * scale,
        src.height as f32 * scale,
    ];
    let (x0, y0, x1, y1) = bounds(image, rect, 0.0);
    let clip = clip_of(b);
    rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in x0..x1 {
            let px = f64::from(x) + 0.5;
            let clip = clip_at(clip.as_ref(), px, py);
            if clip <= 0.0 {
                continue;
            }
            let u = (px - dx) / f64::from(scale);
            let v = (py - dy) / f64::from(scale);
            if u < 0.0 || v < 0.0 || u >= f64::from(src.width) || v >= f64::from(src.height) {
                continue;
            }
            let texel = src.pixels[(v as usize) * src.width as usize + u as usize];
            over(row, x as usize, texel, b.opacity * clip);
        }
    });
}

/// A colour with full alpha, for the places a `Color` is needed.
#[allow(dead_code)]
fn opaque(c: Color) -> Color {
    Color { a: 1.0, ..c }
}

#[cfg(test)]
mod coverage_tests {
    use super::polygon_coverage;

    /// A square laid on the pixel grid covers what it covers, and nothing
    /// of it is lost at an edge that starts on a boundary.
    ///
    /// The second case is the one this exists for: a span from a pixel's
    /// left edge that ends inside the same pixel has no partial pixel at
    /// its start, only at its end, and an earlier version of this dropped
    /// it. The frame's own edge is where it showed, since a polygon
    /// reaching past the buffer is clamped to exactly 0.0 there.
    #[test]
    fn a_sliver_from_a_pixel_boundary_is_not_lost() {
        let whole = polygon_coverage(
            4,
            1,
            (0, 0),
            &[(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (0.0, 1.0)],
        );
        assert_eq!(whole, vec![1.0, 1.0, 0.0, 0.0]);
        // From the left edge to a quarter into the first pixel.
        let sliver = polygon_coverage(
            4,
            1,
            (0, 0),
            &[(0.0, 0.0), (0.25, 0.0), (0.25, 1.0), (0.0, 1.0)],
        );
        assert_eq!(sliver, vec![0.25, 0.0, 0.0, 0.0]);
        // The same sliver inside the buffer, which never regressed.
        let inside = polygon_coverage(
            4,
            1,
            (0, 0),
            &[(2.0, 0.0), (2.25, 0.0), (2.25, 1.0), (2.0, 1.0)],
        );
        assert_eq!(inside, vec![0.0, 0.0, 0.25, 0.0]);
        // A polygon reaching past the left edge is clamped to it, and the
        // part inside the frame still counts.
        let clamped = polygon_coverage(
            4,
            1,
            (0, 0),
            &[(-3.0, 0.0), (0.5, 0.0), (0.5, 1.0), (-3.0, 1.0)],
        );
        assert_eq!(clamped, vec![0.5, 0.0, 0.0, 0.0]);
    }
}

#[cfg(test)]
mod bound_tests {
    use super::bound_group;

    const SURFACE: (u32, u32) = (1280, 720);

    /// A news crawl: a few hundred pixels tall, many thousands wide, and
    /// sitting far to the left of the surface once its animation has run
    /// a while. The old bound was a rectangle two surfaces across, which
    /// this leaves long before the end, and it went blank rather than
    /// saying anything. It is well inside the area the bound allows.
    #[test]
    fn a_crawl_far_past_the_surface_is_kept_whole() {
        let slice = [11_000.0, 0.0, 12_280.0, 40.0];
        let (r, clipped) = bound_group(slice, SURFACE);
        assert!(!clipped, "a 1280x40 slice is not near the budget");
        assert_eq!(r, [11_000, 0, 12_280, 40], "kept where it really is");
    }

    /// The bound still exists: the budget is the area of the rectangle it
    /// used to be, three surfaces by three, so nothing costs more memory
    /// than it used to.
    #[test]
    fn a_group_over_the_budget_is_cut_and_says_so() {
        let w = f64::from(SURFACE.0);
        let h = f64::from(SURFACE.1);
        let huge = [0.0, 0.0, 40.0 * w, 40.0 * h];
        let (r, clipped) = bound_group(huge, SURFACE);
        assert!(clipped, "1600 surfaces of area is over the budget");
        assert_eq!(r, [0, 0, 2 * 1280, 2 * 720], "cut back to the old bound");
    }

    /// Exactly the old rectangle is exactly the budget, so what used to
    /// fit still fits.
    #[test]
    fn the_old_rectangle_still_fits() {
        let w = f64::from(SURFACE.0);
        let h = f64::from(SURFACE.1);
        let (_, clipped) = bound_group([-w, -h, 2.0 * w, 2.0 * h], SURFACE);
        assert!(!clipped, "three surfaces by three is the budget itself");
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::{RoundRect, coverage, polygon_covers, polygon_reach, srgb_from_linear};
    use geneva_color::{LinearRgba, Transfer};
    use geneva_timeline::schema::BlendMode;

    /// A wipe's polygon before it opens is a line, and an iris's is a
    /// point: neither covers anything, so the group is not painted.
    #[test]
    fn a_polygon_with_no_area_reaches_nothing() {
        let line = [(0.0, 0.0), (0.0, 0.0), (-384.0, 1080.0), (-384.0, 1080.0)];
        assert_eq!(polygon_reach(&line), None);
        let point = [(960.0, 540.0); 4];
        assert_eq!(polygon_reach(&point), None);
        let triangle = [(10.0, 20.0), (50.0, 5.0), (30.0, 60.0)];
        assert_eq!(polygon_reach(&triangle), Some([10.0, 5.0, 50.0, 60.0]));
    }

    #[test]
    fn a_polygon_covers_a_rectangle_only_when_it_holds_it_whole() {
        let frame = [0, 0, 1920, 1080];
        // A wipe once it is open, and an iris past the corners.
        let open = [
            (0.0, 0.0),
            (2304.0, 0.0),
            (1920.0, 1080.0),
            (-384.0, 1080.0),
        ];
        assert!(polygon_covers(&open, frame));
        let iris = [
            (-1152.0, 540.0),
            (960.0, -648.0),
            (3072.0, 540.0),
            (960.0, 1728.0),
        ];
        assert!(polygon_covers(&iris, frame));
        // Half way across.
        let half = [(0.0, 0.0), (1000.0, 0.0), (900.0, 1080.0), (-384.0, 1080.0)];
        assert!(!polygon_covers(&half, frame));
        // Repeated points and either winding are fine.
        let backwards = [
            (-10.0, -10.0),
            (-10.0, 2000.0),
            (3000.0, 2000.0),
            (3000.0, -10.0),
            (-10.0, -10.0),
        ];
        assert!(polygon_covers(&backwards, frame));
        // A star turns the same way at every point but is not convex; its
        // corners can sit around the rectangle without holding it.
        let star: Vec<(f64, f64)> = (0..5)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i * 2) / 5.0;
                (960.0 + 5000.0 * a.cos(), 540.0 + 5000.0 * a.sin())
            })
            .collect();
        assert!(!polygon_covers(&star, frame));
        // Concave: an L around the frame's corner.
        let l = [
            (-10.0, -10.0),
            (3000.0, -10.0),
            (3000.0, 500.0),
            (500.0, 500.0),
            (500.0, 2000.0),
            (-10.0, 2000.0),
        ];
        assert!(!polygon_covers(&l, frame));
    }

    /// The shortcut inside a rounded rectangle gives what the distance
    /// field gives, pixel for pixel.
    #[test]
    fn a_round_rect_reads_the_same_as_its_distance_field() {
        for (rect, radius) in [
            ([0.0f32, 0.0, 1920.0, 1080.0], [0.0; 4]),
            ([10.3, 7.7, 120.0, 60.0], [12.0, 0.0, 30.0, 4.0]),
            ([-5.0, 3.0, 40.0, 20.0], [100.0; 4]),
            ([5.0, 5.0, 0.0, 10.0], [0.0; 4]),
        ] {
            let shape = RoundRect::new(rect, radius);
            for y in -4..140 {
                for x in -4..200 {
                    let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                    assert_eq!(
                        shape.coverage(px, py),
                        coverage(px, py, rect, radius),
                        "{rect:?} {radius:?} at {x},{y}"
                    );
                }
            }
        }
        let frame = RoundRect::new([0.0, 0.0, 1920.0, 1080.0], [0.0; 4]);
        assert!(frame.covers([0, 0, 1920, 1080]));
        assert!(!frame.covers([0, 0, 1921, 1080]));
        let round = RoundRect::new([0.0, 0.0, 1920.0, 1080.0], [8.0; 4]);
        assert!(!round.covers([0, 0, 1920, 1080]));
    }

    #[test]
    fn the_encoding_table_is_within_a_hair_of_the_curve() {
        let mut worst = 0.0f64;
        for i in 0..=200_000 {
            let v = f64::from(i) / 200_000.0;
            let got = f64::from(srgb_from_linear(v as f32));
            worst = worst.max((got - Transfer::Srgb.from_linear(v)).abs());
        }
        assert!(worst < 2e-5, "{worst}");
    }

    /// Screen and multiply without dividing by alpha give the general
    /// formula's answer.
    #[test]
    fn premultiplied_screen_and_multiply_match_the_general_formula() {
        let general = |s: LinearRgba, d: LinearRgba, f: fn(f32, f32) -> f32| {
            let straight = |c: f32, a: f32| if a > 0.0 { c / a } else { 0.0 };
            let channel = |cs: f32, cb: f32| {
                cs * (1.0 - d.a)
                    + cb * (1.0 - s.a)
                    + s.a * d.a * f(straight(cb, d.a), straight(cs, s.a))
            };
            LinearRgba {
                r: channel(s.r, d.r),
                g: channel(s.g, d.g),
                b: channel(s.b, d.b),
                a: s.a + d.a * (1.0 - s.a),
            }
        };
        let pixels = [
            LinearRgba {
                r: 0.2,
                g: 0.4,
                b: 0.1,
                a: 0.5,
            },
            LinearRgba {
                r: 0.7,
                g: 0.1,
                b: 0.9,
                a: 1.0,
            },
            LinearRgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            },
            LinearRgba {
                r: 0.05,
                g: 0.3,
                b: 0.3,
                a: 0.3,
            },
        ];
        for s in pixels {
            for d in pixels {
                for (mode, f) in [
                    (
                        BlendMode::Screen,
                        (|b, s| b + s - b * s) as fn(f32, f32) -> f32,
                    ),
                    (BlendMode::Multiply, |b, s| b * s),
                ] {
                    let got = crate::cpu::composite(s, d, mode);
                    let want = general(s, d, f);
                    for (g, w) in [
                        (got.r, want.r),
                        (got.g, want.g),
                        (got.b, want.b),
                        (got.a, want.a),
                    ] {
                        assert!(
                            (g - w).abs() < 1e-6,
                            "{mode:?} {s:?} {d:?}: {got:?} {want:?}"
                        );
                    }
                }
            }
        }
    }
}
