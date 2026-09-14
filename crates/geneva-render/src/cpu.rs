use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use geneva_color::{Color, LinearRgba};
use geneva_timeline::schema::{BlendMode, Fit, ShapeKind};
use geneva_timeline::{
    Composition, Ratio, ResolvedClip, ResolvedEffect, ResolvedLayer, ResolvedMask, ResolvedSource,
};
use rayon::prelude::*;

use crate::assets::{AssetSource, FileAssets, Image};
use crate::frame::Frame;
use crate::text::TextEngine;
use crate::{RenderError, Renderer};

/// Sub-pixel sample offsets: a 2×2 grid at quarter-pixel positions.
const SUBSAMPLES: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

/// The reference software renderer.
///
/// Every clip is drawn by mapping output pixels back into the clip's own
/// coordinate space, so transforms are exact and edges are anti-aliased by
/// supersampling. Compositing happens in premultiplied linear light.
pub struct CpuRenderer<A: AssetSource> {
    assets: A,
    text: TextEngine,
    /// Rendered text images that do not change with time, by a hash of
    /// the clip and its text, so a caption is laid out once per clip.
    text_cache: HashMap<u64, Image>,
    /// Markup boxes drawn earlier, by a hash of the clip; an HTML source
    /// is the same picture at every time.
    html_cache: HashMap<u64, Image>,
    /// Pixel buffers of nested compositions drawn earlier, used again for
    /// the next ones so that a frame-sized buffer is not allocated and
    /// faulted in on every frame.
    spare: Vec<Vec<LinearRgba>>,
    /// Luma mask images by asset id, shared so that a placement can hold
    /// one while the paint borrows the renderer.
    masks: HashMap<String, Arc<Image>>,
}

/// How many spare buffers are kept.
const SPARE_BUFFERS: usize = 4;

impl<A: AssetSource> std::fmt::Debug for CpuRenderer<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpuRenderer").finish_non_exhaustive()
    }
}

impl CpuRenderer<FileAssets> {
    /// Creates a renderer that loads assets from files under `root`.
    pub fn with_asset_root(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            assets: FileAssets::new(root),
            text: TextEngine::new(),
            text_cache: HashMap::new(),
            html_cache: HashMap::new(),
            spare: Vec::new(),
            masks: HashMap::new(),
        }
    }
}

impl<A: AssetSource> CpuRenderer<A> {
    /// Creates a renderer with a custom asset source.
    pub fn new(assets: A) -> Self {
        Self {
            assets,
            text: TextEngine::new(),
            text_cache: HashMap::new(),
            html_cache: HashMap::new(),
            spare: Vec::new(),
            masks: HashMap::new(),
        }
    }

    /// The luma image of a clip's mask, if it has one, loaded once.
    fn mask_image(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
    ) -> Result<Option<Arc<Image>>, RenderError> {
        let Some(id) = clip.mask.as_ref().and_then(|m| m.asset.as_deref()) else {
            return Ok(None);
        };
        if let Some(img) = self.masks.get(id) {
            return Ok(Some(Arc::clone(img)));
        }
        let img = Arc::new(self.assets.image(comp, id)?.clone());
        self.masks.insert(id.to_owned(), Arc::clone(&img));
        Ok(Some(img))
    }

    /// The asset source.
    pub fn assets_mut(&mut self) -> &mut A {
        &mut self.assets
    }
}

impl<A: AssetSource> Renderer for CpuRenderer<A> {
    fn render_into(
        &mut self,
        comp: &Composition,
        t: Ratio,
        frame: &mut Frame,
    ) -> Result<(), RenderError> {
        if t < Ratio::ZERO || t >= comp.duration {
            return Err(RenderError::OutOfRange {
                time: t,
                duration: comp.duration,
            });
        }
        self.render_layers_into(
            comp,
            &comp.layers,
            comp.width,
            comp.height,
            comp.background,
            t,
            frame,
        )
    }
}

