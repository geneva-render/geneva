//! A markup box as layers rather than as a picture, for a renderer that
//! composites them itself.
//!
//! [`super::render`] paints the boxes and composites the groups on the
//! CPU and hands back the finished picture. [`render_layers`] walks the
//! same display list with the same caches, paints the boxes on the CPU
//! as before, and stops there: what comes back is the runs of painted
//! pixels and the groups they sit in, each with its opacity, blur, clip
//! and transform still to apply. The GPU renderer composites those on
//! the device, where a transform is a texture sample rather than a loop.
//! [`MarkupLayers::flatten`] composites them on the CPU through the
//! same code the painter uses, so the two can be held to each other.
//!
//! Everything here is in the painter's working space: sRGB-encoded
//! channels premultiplied by alpha. Whoever composites the layers turns
//! the finished box into linear light afterwards, once.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use geneva_color::LinearRgba;
use geneva_html::layout::Rectangle;
use geneva_html::{Group, Laid, Prepared};
use geneva_timeline::ResolvedHtml;
use geneva_timeline::motion::Transform;

use super::{
    Bounds, GroupCache, Layer, affine_inverse, buffer_rect, centre_of, chain_of, composite,
    content_of, fill_cache, group_bounds, lay_out, paint_one, polygon_coverage, shifted, taps_of,
    to_linear,
};
use crate::assets::Image;
use crate::text::TextEngine;

/// A markup box at one time, as the layers that make it: painted runs
/// and the groups they sit in, in paint order, over a transparent
/// surface of `width` by `height`.
#[derive(Debug)]
pub struct MarkupLayers<'a> {
    /// The surface's width.
    pub width: u32,
    /// The surface's height.
    pub height: u32,
    /// The sub-rectangle `[x, y, w, h]` of the surface outside which
    /// nothing is drawn, when it is known.
    pub content: Option<[u32; 4]>,
    /// What lies on the surface, in paint order.
    pub items: Vec<MarkupItem<'a>>,
}

/// One thing laid on a surface or into a group's buffer.
#[derive(Debug)]
pub enum MarkupItem<'a> {
    /// Pixels painted on the CPU, laid straight, pixel for pixel.
    Run(MarkupRun<'a>),
    /// A group: its own items composited into a buffer of its own and
    /// laid under its opacity, blur, clips and transform.
    Group(MarkupGroup<'a>),
}

/// Painted pixels in the painter's working space, laid straight.
#[derive(Debug)]
pub struct MarkupRun<'a> {
    /// The pixels. A picture kept between frames is borrowed from the
    /// cache and may be larger than the part laid.
    pub image: Cow<'a, Image>,
    /// The part of `image` that is laid: `[x, y, w, h]` in its pixels.
    pub window: [u32; 4],
    /// Where the window's top-left pixel sits on the surface.
    pub origin: (i64, i64),
    /// The identity of `image` when it is one the painter keeps between
    /// frames: two runs with the same key carry the same pixels, so a
    /// renderer that copies pictures somewhere (a texture) can keep the
    /// copy by the key. A run without a key is painted for this frame.
    pub key: Option<u64>,
}

/// A group's buffer and how it is laid into what holds it.
#[derive(Debug)]
pub struct MarkupGroup<'a> {
    /// The buffer's edges on the surface: left, top, right, bottom, in
    /// whole pixels. Its items are composited into it with its top-left
    /// pixel as their origin.
    pub buffer: [i64; 4],
    /// What is in the buffer, in paint order.
    pub items: Vec<MarkupItem<'a>>,
    /// The element's border box on the surface, which the transform
    /// turns about.
    pub rect: Rectangle,
    /// The group's own opacity, applied when it is laid.
    pub opacity: f32,
    /// `filter: blur()` in pixels, applied to the buffer before the
    /// clips and the transform, pixels beyond the buffer counting as
    /// transparent.
    pub blur: f64,
    /// The rounded rectangle an ancestor outside the group clips it to,
    /// on the surface, as coverage at each pixel the group lands on.
    pub clip: Option<(Rectangle, [f64; 4])>,
    /// `clip-path: polygon()` on the surface, applied to the buffer's
    /// pixels after the blur and before the transform.
    pub clip_path: Option<Vec<(f64, f64)>>,
    /// The transform the buffer is laid through, about the centre of
    /// `rect`; straight, pixel for pixel, when there is none.
    pub transform: Option<Transform>,
    /// Where the transformed buffer lands on the surface, as its left,
    /// top, right and bottom; read only with a transform.
    pub landing: Option<[f64; 4]>,
}

