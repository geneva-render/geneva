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
use geneva_timeline::schema::{BlendMode, Shadow, Shadows, TextAlign, TextSource, TextStyle};
use geneva_timeline::{Animated, FillTrack, ResolvedHtml, ResolvedText};
use rayon::prelude::*;

use crate::assets::Image;
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
    /// measured at; flex asks the same question several times.
    memo: HashMap<(String, String, u32), (f32, f32)>,
}

/// Turns an HTML text style into the text source the engine draws, so
/// markup takes the same shaping, fallback and colour path as a text clip.
fn as_text_source(text: &str, style: &Text, max_width: f64) -> ResolvedText {
    ResolvedText::constant(
        text.to_owned(),
        TextSource {
            text: Some(text.to_owned()),
            words: None,
            highlight: None,
            style: TextStyle {
                font: style.family.clone(),
                size: Some(style.size),
                weight: Some(style.weight),
                italic: Some(style.italic),
                color: Some(Animated::Constant(style.color.into())),
                fill: None,
                letter_spacing: Some(style.letter_spacing),
            },
            max_width: None,
            align: Some(match style.align {
                geneva_html::TextAlign::Left => TextAlign::Left,
                geneva_html::TextAlign::Center => TextAlign::Center,
                geneva_html::TextAlign::Right => TextAlign::Right,
            }),
            line_height: Some(style.line_height),
            padding: None,
            background: None,
            radius: None,
            outline: None,
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
    )
}

/// A key that distinguishes two runs with different styles.
fn style_key(style: &Text) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{:?}",
        style.family.as_deref().unwrap_or(""),
        style.size,
        style.weight,
        style.italic,
        style.line_height,
        style.letter_spacing,
        style.align
    )
}