impl<A: AssetSource> CpuRenderer<A> {
    /// Registers the font assets a text source refers to, once each.
    fn load_fonts(
        &mut self,
        comp: &Composition,
        text: &geneva_timeline::ResolvedText,
    ) -> Result<(), RenderError> {
        let fonts = [
            text.spec.style.font.as_deref(),
            text.spec.highlight.as_ref().and_then(|h| h.font.as_deref()),
        ];
        for id in fonts.into_iter().flatten() {
            if comp.assets.contains_key(id) && !self.text.has_font(id) {
                let data = self.assets.font(comp, id)?;
                if self.text.add_font(id, data.as_ref().clone()).is_none() {
                    return Err(RenderError::Asset {
                        id: id.to_owned(),
                        reason: "not a usable font file".to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// What a clip paints at time `t` (`local` is the clip-relative time),
    /// or `None` when it paints nothing.
    fn paint_for(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
        t: Ratio,
        local: f64,
    ) -> Result<Option<Paint<'_>>, RenderError> {
        let paint = match &clip.source {
            ResolvedSource::Solid { color } => Paint::Solid {
                color: color.sample(local),
                width: f64::from(comp.width),
                height: f64::from(comp.height),
            },
            ResolvedSource::Shape {
                kind,
                width,
                height,
                fill,
                stroke,
                radius,
            } => Paint::Shape {
                kind: *kind,
                width: *width,
                height: *height,
                fill: fill.sample(local),
                stroke: *stroke,
                radius: *radius,
            },
            ResolvedSource::Image { asset } => {
                Paint::Image(Cow::Borrowed(self.assets.image(comp, asset)?))
            }
            ResolvedSource::Composition(nested) => {
                let inner = self.render_layers(
                    comp,
                    &nested.layers,
                    nested.width,
                    nested.height,
                    nested.background,
                    (t - clip.start) * clip.speed,
                )?;
                Paint::Image(Cow::Owned(Image::from_frame_pixels(inner)))
            }
            ResolvedSource::Video { asset, in_, .. } => {
                let source_time = *in_ + (t - clip.start) * clip.speed;
                Paint::Image(Cow::Borrowed(self.assets.video_frame(
                    comp,
                    asset,
                    source_time,
                )?))
            }
            ResolvedSource::Html(html) => {
                // Neither layout nor paint depends on time, so the box is
                // drawn once per clip and reused for every frame.
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                clip.path.hash(&mut hasher);
                let key = hasher.finish();
                if !self.html_cache.contains_key(&key) {
                    let prepared =
                        crate::html::prepare(html).map_err(|reason| RenderError::Asset {
                            id: clip.path.clone(),
                            reason,
                        })?;
                    let mut images = HashMap::new();
                    for src in crate::html::image_sources(&prepared) {
                        if let Ok(image) = self.assets.image(comp, &src) {
                            images.insert(src, image.clone());
                        }
                    }
                    let drawn = crate::html::render(html, &prepared, &mut self.text, &images)
                        .map_err(|reason| RenderError::Asset {
                            id: clip.path.clone(),
                            reason,
                        })?;
                    self.html_cache.insert(key, drawn);
                }
                Paint::Image(Cow::Borrowed(&self.html_cache[&key]))
            }
            ResolvedSource::Text(text) => {
                self.load_fonts(comp, text)?;
                if text.words.is_empty() {
                    // Static text: laid out once per clip.
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    clip.path.hash(&mut hasher);
                    text.text.hash(&mut hasher);
                    text.max_width.to_bits().hash(&mut hasher);
                    format!("{:?}", text.spec).hash(&mut hasher);
                    let key = hasher.finish();
                    let engine = &mut self.text;
                    let image = self
                        .text_cache
                        .entry(key)
                        .or_insert_with(|| engine.render(text, local));
                    Paint::Image(Cow::Borrowed(image))
                } else {
                    Paint::Image(Cow::Owned(self.text.render(text, local)))
                }
            }
        };
        Ok(Some(paint))
    }

    /// Whether every clip above the first layer composites normally, so
    /// that the layers above can be drawn on their own and laid over the
    /// first layer's picture afterwards with the same result.
    pub fn overlays_are_plain(comp: &Composition) -> bool {
        comp.layers
            .iter()
            .skip(1)
            .flat_map(|l| l.clips.iter())
            .all(|c| c.blend == BlendMode::Normal && c.effects.is_empty())
    }

    /// Draws the clips above the first layer that are visible at `t` onto
    /// a transparent frame covering just their bounding box, and returns
    /// it with the box `[x0, y0, x1, y1]` in output pixels; `None` when
    /// nothing is shown above the first layer. Laying the result over the
    /// first layer's picture gives the composited frame when
    /// [`overlays_are_plain`](Self::overlays_are_plain) holds.
    pub fn render_overlays(
        &mut self,
        comp: &Composition,
        t: Ratio,
    ) -> Result<Option<(Frame, [u32; 4])>, RenderError> {
        let Some(layers) = comp.layers.get(1..) else {
            return Ok(None);
        };
        let mut items: Vec<(Paint<'static>, Placement, f32, BlendMode)> = Vec::new();
        let mut bounds: Option<[u32; 4]> = None;
        for layer in layers {
            for clip in layer.clips.iter().filter(|c| c.start <= t && t < c.end) {
                let local = (t - clip.start).to_f64();
                let mut opacity = clip.opacity.sample(local).clamp(0.0, 1.0);
                if let Some((_, fade)) = clip.transition_in {
                    if fade > Ratio::ZERO && t < clip.start + fade {
                        opacity *= ((t - clip.start) / fade).to_f64();
                    }
                }
                if opacity <= 0.0 {
                    continue;
                }
                let mask_image = self.mask_image(comp, clip)?;
                let Some(paint) = self.paint_for(comp, clip, t, local)? else {
                    continue;
                };
                let paint = paint.into_owned();
                let Some(window) = window_of(clip, paint.size()) else {
                    continue;
                };
                let Some(place) =
                    Placement::new(comp.width, comp.height, clip, local, window, mask_image)
                else {
                    continue;
                };
                let b = place.bounds;
                if b[0] >= b[2] || b[1] >= b[3] {
                    continue;
                }
                bounds = Some(match bounds {
                    None => b,
                    Some(u) => [
                        u[0].min(b[0]),
                        u[1].min(b[1]),
                        u[2].max(b[2]),
                        u[3].max(b[3]),
                    ],
                });
                items.push((paint, place, opacity as f32, clip.blend));
            }
        }
        let Some(rect) = bounds else {
            return Ok(None);
        };
        let mut frame = Frame::new(rect[2] - rect[0], rect[3] - rect[1], Color::TRANSPARENT);
        for (paint, place, opacity, blend) in items {
            draw(
                &mut frame,
                [rect[0], rect[1]],
                &paint,
                &place,
                opacity,
                blend,
            );
            let pixels = owned_pixels(paint);
            self.recycle(pixels);
        }
        Ok(Some((frame, rect)))
    }

    /// Renders a set of layers into a fresh frame at time `t`, which is
    /// relative to the layers' own origin (the output, or the clip that
    /// shows a nested composition).
    fn render_layers(
        &mut self,
        comp: &Composition,
        layers: &[ResolvedLayer],
        width: u32,
        height: u32,
        background: Color,
        t: Ratio,
    ) -> Result<Frame, RenderError> {
        let mut frame = Frame::from_pixels(self.spare.pop().unwrap_or_default());
        self.render_layers_into(comp, layers, width, height, background, t, &mut frame)?;
        Ok(frame)
    }

    /// Keeps the buffer of a paint that owns one, for the next nested
    /// composition.
    fn recycle(&mut self, pixels: Option<Vec<LinearRgba>>) {
        if let Some(pixels) = pixels {
            if self.spare.len() < SPARE_BUFFERS {
                self.spare.push(pixels);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_layers_into(
        &mut self,
        comp: &Composition,
        layers: &[ResolvedLayer],
        width: u32,
        height: u32,
        background: Color,
        t: Ratio,
        frame: &mut Frame,
    ) -> Result<(), RenderError> {
        frame.reset(width, height, background);
        let visible = layers
            .iter()
            .flat_map(|layer| layer.clips.iter().filter(|c| c.start <= t && t < c.end));
        for clip in visible {
            let local = (t - clip.start).to_f64();
            let mut opacity = clip.opacity.sample(local).clamp(0.0, 1.0);
            if let Some((_, fade)) = clip.transition_in {
                if fade > Ratio::ZERO && t < clip.start + fade {
                    opacity *= ((t - clip.start) / fade).to_f64();
                }
            }
            if opacity <= 0.0 {
                continue;
            }
            // The blur's layer buffer comes from the pool now, while the
            // renderer is free; the paint below borrows it.
            let sigma = clip
                .effects
                .iter()
                .map(|e| match e {
                    ResolvedEffect::Blur(radius) => radius.sample(local).max(0.0),
                })
                .fold(0.0f64, |acc, s| (acc * acc + s * s).sqrt());
            let scratch = if sigma > 0.0 {
                self.spare.pop().unwrap_or_default()
            } else {
                Vec::new()
            };
            let mask_image = self.mask_image(comp, clip)?;
            let Some(paint) = self.paint_for(comp, clip, t, local)? else {
                continue;
            };
            let Some(window) = window_of(clip, paint.size()) else {
                continue;
            };
            let Some(placement) = Placement::new(width, height, clip, local, window, mask_image)
            else {
                continue;
            };
            let scratch = if sigma > 0.0 {
                Some(draw_blurred(
                    scratch,
                    frame,
                    &paint,
                    &placement,
                    sigma,
                    opacity as f32,
                    clip.blend,
                ))
            } else {
                draw(
                    frame,
                    [0, 0],
                    &paint,
                    &placement,
                    opacity as f32,
                    clip.blend,
                );
                None
            };
            let pixels = owned_pixels(paint);
            self.recycle(pixels);
            self.recycle(scratch);
        }
        Ok(())
    }
}

/// Draws a clip blurred by `sigma` output pixels: the picture goes onto
/// a transparent layer covering everything whose blur can reach the
/// frame, the layer is blurred, and the result is laid onto the frame
/// with the clip's opacity and blend mode. A wide blur is computed on a
/// smaller layer (the picture drawn `k` times smaller and blurred by
/// `sigma / k`) and brought back bilinearly, which is indistinguishable
/// at such radii and keeps the cost flat. The layer's buffer is
/// `scratch`, returned for the next use.
fn draw_blurred(
    scratch: Vec<LinearRgba>,
    frame: &mut Frame,
    paint: &Paint,
    place: &Placement,
    sigma: f64,
    opacity: f32,
    blend: BlendMode,
) -> Vec<LinearRgba> {
    let reach = (3.0 * sigma).ceil();
    let (fw, fh) = (f64::from(frame.width()), f64::from(frame.height()));
    let x0 = (place.extent[0] - reach).max(-reach).floor();
    let y0 = (place.extent[1] - reach).max(-reach).floor();
    let x1 = (place.extent[2] + reach).min(fw + reach).ceil();
    let y1 = (place.extent[3] + reach).min(fh + reach).ceil();
    if x1 <= x0 || y1 <= y0 {
        return scratch;
    }
    let k = if sigma >= 4.0 {
        (sigma / 2.0).floor().min(8.0)
    } else {
        1.0
    };
    let lw = ((x1 - x0) / k).ceil().max(1.0) as u32;
    let lh = ((y1 - y0) / k).ceil().max(1.0) as u32;
    let mut layer = Frame::from_pixels(scratch);
    layer.reset(lw, lh, Color::TRANSPARENT);
    if let Some(p) = place.moved([-x0, -y0], 1.0 / k, lw, lh) {
        draw(&mut layer, [0, 0], paint, &p, 1.0, BlendMode::Normal);
    }
    crate::blur::gaussian_blur(&mut layer, sigma / k);
    let image = Image::from_frame_pixels(layer);
    composite_layer(frame, &image, [x0, y0], k, opacity, blend);
    image.pixels
}

/// Lays a layer whose pixel (0, 0) sits at `origin` in the frame, each
/// of its pixels `k` frame pixels wide, onto the frame with `opacity`
/// and `blend`; a layer at the frame's own scale is read texel by texel,
/// a smaller one is sampled bilinearly.
fn composite_layer(
    frame: &mut Frame,
    layer: &Image,
    origin: [f64; 2],
    k: f64,
    opacity: f32,
    blend: BlendMode,
) {
    let (fw, fh) = (frame.width(), frame.height());
    let x0 = origin[0].max(0.0) as u32;
    let y0 = origin[1].max(0.0) as u32;
    let x1 = ((origin[0] + f64::from(layer.width) * k).ceil().max(0.0) as u32).min(fw);
    let y1 = ((origin[1] + f64::from(layer.height) * k).ceil().max(0.0) as u32).min(fh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let plain = opacity >= 1.0 && blend == BlendMode::Normal;
    let width = fw as usize;
    let rows = &mut frame.pixels_mut()[y0 as usize * width..y1 as usize * width];
    let direct = k == 1.0 && origin[0].fract() == 0.0 && origin[1].fract() == 0.0;
    rows.par_chunks_mut(width).enumerate().for_each(|(i, row)| {
        let y = y0 + i as u32;
        for x in x0..x1 {
            let src = if direct {
                layer.texel(
                    i64::from(x) - origin[0] as i64,
                    i64::from(y) - origin[1] as i64,
                )
            } else {
                layer.sample(
                    (f64::from(x) + 0.5 - origin[0]) / k,
                    (f64::from(y) + 0.5 - origin[1]) / k,
                )
            };
            put(row, x as usize, src, opacity, blend, plain);
        }
    });
}

/// The pixel buffer of a paint that owns one, giving up the paint.
fn owned_pixels(paint: Paint<'_>) -> Option<Vec<LinearRgba>> {
    match paint {
        Paint::Image(Cow::Owned(img)) => Some(img.pixels),
        _ => None,
    }
}

/// What a clip paints, in its own coordinate space with the origin at the
/// top-left of its box.
enum Paint<'a> {
    Solid {
        color: LinearRgba,
        width: f64,
        height: f64,
    },
    Shape {
        kind: ShapeKind,
        width: f64,
        height: f64,
        fill: LinearRgba,
        stroke: Option<(LinearRgba, f64)>,
        radius: f64,
    },
    Image(Cow<'a, Image>),
}

impl Paint<'_> {
    /// The same paint with any borrowed image copied.
    fn into_owned(self) -> Paint<'static> {
        match self {
            Self::Solid {
                color,
                width,
                height,
            } => Paint::Solid {
                color,
                width,
                height,
            },
            Self::Shape {
                kind,
                width,
                height,
                fill,
                stroke,
                radius,
            } => Paint::Shape {
                kind,
                width,
                height,
                fill,
                stroke,
                radius,
            },
            Self::Image(img) => Paint::Image(Cow::Owned(img.into_owned())),
        }
    }

    fn size(&self) -> (f64, f64) {
        match self {
            Self::Solid { width, height, .. } | Self::Shape { width, height, .. } => {
                (*width, *height)
            }
            Self::Image(img) => (f64::from(img.width), f64::from(img.height)),
        }
    }

    /// Color at a point of the box; transparent outside the geometry.
    fn sample(&self, u: f64, v: f64) -> LinearRgba {
        let (w, h) = self.size();
        if u < 0.0 || v < 0.0 || u >= w || v >= h {
            return LinearRgba::TRANSPARENT;
        }
        match self {
            Self::Solid { color, .. } => *color,
            Self::Image(img) => img.sample(u, v),
            Self::Shape {
                kind: ShapeKind::Rect,
                fill,
                stroke,
                radius,
                ..
            } => {
                // Signed distance to a rounded rectangle centred in the box.
                let r = radius.min(w / 2.0).min(h / 2.0);
                let qx = (u - w / 2.0).abs() - (w / 2.0 - r);
                let qy = (v - h / 2.0).abs() - (h / 2.0 - r);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let d = outside + qx.max(qy).min(0.0) - r;
                if d > 0.0 {
                    return LinearRgba::TRANSPARENT;
                }
                match stroke {
                    Some((color, width)) if d > -width => *color,
                    _ => *fill,
                }
            }
            Self::Shape {
                kind: ShapeKind::Ellipse,
                fill,
                stroke,
                ..
            } => {
                let nx = (u - w / 2.0) / (w / 2.0);
                let ny = (v - h / 2.0) / (h / 2.0);
                let f = (nx * nx + ny * ny).sqrt();
                if f > 1.0 {
                    return LinearRgba::TRANSPARENT;
                }
                match stroke {
                    Some((color, width)) if (1.0 - f) * (w.min(h) / 2.0) < *width => *color,
                    _ => *fill,
                }
            }
        }
    }
}

/// The part of a paint a clip shows, `[x, y, width, height]` in the
/// paint's own pixels: all of it, or its crop; `None` when the crop
/// leaves nothing.
fn window_of(clip: &ResolvedClip, (w, h): (f64, f64)) -> Option<[f64; 4]> {
    match clip.crop {
        None => Some([0.0, 0.0, w, h]),
        Some(crop) => crop.to_px(w, h),
    }
}

/// The affine mapping from a clip's box to the output frame, and its inverse.
struct Placement {
    /// The part of the paint shown, in paint coordinates: the clip's box.
    window: [f64; 4],
    /// Anchor in paint coordinates.
    anchor: [f64; 2],
    /// Anchor position in output coordinates.
    position: [f64; 2],
    /// Combined fit and user scale per axis.
    scale: [f64; 2],
    cos: f64,
    sin: f64,
    /// Output-space bounding box `[x0, y0, x1, y1]`, clamped to the frame.
    bounds: [u32; 4],
    /// The same box before clamping, in output pixels.
    extent: [f64; 4],
    /// True when box pixels map one-to-one onto output pixels.
    pixel_aligned: bool,
    /// The clip's mask, evaluated in paint coordinates.
    mask: Option<MaskEval>,
}

/// A mask ready to evaluate at a point of the paint.
struct MaskEval {
    /// The window (the clip's box) in paint coordinates.
    window: [f64; 4],
    spec: ResolvedMask,
    /// The shape's box in paint coordinates: left, top, width, height.
    rect: [f64; 4],
    luma: Option<Arc<Image>>,
}

impl MaskEval {
    /// Coverage in `[0, 1]` at a point of the paint.
    fn coverage(&self, u: f64, v: f64) -> f32 {
        let c = match &self.luma {
            Some(img) => {
                let [cx, cy, w, h] = self.window;
                // Stretched over the box, its edge texels extended past
                // the edges rather than fading into transparency.
                let (iw, ih) = (f64::from(img.width), f64::from(img.height));
                let p = img.sample(
                    ((u - cx) / w * iw).clamp(0.5, iw - 0.5),
                    ((v - cy) / h * ih).clamp(0.5, ih - 0.5),
                );
                // Premultiplied linear luma: white shows, black or
                // transparent hides.
                (0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b).clamp(0.0, 1.0)
            }
            None => {
                let [mx, my, mw, mh] = self.rect;
                // Signed distance to the shape, negative inside.
                let d = match self.spec.shape {
                    ShapeKind::Rect => {
                        let r = self.spec.radius.min(mw / 2.0).min(mh / 2.0);
                        let qx = (u - mx - mw / 2.0).abs() - (mw / 2.0 - r);
                        let qy = (v - my - mh / 2.0).abs() - (mh / 2.0 - r);
                        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                        outside + qx.max(qy).min(0.0) - r
                    }
                    ShapeKind::Ellipse => {
                        let nx = (u - mx - mw / 2.0) / (mw / 2.0);
                        let ny = (v - my - mh / 2.0) / (mh / 2.0);
                        ((nx * nx + ny * ny).sqrt() - 1.0) * (mw.min(mh) / 2.0)
                    }
                };
                if self.spec.feather > 0.0 {
                    (0.5 - d / self.spec.feather).clamp(0.0, 1.0) as f32
                } else if d <= 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
        };
        if self.spec.invert { 1.0 - c } else { c }
    }
}

impl Placement {
    fn new(
        frame_w: u32,
        frame_h: u32,
        clip: &ResolvedClip,
        local: f64,
        window: [f64; 4],
        mask_image: Option<Arc<Image>>,
    ) -> Option<Self> {
        let [cx, cy, w, h] = window;
        let mask = clip.mask.as_ref().map(|m| {
            let [mx, my, mw, mh] = m.rect_px(w, h);
            MaskEval {
                window,
                spec: m.clone(),
                rect: [cx + mx, cy + my, mw, mh],
                luma: mask_image,
            }
        });
        let out_w = f64::from(frame_w);
        let out_h = f64::from(frame_h);
        let fit = match clip.fit {
            Fit::None => [1.0, 1.0],
            Fit::Contain => {
                let s = (out_w / w).min(out_h / h);
                [s, s]
            }
            Fit::Cover => {
                let s = (out_w / w).max(out_h / h);
                [s, s]
            }
            Fit::Fill => [out_w / w, out_h / h],
        };
        let user = clip.scale.sample(local);
        let scale = [fit[0] * user[0], fit[1] * user[1]];
        if scale[0] == 0.0 || scale[1] == 0.0 || !scale[0].is_finite() || !scale[1].is_finite() {
            return None;
        }
        // The anchor is a point of the box, which is the window.
        let anchor = [cx + clip.anchor.x.to_px(w), cy + clip.anchor.y.to_px(h)];
        let position = clip.position.sample(local);
        let angle = clip.rotation.sample(local).to_radians();
        let (sin, cos) = angle.sin_cos();

        let forward = |q: [f64; 2]| {
            let x = (q[0] - anchor[0]) * scale[0];
            let y = (q[1] - anchor[1]) * scale[1];
            [
                position[0] + cos * x - sin * y,
                position[1] + sin * x + cos * y,
            ]
        };
        let corners = [
            forward([cx, cy]),
            forward([cx + w, cy]),
            forward([cx, cy + h]),
            forward([cx + w, cy + h]),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for [x, y] in corners {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Self::finish(
            window,
            anchor,
            position,
            scale,
            (cos, sin),
            [x0, y0, x1, y1],
            (frame_w, frame_h),
            mask,
        )
    }

    /// Clamps the extent to the frame and decides pixel alignment.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        window: [f64; 4],
        anchor: [f64; 2],
        position: [f64; 2],
        scale: [f64; 2],
        (cos, sin): (f64, f64),
        extent: [f64; 4],
        (frame_w, frame_h): (u32, u32),
        mask: Option<MaskEval>,
    ) -> Option<Self> {
        let out_w = f64::from(frame_w);
        let out_h = f64::from(frame_h);
        let [x0, y0, x1, y1] = extent;
        if x1 <= 0.0 || y1 <= 0.0 || x0 >= out_w || y0 >= out_h {
            return None;
        }
        let bounds = [
            x0.floor().max(0.0) as u32,
            y0.floor().max(0.0) as u32,
            (x1.ceil().min(out_w)) as u32,
            (y1.ceil().min(out_h)) as u32,
        ];
        let rotation_is_identity = (cos - 1.0).abs() < 1e-12 && sin.abs() < 1e-12;
        let unit_scale = (scale[0] - 1.0).abs() < 1e-12 && (scale[1] - 1.0).abs() < 1e-12;
        let integer_offset = |v: f64| (v - v.round()).abs() < 1e-9;
        let pixel_aligned = rotation_is_identity
            && unit_scale
            && integer_offset(position[0] - anchor[0])
            && integer_offset(position[1] - anchor[1])
            && window.iter().all(|v| integer_offset(*v));
        Some(Self {
            window,
            anchor,
            position,
            scale,
            cos,
            sin,
            bounds,
            extent,
            pixel_aligned,
            mask,
        })
    }

    /// The same placement in another frame: output coordinates are moved
    /// by `offset` and then multiplied by `factor`, and the bounds are
    /// clamped to the new frame's size.
    fn moved(&self, offset: [f64; 2], factor: f64, frame_w: u32, frame_h: u32) -> Option<Self> {
        let position = [
            (self.position[0] + offset[0]) * factor,
            (self.position[1] + offset[1]) * factor,
        ];
        let scale = [self.scale[0] * factor, self.scale[1] * factor];
        let extent = [
            (self.extent[0] + offset[0]) * factor,
            (self.extent[1] + offset[1]) * factor,
            (self.extent[2] + offset[0]) * factor,
            (self.extent[3] + offset[1]) * factor,
        ];
        let mask = self.mask.as_ref().map(|m| MaskEval {
            window: m.window,
            spec: m.spec.clone(),
            rect: m.rect,
            luma: m.luma.clone(),
        });
        Self::finish(
            self.window,
            self.anchor,
            position,
            scale,
            (self.cos, self.sin),
            extent,
            (frame_w, frame_h),
            mask,
        )
    }

    /// Whether the placement only moves and scales (no rotation, or a
    /// half turn), so that source coordinates advance by a constant step
    /// along an output row.
    fn axis_aligned(&self) -> bool {
        self.sin.abs() < 1e-12
    }

    /// For an axis-aligned image of `img_w`×`img_h` texels: the output
    /// pixels `[x0, y0, x1, y1)` whose samples, and the texels those
    /// samples blend, all lie inside the window, so they can be read
    /// without checks. Empty when the image is too small.
    fn interior(&self, img_w: u32, img_h: u32) -> [i64; 4] {
        if img_w < 2 || img_h < 2 {
            return [0; 4];
        }
        let [cx, cy, w, h] = self.window;
        let along = |q: f64, axis: usize| {
            self.position[axis] + self.cos * (q - self.anchor[axis]) * self.scale[axis]
        };
        let (xa, xb) = (along(cx, 0), along(cx + w, 0));
        let (ya, yb) = (along(cy, 1), along(cy + h, 1));
        // A sample must stay 1.5 texels inside the window: its bilinear
        // footprint is one texel, and the subsamples sit up to 0.75 px
        // from the pixel's corner.
        let margin = |scale: f64| (1.5 * scale.abs()).ceil() + 1.0;
        let (mx, my) = (margin(self.scale[0]), margin(self.scale[1]));
        [
            (xa.min(xb).ceil() + mx) as i64,
            (ya.min(yb).ceil() + my) as i64,
            (xa.max(xb).floor() - mx) as i64,
            (ya.max(yb).floor() - my) as i64,
        ]
    }

    /// The paint's color at a point of paint space, transparent outside
    /// the window, through the mask.
    fn sample(&self, paint: &Paint, u: f64, v: f64) -> LinearRgba {
        let [cx, cy, w, h] = self.window;
        if u < cx || v < cy || u >= cx + w || v >= cy + h {
            return LinearRgba::TRANSPARENT;
        }
        let p = paint.sample(u, v);
        match &self.mask {
            Some(m) => p.scaled(m.coverage(u, v)),
            None => p,
        }
    }

    /// Maps an output point back into paint coordinates.
    fn inverse(&self, x: f64, y: f64) -> (f64, f64) {
        let dx = x - self.position[0];
        let dy = y - self.position[1];
        let rx = self.cos * dx + self.sin * dy;
        let ry = -self.sin * dx + self.cos * dy;
        (
            self.anchor[0] + rx / self.scale[0],
            self.anchor[1] + ry / self.scale[1],
        )
    }
}

/// Draws `paint` into `frame`, whose top-left corner sits at `origin` in
/// output coordinates (the placement's bounds are in output coordinates
/// too, so a frame covering part of the output receives its part).
fn draw(
    frame: &mut Frame,
    origin: [u32; 2],
    paint: &Paint,
    place: &Placement,
    opacity: f32,
    blend: BlendMode,
) {
    let [ox, oy] = origin;
    let x0 = place.bounds[0].max(ox);
    let y0 = place.bounds[1].max(oy);
    let x1 = place.bounds[2].min(ox + frame.width());
    let y1 = place.bounds[3].min(oy + frame.height());
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let width = frame.width() as usize;
    let rows = &mut frame.pixels_mut()[(y0 - oy) as usize * width..(y1 - oy) as usize * width];
    let plain = opacity >= 1.0 && blend == BlendMode::Normal;
    // A pixel-aligned image maps texels one-to-one onto output pixels at
    // an integer offset, so it is read directly instead of sampled.
    let aligned = match paint {
        Paint::Image(img) if place.pixel_aligned && place.mask.is_none() => Some((
            img.as_ref(),
            (place.position[0] - place.anchor[0]).round() as i64,
            (place.position[1] - place.anchor[1]).round() as i64,
        )),
        _ => None,
    };
    // An image that is only moved and scaled is resampled span by span:
    // along a row the source coordinates advance by a constant step, and
    // well inside the picture the texels need no bounds checks.
    let spans = match paint {
        Paint::Image(img)
            if !place.pixel_aligned && place.axis_aligned() && place.mask.is_none() =>
        {
            Some((img.as_ref(), place.interior(img.width, img.height)))
        }
        _ => None,
    };
    // The texels shown: the window, within the image.
    let [wx, wy, ww, wh] = place.window.map(|v| v.round() as i64);
    // Rows are independent, so they are drawn in parallel.
    rows.par_chunks_mut(width).enumerate().for_each(|(i, row)| {
        let y = y0 + i as u32;
        if let Some((img, ox_img, oy_img)) = aligned {
            let v = i64::from(y) - oy_img;
            if v < wy.max(0) || v >= (wy + wh).min(i64::from(img.height)) {
                return;
            }
            let src_row = &img.pixels[v as usize * img.width as usize..][..img.width as usize];
            let first = i64::from(x0).max(ox_img + wx.max(0));
            let last = i64::from(x1).min(ox_img + (wx + ww).min(i64::from(img.width)));
            for x in first..last {
                let src = src_row[(x - ox_img) as usize];
                put(
                    row,
                    (x - i64::from(ox)) as usize,
                    src,
                    opacity,
                    blend,
                    plain,
                );
            }
            return;
        }
        if let Some((img, interior)) = spans {
            let in_rows = interior[1] <= i64::from(y) && i64::from(y) < interior[3];
            let (fast0, fast1) = if in_rows {
                (
                    interior[0].clamp(i64::from(x0), i64::from(x1)) as u32,
                    interior[2].clamp(i64::from(x0), i64::from(x1)) as u32,
                )
            } else {
                (x1, x1)
            };
            for x in x0..fast0 {
                let src = sample_pixel(paint, place, x, y);
                put(row, (x - ox) as usize, src, opacity, blend, plain);
            }
            if fast0 < fast1 {
                draw_span(row, ox, img, place, fast0, fast1, y, opacity, blend, plain);
            }
            for x in fast1.max(x0)..x1 {
                let src = sample_pixel(paint, place, x, y);
                put(row, (x - ox) as usize, src, opacity, blend, plain);
            }
            return;
        }
        for x in x0..x1 {
            let src = sample_pixel(paint, place, x, y);
            put(row, (x - ox) as usize, src, opacity, blend, plain);
        }
    });
}

/// The paint's color over output pixel (`x`, `y`): one sample at the
/// center when the placement is pixel-aligned, otherwise the average of
/// the subsamples.
fn sample_pixel(paint: &Paint, place: &Placement, x: u32, y: u32) -> LinearRgba {
    if place.pixel_aligned {
        let (u, v) = place.inverse(f64::from(x) + 0.5, f64::from(y) + 0.5);
        return place.sample(paint, u, v);
    }
    let mut acc = LinearRgba::TRANSPARENT;
    for (ox, oy) in SUBSAMPLES {
        let (u, v) = place.inverse(f64::from(x) + ox, f64::from(y) + oy);
        let s = place.sample(paint, u, v);
        acc.r += s.r;
        acc.g += s.g;
        acc.b += s.b;
        acc.a += s.a;
    }
    acc.scaled(1.0 / SUBSAMPLES.len() as f32)
}

/// Lays `src` at index `i` of an output row with the clip's opacity and
/// blend mode; `plain` says the mode is normal at full opacity, where an
/// opaque source simply replaces the pixel.
#[inline]
fn put(
    row: &mut [LinearRgba],
    i: usize,
    src: LinearRgba,
    opacity: f32,
    blend: BlendMode,
    plain: bool,
) {
    if src.a <= 0.0 {
        return;
    }
    if plain && src.a >= 1.0 {
        row[i] = src;
        return;
    }
    let src = src.scaled(opacity);
    row[i] = composite(src, row[i], blend);
}

/// Two texel rows and the weight between them, for one subsample row.
struct RowPair<'a> {
    top: &'a [LinearRgba],
    bottom: &'a [LinearRgba],
    ty: f32,
    /// Source x of the first output pixel's sample.
    u0: f64,
}

/// Resamples the output pixels `[x0, x1)` of row `y` from an axis-aligned
/// image whose texels around every sample lie inside the window (see
/// [`Placement::interior`]). Magnified or unit-scale pictures take one
/// bilinear sample at the pixel center, which is the usual resampling;
/// minified ones keep the subsample average so that detail is filtered
/// rather than dropped.
#[allow(clippy::too_many_arguments)]
fn draw_span(
    row: &mut [LinearRgba],
    ox: u32,
    img: &Image,
    place: &Placement,
    x0: u32,
    x1: u32,
    y: u32,
    opacity: f32,
    blend: BlendMode,
    plain: bool,
) {
    const CENTER: [(f64, f64); 1] = [(0.5, 0.5)];
    let magnified = place.scale[0].abs() >= 1.0 && place.scale[1].abs() >= 1.0;
    let samples: &[(f64, f64)] = if magnified { &CENTER } else { &SUBSAMPLES };
    let weight = 1.0 / samples.len() as f32;
    let (w, h) = (img.width as usize, img.height as usize);
    let du = place.cos / place.scale[0];
    let mut pairs: [Option<RowPair>; SUBSAMPLES.len()] = [None, None, None, None];
    for (k, (sx, sy)) in samples.iter().enumerate() {
        let (u, v) = place.inverse(f64::from(x0) + sx, f64::from(y) + sy);
        let fy = v - 0.5;
        let yi = (fy.floor().max(0.0) as usize).min(h - 2);
        pairs[k] = Some(RowPair {
            top: &img.pixels[yi * w..][..w],
            bottom: &img.pixels[(yi + 1) * w..][..w],
            ty: (fy - yi as f64) as f32,
            u0: u,
        });
    }
    let lerp = |a: LinearRgba, b: LinearRgba, t: f32| LinearRgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    };
    for (k, x) in (x0..x1).enumerate() {
        let mut acc = LinearRgba::TRANSPARENT;
        for pair in pairs.iter().flatten() {
            let fx = pair.u0 + k as f64 * du - 0.5;
            let xi = (fx.floor().max(0.0) as usize).min(w - 2);
            let tx = (fx - xi as f64) as f32;
            let top = lerp(pair.top[xi], pair.top[xi + 1], tx);
            let bottom = lerp(pair.bottom[xi], pair.bottom[xi + 1], tx);
            let s = lerp(top, bottom, pair.ty);
            acc.r += s.r;
            acc.g += s.g;
            acc.b += s.b;
            acc.a += s.a;
        }
        put(
            row,
            (x - ox) as usize,
            acc.scaled(weight),
            opacity,
            blend,
            plain,
        );
    }
}

/// Blends premultiplied `src` onto premultiplied `dst`.
///
/// Separable modes follow the W3C compositing formula: each channel is
/// `Cs·αs·(1−αb) + Cb·αb·(1−αs) + αs·αb·B(Cb, Cs)` where `B` is the mode's
/// blend function on straight (un-premultiplied) colors, and the result
/// alpha is the ordinary "over" alpha.
fn composite(src: LinearRgba, dst: LinearRgba, mode: BlendMode) -> LinearRgba {
    let blend: fn(f32, f32) -> f32 = match mode {
        BlendMode::Normal => return src.over(dst),
        BlendMode::Add => {
            return LinearRgba {
                r: src.r + dst.r,
                g: src.g + dst.g,
                b: src.b + dst.b,
                a: src.a + dst.a - src.a * dst.a,
            };
        }
        BlendMode::Multiply => |cb, cs| cb * cs,
        BlendMode::Screen => |cb, cs| cb + cs - cb * cs,
        BlendMode::Overlay => |cb, cs| hard_light(cs, cb),
        BlendMode::Darken => f32::min,
        BlendMode::Lighten => f32::max,
        BlendMode::Difference => |cb, cs| (cb - cs).abs(),
        BlendMode::SoftLight => soft_light,
    };
    let (sa, ba) = (src.a, dst.a);
    let straight = |c: f32, a: f32| if a > 0.0 { c / a } else { 0.0 };
    let channel = |s: f32, d: f32| {
        let (cs, cb) = (straight(s, sa), straight(d, ba));
        s * (1.0 - ba) + d * (1.0 - sa) + sa * ba * blend(cb, cs)
    };
    LinearRgba {
        r: channel(src.r, dst.r),
        g: channel(src.g, dst.g),
        b: channel(src.b, dst.b),
        a: sa + ba * (1.0 - sa),
    }
}

/// Hard light: multiply for dark source values, screen for light ones.
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb * 2.0 * cs
    } else {
        cb + (2.0 * cs - 1.0) - cb * (2.0 * cs - 1.0)
    }
}

/// Soft light as defined by the W3C compositing specification.
fn soft_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
    } else {
        let d = if cb <= 0.25 {
            ((16.0 * cb - 12.0) * cb + 4.0) * cb
        } else {
            cb.sqrt()
        };
        cb + (2.0 * cs - 1.0) * (d - cb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::NoAssets;
    use geneva_color::Color;
    use geneva_timeline::load;

    fn render(text: &str, t: &str) -> Frame {
        let loaded = load(text);
        assert!(loaded.is_ok(), "{:#?}", loaded.diagnostics);
        let comp = loaded.composition.unwrap();
        let t = geneva_timeline::Time::parse(t).unwrap().resolve(comp.fps);
        CpuRenderer::new(NoAssets).render_frame(&comp, t).unwrap()
    }

    fn doc(body: &str) -> String {
        format!(
            r##"{{"geneva":"0.1","output":{{"width":64,"height":32,"fps":30,"duration":"2s"}},{body}}}"##
        )
    }

    #[test]
    fn solid_fills_every_pixel_exactly() {
        let f = render(
            &doc(r##""layers":[{"clips":[{"source":{"kind":"solid","color":"#ff8800"}}]}]"##),
            "0.5s",
        );
        let bytes = f.to_rgba8();
        assert!(bytes.chunks(4).all(|p| p == [255, 136, 0, 255]));
    }

    #[test]
    fn half_opacity_white_over_black_is_linear() {
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"solid","color":"white"},"opacity":0.5}]}]"##,
            ),
            "0s",
        );
        assert_eq!(f.get(10, 10).to_srgb8(), [188, 188, 188, 255]);
    }