impl MarkupGroup<'_> {
    /// Where the buffer's top-left pixel sits on the surface.
    pub fn origin(&self) -> (i64, i64) {
        (self.buffer[0], self.buffer[1])
    }

    /// The buffer's width and height.
    pub fn size(&self) -> (u32, u32) {
        (
            (self.buffer[2] - self.buffer[0]).max(0) as u32,
            (self.buffer[3] - self.buffer[1]).max(0) as u32,
        )
    }

    /// The point the transform turns about.
    pub fn centre(&self) -> (f64, f64) {
        centre_of(self.rect)
    }

    /// The pixels of the surface the group is laid on, as left, top,
    /// right and bottom in whole pixels: the buffer's own when it is
    /// laid straight, the landing's otherwise. `None` when nothing is
    /// laid.
    pub fn bounds(&self) -> Option<[i64; 4]> {
        let (w, h) = self.size();
        if w == 0 || h == 0 {
            return None;
        }
        match &self.transform {
            None => Some(self.buffer),
            Some(_) => {
                let l = self.landing?;
                let b = [
                    l[0].floor() as i64,
                    l[1].floor() as i64,
                    l[2].ceil() as i64,
                    l[3].ceil() as i64,
                ];
                (b[2] > b[0] && b[3] > b[1]).then_some(b)
            }
        }
    }

    /// The transform undone, as the six constants of two linear
    /// expressions in a surface point `(px, py)`:
    /// `u = ax * px + bx * py + cx` and `v = ay * px + by * py + cy`, in
    /// the buffer's own pixels, returned as `[ax, bx, cx, ay, by, cy]`.
    /// The buffer laid straight maps a surface point to itself less the
    /// buffer's origin.
    pub fn inverse(&self) -> [f64; 6] {
        let origin = self.origin();
        match &self.transform {
            Some(tr) => affine_inverse(tr, self.centre(), origin),
            None => [1.0, 0.0, -(origin.0 as f64), 0.0, 1.0, -(origin.1 as f64)],
        }
    }

    /// How many samples a pixel of the surface takes along each axis:
    /// one, or up to four each way for a buffer drawn smaller than it
    /// is.
    pub fn taps(&self) -> (u32, u32) {
        match &self.transform {
            Some(tr) => {
                let (nx, ny) = taps_of(tr);
                (nx as u32, ny as u32)
            }
            None => (1, 1),
        }
    }

    /// The polygon clip as coverage over the buffer's pixels, row-major,
    /// or `None` without one.
    pub fn mask(&self) -> Option<Vec<f32>> {
        let points = self.clip_path.as_ref()?;
        let (w, h) = self.size();
        Some(polygon_coverage(
            w as usize,
            h as usize,
            self.origin(),
            points,
        ))
    }
}