impl Measure for Context<'_> {
    fn text(&mut self, text: &str, style: &Text, width: Option<f32>) -> (f32, f32) {
        if text.trim().is_empty() {
            return (0.0, 0.0);
        }
        // A run with no limit is measured at a width nothing will reach,
        // which is what max-content means here.
        let limit = width.filter(|w| *w > 0.0).unwrap_or(1.0e5);
        let key = (text.to_owned(), style_key(style), limit.to_bits());
        if let Some(size) = self.memo.get(&key) {
            return *size;
        }
        // A shadow pads the rendered image, so measuring with one would
        // move the text it is drawn behind. CSS lays text out as though
        // the shadow were not there, and so does this.
        let mut plain = style.clone();
        plain.shadow.clear();
        let source = as_text_source(text, &plain, f64::from(limit));
        let image = self.text.render(&source, 0.0);
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
) -> Result<Image, String> {
    let (laid, transforms) = lay_out(html, prepared, text, images, t)?;
    let mut surface = paint(&laid, &transforms, text, images, cache);
    to_linear(&mut surface);
    Ok(surface)
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
) -> Result<(Laid, Vec<Option<Transform>>), String> {
    let mut context = Context {
        text,
        images,
        memo: HashMap::new(),
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
    let laid = prepared.layout_with(
        html.width.map(|w| w as f32),
        html.height.map(|h| h as f32),
        &mut context,
        &overrides,
    )?;
    // A transform is sampled against the box the element settled on,
    // which a percentage in a translation is a share of.
    let transforms: Vec<Option<Transform>> = laid
        .groups
        .iter()
        .map(|g| {
            let m = motions.get(&g.node)?;
            let size = (f64::from(g.rect[2]), f64::from(g.rect[3]));
            m.sample(t, &prepared.styles[g.node], size)
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
fn encode_pixel(p: LinearRgba) -> LinearRgba {
    if p.a <= 0.0 {
        return LinearRgba::TRANSPARENT;
    }
    let enc = |v: f32| Transfer::Srgb.from_linear(f64::from(v / p.a)) as f32 * p.a;
    LinearRgba {
        r: enc(p.r),
        g: enc(p.g),
        b: enc(p.b),
        a: p.a,
    }
}

fn decode_pixel(p: LinearRgba) -> LinearRgba {
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

/// The painter's image back in linear light.
fn to_linear(image: &mut Image) {
    image.pixels.par_chunks_mut(1 << 12).for_each(|part| {
        for p in part {
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
    let text_shadow = match &painted.content {
        Content::Text { style, .. } => furthest(&style.shadow),
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

/// A group's rectangle after its transform, about the centre of its box.
fn transformed(r: Bounds, centre: (f64, f64), tr: &Transform) -> Bounds {
    let mut out: Option<Bounds> = None;
    for (x, y) in [(r[0], r[1]), (r[2], r[1]), (r[0], r[3]), (r[2], r[3])] {
        let (px, py) = forward(tr, centre, (x, y));
        out = Some(union(out, [px, py, px, py]));
    }
    out.unwrap_or(r)
}

/// Where a point of the group lands: scaled and turned about the centre,
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
        if let Some(tr) = &transforms[g] {
            r = transformed(r, centre_of(group.rect), tr);
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
        let visible = window.and_then(|w| match &transforms[g] {
            Some(tr) if tr.scale[0] == 0.0 || tr.scale[1] == 0.0 => None,
            Some(tr) => Some(padded(inverted(w, centre_of(group.rect), tr), reach)),
            None => Some(padded(w, reach)),
        });
        natural[g] = Some(padded(r, reach));
        own[g] = visible.and_then(|v| intersect(padded(r, reach), v));
    }
    (own, placed, natural)
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

fn centre_of(rect: Rectangle) -> (f64, f64) {
    (
        f64::from(rect[0]) + f64::from(rect[2]) / 2.0,
        f64::from(rect[1]) + f64::from(rect[3]) / 2.0,
    )
}

/// A group's buffer while its boxes are painted. `origin` is where its
/// top-left pixel sits on the surface.
struct Layer {
    group: usize,
    image: Image,
    origin: (i64, i64),
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
            };
        };
        // A group can reach past the surface and be moved back onto it,
        // so its buffer is not clamped to the surface; it is bounded so a
        // runaway scale cannot ask for the world.
        let (w, h) = (f64::from(surface.0), f64::from(surface.1));
        let x0 = b[0].floor().clamp(-w, 2.0 * w) as i64;
        let y0 = b[1].floor().clamp(-h, 2.0 * h) as i64;
        let x1 = b[2].ceil().clamp(-w, 2.0 * w) as i64;
        let y1 = b[3].ceil().clamp(-h, 2.0 * h) as i64;
        let width = (x1 - x0).max(0) as u32;
        let height = (y1 - y0).max(0) as u32;
        Self {
            group,
            image: Image {
                width,
                height,
                pixels: vec![LinearRgba::TRANSPARENT; width as usize * height as usize],
                content: None,
            },
            origin: (x0, y0),
        }
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
    /// the next by it.
    generation: u64,
}

impl Cached {
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
        let width = (want[2] - want[0]).max(0) as u32;
        let height = (want[3] - want[1]).max(0) as u32;
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        let stride = self.image.width as usize;
        for y in 0..i64::from(height) {
            let row = (want[1] + y - self.origin.1) as usize * stride;
            let from = row + (want[0] - self.origin.0) as usize;
            pixels.extend_from_slice(&self.image.pixels[from..from + width as usize]);
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
        }
    }
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
    entries: HashMap<usize, Cached>,
    /// Misses in a row, per group, and past [`STRIKES`] the group is
    /// not painted into the cache again.
    give_up: HashMap<usize, u8>,
    bytes: usize,
    /// Paintings so far, so that each picture kept has a number of its
    /// own.
    generation: u64,
}

/// Pictures below this many pixels are not worth keeping: the copy out
/// of the cache would cost about what painting them does.
const MIN_PIXELS: i64 = 64 * 64;

/// What one markup source may hold. Each thread rendering frames has a
/// cache of its own, so this is what one costs.
const BUDGET: usize = 96 << 20;

/// How many times a group may be painted without its picture ever being
/// used again before it is given up on. An element whose own boxes
/// animate, a letter changing colour, never reuses one, and painting it
/// a size larger than the frame needs is worse than not trying.
const STRIKES: u8 = 3;

impl GroupCache {
    /// Forgets everything, freeing the pictures.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.give_up.clear();
        self.bytes = 0;
    }

    fn insert(&mut self, group: usize, mut entry: Cached) {
        if let Some(old) = self.entries.remove(&group) {
            self.bytes -= old.image.pixels.len() * size_of::<LinearRgba>();
        }
        self.bytes += entry.image.pixels.len() * size_of::<LinearRgba>();
        entry.generation = self.generation;
        self.generation += 1;
        self.entries.insert(group, entry);
    }
}

/// The integer buffer a group needs, as [`Layer::over`] would bound it.
fn buffer_rect(bounds: Option<Bounds>, surface: (u32, u32)) -> Option<[i64; 4]> {
    let b = bounds?;
    let (w, h) = (f64::from(surface.0), f64::from(surface.1));
    let r = [
        b[0].floor().clamp(-w, 2.0 * w) as i64,
        b[1].floor().clamp(-h, 2.0 * h) as i64,
        b[2].ceil().clamp(-w, 2.0 * w) as i64,
        b[3].ceil().clamp(-h, 2.0 * h) as i64,
    ];
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
/// returns which ones the caller may take from the cache.
fn fill_cache(
    cache: &mut GroupCache,
    laid: &Laid,
    own: &[Option<Bounds>],
    natural: &[Option<Bounds>],
    surface: (u32, u32),
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
) -> HashSet<usize> {
    let mut served = HashSet::new();
    for g in 0..laid.groups.len() {
        let Some(want) = buffer_rect(own[g], surface) else {
            continue;
        };
        if (want[2] - want[0]) * (want[3] - want[1]) < MIN_PIXELS {
            continue;
        }
        if !is_leaf(laid, g) {
            continue;
        }
        let boxes: Vec<Painted> = laid
            .boxes
            .iter()
            .filter(|b| b.group == Some(g) && b.opacity > 0.0)
            .cloned()
            .collect();
        if boxes.is_empty() {
            continue;
        }
        // Only the boxes are compared. A group's own opacity, blur and
        // clips are read when its picture is composited rather than when
        // it is painted, so they change nothing in the picture, and its
        // transform is the whole point: the picture stays put and the
        // transform moves it.
        let fresh = cache
            .entries
            .get(&g)
            .is_some_and(|c| c.covers(want) && c.boxes == boxes);
        if !fresh {
            let strikes = cache.give_up.entry(g).or_default();
            if *strikes >= STRIKES {
                continue;
            }
            *strikes += 1;
            // The whole of what the group covers, not just the window
            // this frame needs. The window slides and shrinks as the
            // element drifts and scales, so one that fits the frame
            // would be wrong by the next; painting the lot once costs
            // about three of those and then never again.
            let Some(region) = natural[g] else {
                continue;
            };
            let mut layer = Layer::over(g, Some(region), surface);
            let bytes = layer.image.pixels.len() * size_of::<LinearRgba>();
            if cache.bytes + bytes > BUDGET {
                continue;
            }
            for b in &boxes {
                let moved = shifted(b, layer.origin);
                paint_one(&mut layer.image, &moved, text, images);
            }
            let entry = Cached {
                boxes,
                image: layer.image,
                origin: layer.origin,
                generation: 0,
            };
            // The clamp in `Layer::over` can cut a picture that reaches
            // far past the surface down to less than the frame asks for.
            if !entry.covers(want) {
                continue;
            }
            cache.insert(g, entry);
        }
        cache.give_up.remove(&g);
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
fn paint(
    laid: &Laid,
    transforms: &[Option<Transform>],
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
    cache: &mut GroupCache,
) -> Image {
    let width = laid.size.0.ceil().max(1.0) as u32;
    let height = laid.size.1.ceil().max(1.0) as u32;
    let (own, placed, natural) =
        group_bounds(laid, transforms, (f64::from(width), f64::from(height)));
    let content = content_of(laid, &placed, (width, height));
    let mut surface = Image {
        width,
        height,
        pixels: vec![LinearRgba::TRANSPARENT; width as usize * height as usize],
        content,
    };
    let served = fill_cache(cache, laid, &own, &natural, (width, height), text, images);
    let mut stack: Vec<Layer> = Vec::new();
    // A group taken from the cache: its own boxes are already in the
    // picture, so they are not painted again.
    let mut serving: Option<usize> = None;
    // A group that would not show (no opacity, no size, nothing in it)
    // is skipped whole, boxes and inner groups alike.
    let mut skipping: Option<usize> = None;
    for b in &laid.boxes {
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
            close(layer, laid, transforms, &placed, &mut stack, &mut surface);
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
                .and_then(|want| cache.entries.get(&g).map(|c| c.window(g, want)));
            match ready {
                Some(layer) => {
                    serving = Some(g);
                    stack.push(layer);
                }
                None => stack.push(Layer::over(g, own[g], (width, height))),
            }
        }
        if skipping.is_some() || serving.is_some() || b.opacity <= 0.0 {
            continue;
        }
        match stack.last_mut() {
            Some(layer) => {
                let moved = shifted(b, layer.origin);
                paint_one(&mut layer.image, &moved, text, images);
            }
            None => paint_one(&mut surface, b, text, images),
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
        close(layer, laid, transforms, &placed, &mut stack, &mut surface);
    }
    surface
}

/// Composites a finished group into the layer under it, or the surface.
fn close(
    layer: Layer,
    laid: &Laid,
    transforms: &[Option<Transform>],
    placed: &[Option<Bounds>],
    stack: &mut [Layer],
    surface: &mut Image,
) {
    let group = &laid.groups[layer.group];
    let tr = transforms[layer.group].as_ref();
    let landing = placed[layer.group];
    match stack.last_mut() {
        Some(parent) => {
            let origin = parent.origin;
            composite(&mut parent.image, origin, layer, group, tr, landing);
        }
        None => composite(surface, (0, 0), layer, group, tr, landing),
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
        Content::Text { text: run, style } => paint_text(image, b, run, style, text),
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
    if group.blur > 0.0 {
        let (w, h) = (layer.image.width as usize, layer.image.height as usize);
        crate::blur::blur_pixels(&mut layer.image.pixels, w, h, group.blur);
    }
    // CSS clips after filtering and before the transform, so the polygon
    // is applied in the buffer, where the group's own pixels are.
    if let Some(points) = &group.clip_path {
        mask_polygon(&mut layer.image, layer.origin, points);
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
    // Where the clip covers a pixel whole, which is everywhere but a
    // half-pixel band at its edge and its rounded corners. Inside this
    // rectangle the coverage is exactly one, so the square root and the
    // branches in `distance` are skipped, which is most of the pixels
    // of a full-frame group.
    let solid = group.clip.map(|(rect, radius)| {
        let inset = radius.iter().fold(0.5f64, |a, r| a.max(*r));
        [
            f64::from(rect[0]) + inset,
            f64::from(rect[1]) + inset,
            f64::from(rect[0] + rect[2]) - inset,
            f64::from(rect[1] + rect[3]) - inset,
        ]
    });
    let clip = |px: f64, py: f64| -> f32 {
        match group.clip {
            None => 1.0,
            Some((rect, radius)) => {
                if let Some(s) = solid
                    && px >= s[0]
                    && py >= s[1]
                    && px <= s[2]
                    && py <= s[3]
                {
                    return 1.0;
                }
                coverage(px, py, rect, radius)
            }
        }
    };
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
    let centre = centre_of(group.rect);
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
    let (y0, y1) = (
        y0.max(0) as usize,
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

/// The clip an ancestor imposes, as coverage.
fn clip_coverage(b: &Painted, px: f64, py: f64) -> f32 {
    b.clip
        .map_or(1.0, |(rect, radius)| coverage(px, py, rect, radius))
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
    let (x0, y0, x1, y1) = bounds(image, b.rect, 1.0);
    rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in x0..x1 {
            let px = f64::from(x) + 0.5;
            let outer = coverage(px, py, b.rect, b.paint.radius);
            if outer <= 0.0 {
                continue;
            }
            let clip = clip_coverage(b, px, py);
            if clip <= 0.0 {
                continue;
            }
            if let Some(fill) = &fill {
                blend(row, x as usize, fill.at(px, py), outer * clip * b.opacity);
            }
            if !has_border {
                continue;
            }
            let hole = coverage(px, py, inner_rect, inner_radius);
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
    if shadow.blur <= 0.0 {
        rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
            let py = y as f64 + 0.5;
            for x in x0..x1 {
                let px = f64::from(x) + 0.5;
                let a = coverage(px, py, rect, b.paint.radius) * clip_coverage(b, px, py);
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
                    a * clip_coverage(b, px, py) * b.opacity,
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

fn paint_text(image: &mut Image, b: &Painted, run: &str, style: &Text, engine: &mut TextEngine) {
    if run.trim().is_empty() {
        return;
    }
    let box_width = f64::from(b.content_rect[2]).max(1.0);
    let mut source = as_text_source(run, style, box_width);
    let inset = crate::text::inset_for(&source);
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
            _ => shift(f64::from(engine.render(&source, 0.0).width)),
        };
        source.fill = Some(fill_track(fill, b.content_rect, along));
    }
    // The engine draws in linear light; the page it lands on is not.
    let drawn = to_encoded(engine.render(&source, 0.0));
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
    rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in x0..x1 {
            let px = f64::from(x) + 0.5;
            let clip = clip_coverage(b, px, py);
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
    rows(image, i64::from(y0), i64::from(y1)).for_each(|(y, row)| {
        let py = y as f64 + 0.5;
        for x in x0..x1 {
            let px = f64::from(x) + 0.5;
            let clip = clip_coverage(b, px, py);
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
