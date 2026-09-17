//! Measuring and painting the display list `geneva-html` produces.
//!
//! Layout asks two questions this crate can answer, how big a run of text
//! is and how big an image is, and then hands back boxes with
//! absolute coordinates. Painting them needs nothing the compositor does
//! not already do: a rounded rectangle is a signed distance field, and
//! text goes through the same engine every other text source uses.

use std::collections::{BTreeMap, HashMap};

use geneva_color::{Color, LinearRgba, Transfer};
use geneva_html::layout::Rectangle;
use geneva_html::style::{Extent, TextFill};
use geneva_html::{Content, Group, Laid, Measure, Painted, Prepared, Text};
use geneva_timeline::motion::{NodeMotion, Transform};
use geneva_timeline::schema::{Shadow, Shadows, TextAlign, TextSource, TextStyle};
use geneva_timeline::{Animated, FillTrack, ResolvedHtml, ResolvedText};

use crate::assets::Image;
use crate::fill::{Fill, encoded};
use crate::text::TextEngine;

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
) -> Result<Image, String> {
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
    let mut surface = paint(&laid, &transforms, context.text, images);
    to_linear(&mut surface);
    Ok(surface)
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
    let dec = |v: f32| Transfer::Srgb.to_linear(f64::from(v / p.a)) as f32 * p.a;
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

/// The painter's image back in linear light.
fn to_linear(image: &mut Image) {
    for p in &mut image.pixels {
        *p = decode_pixel(*p);
    }
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

/// How much of the surface a group's own picture spans (`own`, in its
/// own coordinates, taking in its children where they land) and where
/// that picture lands once blurred and transformed (`placed`). Groups
/// are listed parent before child, so a backward walk settles every
/// child before its parent.
fn group_bounds(laid: &Laid, transforms: &[Option<Transform>]) -> (GroupBounds, GroupBounds) {
    let n = laid.groups.len();
    let mut own: GroupBounds = vec![None; n];
    let mut placed: GroupBounds = vec![None; n];
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
    (own, placed)
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

/// Paints the display list in order. Boxes on the surface are painted
/// straight onto it; a group's boxes go into a buffer of its own, which
/// is composited into whatever holds the group when its last box is
/// done, under the group's opacity, blur, transform and outside clip.
fn paint(
    laid: &Laid,
    transforms: &[Option<Transform>],
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
) -> Image {
    let width = laid.size.0.ceil().max(1.0) as u32;
    let height = laid.size.1.ceil().max(1.0) as u32;
    let (own, placed) = group_bounds(laid, transforms);
    // What the surface can be marked in: its own boxes and the top-level
    // groups where they land. The compositor reads that rather than all
    // of it.
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
    let content = touched.map(|[x0, y0, x1, y1]| {
        let x = x0.floor().clamp(0.0, f64::from(width)) as u32;
        let y = y0.floor().clamp(0.0, f64::from(height)) as u32;
        let right = x1.ceil().clamp(0.0, f64::from(width)) as u32;
        let bottom = y1.ceil().clamp(0.0, f64::from(height)) as u32;
        [x, y, right.saturating_sub(x), bottom.saturating_sub(y)]
    });
    let mut surface = Image {
        width,
        height,
        pixels: vec![LinearRgba::TRANSPARENT; width as usize * height as usize],
        content,
    };
    let mut stack: Vec<Layer> = Vec::new();
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
            } else {
                let bounds = own[g].map(|r| padded(r, group.blur * 3.0));
                stack.push(Layer::over(g, bounds, (width, height)));
            }
        }
        if skipping.is_some() || b.opacity <= 0.0 {
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
    let clip = |px: f64, py: f64| -> f32 {
        group
            .clip
            .map_or(1.0, |(rect, radius)| coverage(px, py, rect, radius))
    };
    let src = &layer.image;
    let (sw, sh) = (i64::from(src.width), i64::from(src.height));
    let (dw, dh) = (i64::from(dst.width), i64::from(dst.height));
    let Some(tr) = tr else {
        // Straight on: pixel for pixel, offset by the two origins.
        let (ox, oy) = (layer.origin.0 - dst_origin.0, layer.origin.1 - dst_origin.1);
        for sy in 0..sh {
            let dy = sy + oy;
            if dy < 0 || dy >= dh {
                continue;
            }
            for sx in 0..sw {
                let dx = sx + ox;
                if dx < 0 || dx >= dw {
                    continue;
                }
                let texel = src.pixels[(sy * sw + sx) as usize];
                if texel.a <= 0.0 {
                    continue;
                }
                let (px, py) = (
                    (dx + dst_origin.0) as f64 + 0.5,
                    (dy + dst_origin.1) as f64 + 0.5,
                );
                over(dst, dx as usize, dy as usize, texel, opacity * clip(px, py));
            }
        }
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
    let taps = |scale: f64| ((1.0 / scale.abs().max(1e-3)).ceil() as usize).clamp(1, 4);
    let (nx, ny) = (taps(tr.scale[0]), taps(tr.scale[1]));
    let norm = 1.0 / (nx * ny) as f32;
    for dy in y0..y1 {
        for dx in x0..x1 {
            let mut sum = LinearRgba::TRANSPARENT;
            for j in 0..ny {
                for i in 0..nx {
                    let px = (dx + dst_origin.0) as f64 + (i as f64 + 0.5) / nx as f64;
                    let py = (dy + dst_origin.1) as f64 + (j as f64 + 0.5) / ny as f64;
                    let (lx, ly) = inverse(tr, centre, (px, py));
                    let (u, v) = (lx - layer.origin.0 as f64, ly - layer.origin.1 as f64);
                    if u < 0.0 || v < 0.0 || u >= sw as f64 || v >= sh as f64 {
                        continue;
                    }
                    let s = src.sample(u, v);
                    sum.r += s.r;
                    sum.g += s.g;
                    sum.b += s.b;
                    sum.a += s.a;
                }
            }
            if sum.a <= 0.0 {
                continue;
            }
            let texel = sum.scaled(norm);
            let (px, py) = (
                (dx + dst_origin.0) as f64 + 0.5,
                (dy + dst_origin.1) as f64 + 0.5,
            );
            over(dst, dx as usize, dy as usize, texel, opacity * clip(px, py));
        }
    }
}

/// Keeps what is inside the polygon, given in the surface's pixels, of a
/// buffer whose top-left pixel sits at `origin`. Coverage is measured on
/// four scanlines per row with exact horizontal overlap, and the fill
/// rule is nonzero, as CSS's is.
fn mask_polygon(image: &mut Image, origin: (i64, i64), points: &[(f64, f64)]) {
    const SUB: usize = 4;
    let (w, h) = (image.width as usize, image.height as usize);
    if points.len() < 3 {
        image.pixels.fill(LinearRgba::TRANSPARENT);
        return;
    }
    let mut coverage = vec![0f32; w];
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    for y in 0..h {
        coverage.fill(0.0);
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
            for (x, dir) in &crossings {
                let was = winding;
                winding += dir;
                if was == 0 && winding != 0 {
                    start = *x;
                } else if was != 0 && winding == 0 {
                    // A span across [start, x], in buffer pixels.
                    let a = (start - origin.0 as f64).clamp(0.0, w as f64);
                    let b = (x - origin.0 as f64).clamp(0.0, w as f64);
                    let (first, last) = (a.floor() as usize, b.ceil() as usize);
                    for (px, c) in coverage
                        .iter_mut()
                        .enumerate()
                        .take(last.min(w))
                        .skip(first)
                    {
                        let overlap = b.min(px as f64 + 1.0) - a.max(px as f64);
                        if overlap > 0.0 {
                            *c += overlap as f32 / SUB as f32;
                        }
                    }
                }
            }
        }
        let row = &mut image.pixels[y * w..(y + 1) * w];
        for (p, c) in row.iter_mut().zip(&coverage) {
            let c = c.clamp(0.0, 1.0);
            if c < 1.0 {
                *p = p.scaled(c);
            }
        }
    }
}

/// A premultiplied pixel over another, at a coverage.
fn over(dst: &mut Image, x: usize, y: usize, texel: LinearRgba, a: f32) {
    if a <= 0.0 {
        return;
    }
    let i = y * dst.width as usize + x;
    let d = dst.pixels[i];
    let inv = 1.0 - texel.a * a;
    dst.pixels[i] = LinearRgba {
        r: texel.r * a + d.r * inv,
        g: texel.g * a + d.g * inv,
        b: texel.b * a + d.b * inv,
        a: texel.a * a + d.a * inv,
    };
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

fn blend(image: &mut Image, x: u32, y: u32, color: LinearRgba, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let src = LinearRgba {
        r: color.r * alpha,
        g: color.g * alpha,
        b: color.b * alpha,
        a: color.a * alpha,
    };
    let i = (y as usize) * image.width as usize + x as usize;
    let dst = image.pixels[i];
    let inv = 1.0 - src.a;
    image.pixels[i] = LinearRgba {
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
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let outer = coverage(px, py, b.rect, b.paint.radius);
            if outer <= 0.0 {
                continue;
            }
            let clip = clip_coverage(b, px, py);
            if clip <= 0.0 {
                continue;
            }
            let hole = coverage(px, py, inner_rect, inner_radius);
            if let Some(fill) = &fill {
                blend(image, x, y, fill.at(px, py), outer * clip * b.opacity);
            }
            if has_border && outer > hole {
                let colour = b.paint.border_color[side(b, px, py)];
                if colour.a > 0.0 {
                    blend(
                        image,
                        x,
                        y,
                        encoded(colour),
                        (outer - hole) * clip * b.opacity,
                    );
                }
            }
        }
    }
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
    if shadow.blur <= 0.0 {
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                let a = coverage(px, py, rect, b.paint.radius) * clip_coverage(b, px, py);
                blend(image, x, y, encoded(shadow.color), a * b.opacity);
            }
        }
        return;
    }
    // A blurred shadow is the same shape with its distance field softened
    // over the blur, which matches a Gaussian closely enough at these
    // radii and costs one pass instead of three.
    let sigma = shadow.blur / 2.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let d = distance(px, py, rect, b.paint.radius);
            let a = (0.5 - d / (2.0 * sigma)).clamp(0.0, 1.0) as f32;
            if a > 0.0 {
                blend(
                    image,
                    x,
                    y,
                    encoded(shadow.color),
                    a * clip_coverage(b, px, py) * b.opacity,
                );
            }
        }
    }
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
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let clip = clip_coverage(b, px, py);
            if clip <= 0.0 {
                continue;
            }
            let u = (px - f64::from(rect[0])) * sx;
            let v = (py - f64::from(rect[1])) * sy;
            let texel = source.sample(u, v);
            // The sampled pixel is already premultiplied.
            let a = b.opacity * clip;
            let i = (y as usize) * image.width as usize + x as usize;
            let dst = image.pixels[i];
            let inv = 1.0 - texel.a * a;
            image.pixels[i] = LinearRgba {
                r: texel.r * a + dst.r * inv,
                g: texel.g * a + dst.g * inv,
                b: texel.b * a + dst.b * inv,
                a: texel.a * a + dst.a * inv,
            };
        }
    }
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
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
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
            let a = b.opacity * clip;
            let i = (y as usize) * image.width as usize + x as usize;
            let dst = image.pixels[i];
            let inv = 1.0 - texel.a * a;
            image.pixels[i] = LinearRgba {
                r: texel.r * a + dst.r * inv,
                g: texel.g * a + dst.g * inv,
                b: texel.b * a + dst.b * inv,
                a: texel.a * a + dst.a * inv,
            };
        }
    }
}

/// A colour with full alpha, for the places a `Color` is needed.
#[allow(dead_code)]
fn opaque(c: Color) -> Color {
    Color { a: 1.0, ..c }
}