/// Lays out an HTML source at `t` seconds into its clip and paints its
/// boxes, as [`super::render`] does, but leaves the groups to whoever
/// composites the layers. Pictures of groups that do not change are
/// taken from `cache` and carry a key made with `salt`, so that keys
/// of different sources never meet.
pub fn render_layers<'c>(
    html: &ResolvedHtml,
    prepared: &Prepared,
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
    t: f64,
    cache: &'c mut GroupCache,
    salt: u64,
) -> Result<MarkupLayers<'c>, String> {
    let (laid, transforms) = lay_out(html, prepared, text, images, t)?;
    let width = laid.size.0.ceil().max(1.0) as u32;
    let height = laid.size.1.ceil().max(1.0) as u32;
    let (own, placed, natural) =
        group_bounds(&laid, &transforms, (f64::from(width), f64::from(height)));
    let content = content_of(&laid, &placed, (width, height));
    let served = fill_cache(cache, &laid, &own, &natural, (width, height), text, images);
    // Painted, and read from here on.
    let cache: &'c GroupCache = cache;
    let mut root = Level::over(Some([0, 0, i64::from(width), i64::from(height)]));
    let mut stack: Vec<(usize, Level<'c>)> = Vec::new();
    // A group taken from the cache: its picture is its one run, so its
    // boxes are not painted again.
    let mut serving: Option<usize> = None;
    // A group that would not show (no opacity, no size, nothing in it)
    // is skipped whole, boxes and inner groups alike.
    let mut skipping: Option<usize> = None;
    for b in &laid.boxes {
        let chain = chain_of(&laid, b.group);
        while let Some((top, _)) = stack.last() {
            if chain.contains(top) {
                break;
            }
            let (g, level) = stack.pop().expect("checked above");
            if skipping == Some(g) {
                skipping = None;
                continue;
            }
            if serving == Some(g) {
                serving = None;
            }
            let parent = stack.last_mut().map_or(&mut root, |(_, l)| l);
            close(g, level, &laid, &transforms, &placed, parent);
        }
        for g in chain {
            if stack.iter().any(|(open, _)| *open == g) {
                continue;
            }
            // A group opening ends the run its parent was painting.
            stack.last_mut().map_or(&mut root, |(_, l)| l).flush();
            let group = &laid.groups[g];
            let flat = transforms[g]
                .as_ref()
                .is_some_and(|tr| tr.scale[0] == 0.0 || tr.scale[1] == 0.0);
            let hidden = skipping.is_some() || group.opacity <= 0.0 || flat || own[g].is_none();
            if hidden {
                skipping.get_or_insert(g);
                stack.push((g, Level::over(None)));
                continue;
            }
            let want = buffer_rect(own[g], (width, height));
            let ready = (serving.is_none() && served.contains(&g))
                .then_some(want)
                .flatten()
                .and_then(|want| cache.entries.get(&g).map(|c| (want, c)));
            match ready {
                Some((want, c)) => {
                    serving = Some(g);
                    let mut level = Level::over(Some(want));
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    "markup-group".hash(&mut hasher);
                    salt.hash(&mut hasher);
                    g.hash(&mut hasher);
                    c.generation.hash(&mut hasher);
                    level.items.push(MarkupItem::Run(MarkupRun {
                        image: Cow::Borrowed(&c.image),
                        window: [
                            (want[0] - c.origin.0) as u32,
                            (want[1] - c.origin.1) as u32,
                            (want[2] - want[0]) as u32,
                            (want[3] - want[1]) as u32,
                        ],
                        origin: (want[0], want[1]),
                        key: Some(hasher.finish()),
                    }));
                    stack.push((g, level));
                }
                None => stack.push((g, Level::over(want))),
            }
        }
        if skipping.is_some() || serving.is_some() || b.opacity <= 0.0 {
            continue;
        }
        let level = stack.last_mut().map_or(&mut root, |(_, l)| l);
        level.paint(b, text, images);
    }
    while let Some((g, level)) = stack.pop() {
        if skipping == Some(g) {
            skipping = None;
            continue;
        }
        if serving == Some(g) {
            serving = None;
        }
        let parent = stack.last_mut().map_or(&mut root, |(_, l)| l);
        close(g, level, &laid, &transforms, &placed, parent);
    }
    root.flush();
    Ok(MarkupLayers {
        width,
        height,
        content,
        items: root.items,
    })
}

/// The surface, or a group's buffer, while its items are gathered.
struct Level<'a> {
    /// Its edges on the surface, or nothing for a group that would not
    /// show, whose boxes are dropped.
    bounds: Option<[i64; 4]>,
    items: Vec<MarkupItem<'a>>,
    /// The run being painted, and where its top-left pixel sits on the
    /// surface.
    run: Option<(Image, (i64, i64))>,
}