    #[test]
    fn rect_lands_on_exact_pixel_bounds() {
        // A 20×10 rect anchored at its top-left corner, placed at (4, 6).
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"red"},
                "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        for y in 0..32 {
            for x in 0..64 {
                let inside = (4..24).contains(&x) && (6..16).contains(&y);
                let px = f.get(x, y).to_srgb8();
                assert_eq!(
                    px,
                    if inside {
                        [255, 0, 0, 255]
                    } else {
                        [0, 0, 0, 255]
                    },
                    "pixel {x},{y}"
                );
            }
        }
    }

    #[test]
    fn crop_shows_one_part_of_the_source_as_the_clip_box() {
        // The middle 10×10 of a 20×10 rect, anchored top-left at (4, 6):
        // the box is the crop, so it spans x in [4, 14).
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"red"},
                "crop":{"x":5,"width":"50%"},
                "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        assert_eq!(f.get(4, 8).to_srgb8(), [255, 0, 0, 255]);
        assert_eq!(f.get(13, 8).to_srgb8(), [255, 0, 0, 255]);
        assert_eq!(f.get(14, 8).to_srgb8(), [0, 0, 0, 255]);
        assert_eq!(f.get(3, 8).to_srgb8(), [0, 0, 0, 255]);
        // A crop that leaves nothing paints nothing.
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"red"},
                "crop":{"x":"99%","width":"0.5%"}}]}]"##,
            ),
            "0s",
        );
        assert_eq!(f.get(32, 16).to_srgb8(), [0, 0, 0, 255]);
    }

    #[test]
    fn blur_spreads_a_shape_past_its_edge() {
        // A 20×10 white rect with its top-left at (4, 6), blurred by 2 px:
        // just outside the edge some of it shows, the middle stays white,
        // far away nothing changes.
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
                "effects":[{"kind":"blur","radius":2}],
                "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        let outside = f.get(2, 10);
        let inside = f.get(14, 11);
        assert!(outside.r > 0.05 && outside.r < 0.5, "{outside:?}");
        assert!(inside.r > 0.95, "{inside:?}");
        assert_eq!(f.get(40, 11).to_srgb8(), [0, 0, 0, 255]);
        // Opacity applies to the blurred result.
        let faded = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
                "effects":[{"kind":"blur","radius":2}],"opacity":0.5,
                "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        assert!((faded.get(14, 11).r - inside.r * 0.5).abs() < 0.02);
    }

    #[test]
    fn a_wide_blur_runs_on_a_smaller_layer() {
        // A frame-sized white solid blurred by 10 px (computed 5 times
        // smaller): the middle stays bright, the corners fade toward the
        // transparent outside, and nothing goes above white.
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"solid","color":"white"},"effects":[{"kind":"blur","radius":10}]}]}]"##,
            ),
            "0s",
        );
        let middle = f.get(32, 16);
        let corner = f.get(0, 0);
        assert!(middle.r > 0.85 && middle.r <= 1.0 + 1e-5, "{middle:?}");
        assert!(corner.r < middle.r * 0.6, "{corner:?} vs {middle:?}");
        assert!(f.pixels().iter().all(|p| p.r <= 1.0 + 1e-5));
    }

    #[test]
    fn masks_cut_shapes_from_the_clip_box() {
        // A 20×10 white rect at (4, 6) with an ellipse mask: the middle
        // shows, the corners of the box do not.
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
                "mask":{"shape":"ellipse"},
                "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        assert_eq!(f.get(14, 11).to_srgb8(), [255, 255, 255, 255]);
        assert_eq!(f.get(4, 6).to_srgb8(), [0, 0, 0, 255]);
        assert_eq!(f.get(23, 15).to_srgb8(), [0, 0, 0, 255]);
        // Inverted, the corners show and the middle does not.
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
                "mask":{"shape":"ellipse","invert":true},
                "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        assert_eq!(f.get(14, 11).to_srgb8(), [0, 0, 0, 255]);
        assert_eq!(f.get(4, 6).to_srgb8(), [255, 255, 255, 255]);
        // A rect mask over the left half with a feather: a soft edge
        // across the middle, percentages of the box.
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
                "mask":{"width":"50%","feather":4},
                "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        assert_eq!(f.get(6, 11).to_srgb8(), [255, 255, 255, 255]);
        let edge = f.get(13, 11).r;
        assert!(edge > 0.2 && edge < 0.8, "{edge}");
        assert_eq!(f.get(22, 11).to_srgb8(), [0, 0, 0, 255]);
    }

    #[test]
    fn luma_masks_come_from_an_image_asset() {
        struct OneImage(Image);
        impl AssetSource for OneImage {
            fn image(&mut self, _: &Composition, _: &str) -> Result<&Image, RenderError> {
                Ok(&self.0)
            }
        }
        // A 2×1 luma image, white then black, over a 20×10 white rect: the
        // left half shows, the right half is hidden.
        let luma = Image::from_rgba8(2, 1, &[255, 255, 255, 255, 0, 0, 0, 255]);
        let loaded = load(&doc(
            r##""assets":{"m":{"src":"m.png"}},"layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
            "mask":{"asset":"m"},
            "transform":{"position":{"x":4,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
        ));
        assert!(loaded.is_ok(), "{:#?}", loaded.diagnostics);
        let comp = loaded.composition.unwrap();
        let f = CpuRenderer::new(OneImage(luma))
            .render_frame(&comp, Ratio::ZERO)
            .unwrap();
        assert_eq!(f.get(6, 11).to_srgb8(), [255, 255, 255, 255]);
        assert_eq!(f.get(22, 11).to_srgb8(), [0, 0, 0, 255]);
    }

    #[test]
    fn rotation_by_ninety_degrees_swaps_the_box() {
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
                "transform":{"position":{"x":32,"y":16},"rotation":90}}]}]"##,
            ),
            "0s",
        );
        // The rotated box spans x in [27, 37) and y in [6, 26).
        assert_eq!(f.get(30, 8).to_srgb8(), [255, 255, 255, 255]);
        assert_eq!(f.get(40, 16).to_srgb8(), [0, 0, 0, 255]);
        assert_eq!(f.get(32, 4).to_srgb8(), [0, 0, 0, 255]);
    }

    #[test]
    fn edges_are_anti_aliased_when_not_pixel_aligned() {
        let f = render(
            &doc(
                r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":20,"height":10,"fill":"white"},
                "transform":{"position":{"x":4.5,"y":6},"anchor":{"x":0,"y":0}}}]}]"##,
            ),
            "0s",
        );
        let edge = f.get(4, 8);
        assert!(edge.a > 0.0 && (edge.r - 0.5).abs() < 1e-6, "{edge:?}");
        assert_eq!(f.get(10, 8).to_srgb8(), [255, 255, 255, 255]);
    }

    #[test]
    fn keyframes_move_the_clip_over_time() {
        let text = doc(
            r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"rect","width":4,"height":4,"fill":"white"},
            "transform":{"anchor":{"x":0,"y":0},"position":{"keyframes":[{"t":0,"v":{"x":0,"y":0}},{"t":"1s","v":{"x":40,"y":0}}]}}}]}]"##,
        );
        let start = render(&text, "0s");
        let half = render(&text, "0.5s");
        assert_eq!(start.get(1, 1).to_srgb8(), [255, 255, 255, 255]);
        assert_eq!(start.get(21, 1).to_srgb8(), [0, 0, 0, 255]);
        assert_eq!(half.get(21, 1).to_srgb8(), [255, 255, 255, 255]);
        assert_eq!(half.get(1, 1).to_srgb8(), [0, 0, 0, 255]);
    }

    #[test]
    fn crossfade_ramps_the_incoming_clip() {
        let text = doc(r##""layers":[{"clips":[
            {"source":{"kind":"solid","color":"black"},"duration":"1s"},
            {"source":{"kind":"solid","color":"white"},"duration":"1s","transition":{"kind":"crossfade","duration":"0.5s"}}]}]"##);
        let mid = render(&text, "0.75s");
        assert_eq!(mid.get(0, 0).to_srgb8(), [188, 188, 188, 255]);
        let after = render(&text, "1.25s");
        assert_eq!(after.get(0, 0).to_srgb8(), [255, 255, 255, 255]);
    }

    #[test]
    fn transparent_background_stays_transparent() {
        let text = r##"{"geneva":"0.1","output":{"width":8,"height":8,"fps":30,"duration":"1s","background":"transparent"},
            "layers":[{"clips":[{"source":{"kind":"shape","shape":"ellipse","width":4,"height":4,"fill":"red"}}]}]}"##;
        let f = render(text, "0s");
        assert_eq!(f.get(0, 0).to_srgb8(), [0, 0, 0, 0]);
        assert_eq!(f.get(4, 4).to_srgb8(), [255, 0, 0, 255]);
    }

    #[test]
    fn video_without_a_decoder_fails_with_the_asset_id() {
        let loaded = load(&doc(
            r##""assets":{"v":{"src":"v.mp4"}},"layers":[{"clips":[{"source":{"kind":"video","asset":"v","out":"1s"}}]}]"##,
        ));
        let comp = loaded.composition.unwrap();
        let err = CpuRenderer::new(NoAssets)
            .render_frame(&comp, Ratio::ZERO)
            .unwrap_err();
        assert_eq!(err.code(), "E501");
        assert!(err.to_string().contains("\"v\""));
    }

    #[test]
    fn out_of_range_times_are_rejected() {
        let comp = load(&doc(
            r##""layers":[{"clips":[{"source":{"kind":"solid","color":"red"}}]}]"##,
        ))
        .composition
        .unwrap();
        let err = CpuRenderer::new(NoAssets)
            .render_frame(&comp, Ratio::from_int(2))
            .unwrap_err();
        assert_eq!(err.code(), "E502");
    }

    #[test]
    fn rendering_is_deterministic() {
        let text = doc(
            r##""layers":[{"clips":[{"source":{"kind":"shape","shape":"ellipse","width":30,"height":20,"fill":"#8899aa","stroke":{"color":"white","width":2}},
            "transform":{"rotation":33,"scale":1.3}}]}]"##,
        );
        let a = render(&text, "0.5s");
        let b = render(&text, "0.5s");
        assert_eq!(a, b);
    }

    #[test]
    fn blend_modes_on_opaque_colors_match_their_definitions() {
        let a = |r: f32, g: f32, b: f32| LinearRgba { r, g, b, a: 1.0 };
        let src = a(0.5, 1.0, 0.0);
        let dst = a(0.5, 0.25, 0.75);
        let close = |x: LinearRgba, y: LinearRgba| {
            assert!(
                (x.r - y.r).abs() < 1e-6 && (x.g - y.g).abs() < 1e-6 && (x.b - y.b).abs() < 1e-6,
                "{x:?} vs {y:?}"
            );
        };
        close(composite(src, dst, BlendMode::Multiply), a(0.25, 0.25, 0.0));
        close(composite(src, dst, BlendMode::Screen), a(0.75, 1.0, 0.75));
        close(composite(src, dst, BlendMode::Darken), a(0.5, 0.25, 0.0));
        close(composite(src, dst, BlendMode::Lighten), a(0.5, 1.0, 0.75));
        close(
            composite(src, dst, BlendMode::Difference),
            a(0.0, 0.75, 0.75),
        );
        // Overlay of 0.5 over anything is the backdrop unchanged.
        close(composite(a(0.5, 0.5, 0.5), dst, BlendMode::Overlay), dst);
        close(composite(a(0.5, 0.5, 0.5), dst, BlendMode::SoftLight), dst);
        close(composite(src, dst, BlendMode::Add), a(1.0, 1.25, 0.75));
    }

    #[test]
    fn blend_modes_degrade_to_over_against_transparent_backdrops() {
        let src = LinearRgba {
            r: 0.2,
            g: 0.4,
            b: 0.6,
            a: 0.5,
        };
        for mode in [
            BlendMode::Multiply,
            BlendMode::Overlay,
            BlendMode::Difference,
            BlendMode::SoftLight,
        ] {
            let out = composite(src, LinearRgba::TRANSPARENT, mode);
            assert!(
                (out.r - src.r).abs() < 1e-6 && (out.a - src.a).abs() < 1e-6,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn video_clips_default_to_contain_and_images_to_natural_size() {
        let text = doc(
            r##""assets":{"v":{"src":"v.mp4"},"i":{"src":"i.png"}},"layers":[{"clips":[
            {"source":{"kind":"video","asset":"v","out":"1s"}},
            {"source":{"kind":"image","asset":"i"},"duration":"1s"}]}]"##,
        );
        let comp = load(&text).composition.unwrap();
        assert_eq!(comp.layers[0].clips[0].fit, Fit::Contain);
        assert_eq!(comp.layers[0].clips[1].fit, Fit::None);
    }

    #[test]
    fn frames_clear_to_the_background() {
        let f = Frame::new(2, 2, Color::from_rgba8(10, 20, 30, 255));
        assert_eq!(f.to_rgba8(), [10, 20, 30, 255].repeat(4));
    }
}