impl Level<'_> {
    fn over(bounds: Option<[i64; 4]>) -> Self {
        Self {
            bounds,
            items: Vec::new(),
            run: None,
        }
    }

    /// Paints one box into the run in progress, starting one over the
    /// whole level when there is none. The run is the level's size
    /// rather than the boxes' reach, so that what a box paints past its
    /// reach (a glyph wider than the box it is typed into) is cut where
    /// the painter cuts it, at the buffer's edge.
    fn paint(
        &mut self,
        b: &geneva_html::Painted,
        text: &mut TextEngine,
        images: &HashMap<String, Image>,
    ) {
        let Some(bounds) = self.bounds else {
            return;
        };
        if self.run.is_none() {
            let width = (bounds[2] - bounds[0]).max(0) as u32;
            let height = (bounds[3] - bounds[1]).max(0) as u32;
            if width == 0 || height == 0 {
                return;
            }
            self.run = Some((
                Image {
                    width,
                    height,
                    pixels: vec![LinearRgba::TRANSPARENT; width as usize * height as usize],
                    content: None,
                },
                (bounds[0], bounds[1]),
            ));
        }
        let (image, origin) = self.run.as_mut().expect("started above");
        let moved = shifted(b, *origin);
        paint_one(image, &moved, text, images);
    }

    /// Ends the run in progress, keeping it as an item.
    fn flush(&mut self) {
        if let Some((image, origin)) = self.run.take() {
            let window = [0, 0, image.width, image.height];
            self.items.push(MarkupItem::Run(MarkupRun {
                image: Cow::Owned(image),
                window,
                origin,
                key: None,
            }));
        }
    }
}

/// Ends a group, keeping it as an item of `parent` with everything its
/// composite needs.
fn close<'a>(
    g: usize,
    mut level: Level<'a>,
    laid: &Laid,
    transforms: &[Option<Transform>],
    placed: &[Option<Bounds>],
    parent: &mut Level<'a>,
) {
    level.flush();
    let Some(buffer) = level.bounds else {
        return;
    };
    if level.items.is_empty() {
        return;
    }
    let group = &laid.groups[g];
    parent.items.push(MarkupItem::Group(MarkupGroup {
        buffer,
        items: level.items,
        rect: group.rect,
        opacity: group.opacity,
        blur: group.blur,
        clip: group.clip,
        clip_path: group.clip_path.clone(),
        transform: transforms[g],
        landing: placed[g],
    }));
}

impl MarkupLayers<'_> {
    /// The layers composited on the CPU, through the painter's own
    /// composite, and turned into linear light: the picture
    /// [`super::render`] paints, to rounding. What the GPU's composite
    /// of the same layers is held to.
    pub fn flatten(&self) -> Image {
        let mut surface = Image {
            width: self.width,
            height: self.height,
            pixels: vec![LinearRgba::TRANSPARENT; self.width as usize * self.height as usize],
            content: self.content,
        };
        flatten_into(&self.items, &mut surface, (0, 0));
        to_linear(&mut surface);
        surface
    }
}

/// Composites `items` into `dst`, whose top-left pixel sits at
/// `dst_origin` on the surface.
fn flatten_into(items: &[MarkupItem<'_>], dst: &mut Image, dst_origin: (i64, i64)) {
    for item in items {
        match item {
            MarkupItem::Run(run) => {
                let [x, y, w, h] = run.window;
                let stride = run.image.width as usize;
                let mut pixels = Vec::with_capacity(w as usize * h as usize);
                for row in y..y + h {
                    let from = row as usize * stride + x as usize;
                    pixels.extend_from_slice(&run.image.pixels[from..from + w as usize]);
                }
                let layer = Layer {
                    group: 0,
                    image: Image {
                        width: w,
                        height: h,
                        pixels,
                        content: None,
                    },
                    origin: run.origin,
                };
                let straight = Group {
                    node: 0,
                    parent: None,
                    rect: [0.0; 4],
                    opacity: 1.0,
                    blur: 0.0,
                    clip: None,
                    clip_path: None,
                };
                composite(dst, dst_origin, layer, &straight, None, None);
            }
            MarkupItem::Group(group) => {
                let (w, h) = group.size();
                let mut buffer = Image {
                    width: w,
                    height: h,
                    pixels: vec![LinearRgba::TRANSPARENT; w as usize * h as usize],
                    content: None,
                };
                flatten_into(&group.items, &mut buffer, group.origin());
                let layer = Layer {
                    group: 0,
                    image: buffer,
                    origin: group.origin(),
                };
                let as_group = Group {
                    node: 0,
                    parent: None,
                    rect: group.rect,
                    opacity: group.opacity,
                    blur: group.blur,
                    clip: group.clip,
                    clip_path: group.clip_path.clone(),
                };
                composite(
                    dst,
                    dst_origin,
                    layer,
                    &as_group,
                    group.transform.as_ref(),
                    group.landing,
                );
            }
        }
    }
}
