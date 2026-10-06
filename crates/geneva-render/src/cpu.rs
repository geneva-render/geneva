use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use geneva_color::{Color, LinearRgba};
use geneva_timeline::schema::{BlendMode, TransitionKind};
use geneva_timeline::{
    Composition, Ratio, ResolvedClip, ResolvedEffect, ResolvedLayer, ResolvedSource,
};
use rayon::prelude::*;

use crate::assets::{AssetSource, FileAssets, Image, lerp, split};
use crate::frame::Frame;
use crate::painter::{Paint, Painter};
use crate::placement::{Placement, SUBSAMPLES, crop_window};
use crate::transitions::{fade_veil, transition_gain};
use crate::{RenderError, Renderer};

/// The reference software renderer.
///
/// Every clip is drawn by mapping output pixels back into the clip's own
/// coordinate space, so transforms are exact and edges are anti-aliased by
/// supersampling. Compositing happens in premultiplied linear light.
pub struct CpuRenderer<A: AssetSource> {
    painter: Painter<A>,
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
        Self::new(FileAssets::new(root))
    }
}

impl<A: AssetSource> CpuRenderer<A> {
    /// Creates a renderer with a custom asset source.
    pub fn new(assets: A) -> Self {
        Self {
            painter: Painter::new(assets),
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
        let img = Arc::new(self.painter.assets_mut().image(comp, id)?.clone());
        self.masks.insert(id.to_owned(), Arc::clone(&img));
        Ok(Some(img))
    }

    /// The asset source.
    pub fn assets_mut(&mut self) -> &mut A {
        self.painter.assets_mut()
    }
}

impl<A: AssetSource> Renderer for CpuRenderer<A> {
    fn take_warnings(&mut self) -> Vec<String> {
        self.painter.take_warnings()
    }

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
    /// What a clip paints at time `t` (`local` is the clip-relative time),
    /// or `None` when it paints nothing. A nested composition is drawn
    /// here, as a frame; everything else comes from the painter.
    fn paint_for(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
        t: Ratio,
        local: f64,
        size: Option<[u32; 2]>,
    ) -> Result<Option<Paint<'_>>, RenderError> {
        if let ResolvedSource::Composition(nested) = &clip.source {
            let inner = self.render_layers(
                comp,
                &nested.layers,
                nested.width,
                nested.height,
                nested.background,
                (t - clip.start) * clip.speed,
            )?;
            return Ok(Some(Paint::Image(Cow::Owned(Image::from_frame_pixels(
                inner,
            )))));
        }
        Ok(Some(
            self.painter.paint_shrunk(comp, clip, t, local, size)?.paint,
        ))
    }

    /// The smaller size a video clip's frame can be fetched at, and its
    /// full size: a video drawn smaller than it is (after the reduction
    /// `reduce` a blur applies on top) comes at its drawn size, rounded
    /// up to the pixel, or to a sixteenth of its own when its scale is
    /// animated, so that a zoom goes through a few sizes rather than one
    /// per frame; what is left of the reduction happens in linear light. `None` for anything else, a masked clip included,
    /// since a mask is laid out on the frame's own pixels.
    fn shrink_for(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
        local: f64,
        (frame_w, frame_h): (u32, u32),
        reduce: f64,
    ) -> Result<Option<Shrunk>, RenderError> {
        let ResolvedSource::Video { asset, .. } = &clip.source else {
            return Ok(None);
        };
        if clip.mask.is_some() {
            return Ok(None);
        }
        let Some((w, h)) = self.painter.assets_mut().video_size(comp, asset)? else {
            return Ok(None);
        };
        let size = (f64::from(w), f64::from(h));
        let Some(window) = crop_window(clip, size) else {
            return Ok(None);
        };
        let Some(place) = Placement::new(frame_w, frame_h, clip, local, window, None, None) else {
            return Ok(None);
        };
        let steps = if clip.scale.is_constant() {
            1
        } else {
            ZOOM_STEPS
        };
        let fetched = [
            shrunk_len(w, place.scale[0].abs() / reduce, steps),
            shrunk_len(h, place.scale[1].abs() / reduce, steps),
        ];
        Ok((fetched != [w, h]).then_some(Shrunk {
            full: size,
            size: fetched,
        }))
    }

    /// Whether every clip above the first layer composites normally, so
    /// that the layers above can be drawn on their own and laid over the
    /// first layer's picture afterwards with the same result.
    pub fn overlays_are_plain(comp: &Composition) -> bool {
        comp.layers
            .iter()
            .skip(1)
            .flat_map(|l| l.clips.iter())
            .all(|c| {
                c.blend == BlendMode::Normal
                    && c.effects.is_empty()
                    // A fade's dip color covers the whole frame, which is
                    // not something a bounded overlay can carry.
                    && [&c.transition_in, &c.transition_out]
                        .into_iter()
                        .flatten()
                        .all(|t| t.kind != TransitionKind::Fade)
            })
    }

    /// Draws the clips above the first layer that are visible at `t` onto
    /// transparent frames covering just their bounding boxes, one per
    /// group of clips whose boxes touch, and returns each with its box
    /// `[x0, y0, x1, y1]` in output pixels; empty when nothing is shown
    /// above the first layer. Laying them all over the first layer's
    /// picture, in any order, gives the composited frame when
    /// [`overlays_are_plain`](Self::overlays_are_plain) holds.
    pub fn render_overlays(
        &mut self,
        comp: &Composition,
        t: Ratio,
    ) -> Result<Vec<(Frame, [u32; 4])>, RenderError> {
        let Some(layers) = comp.layers.get(1..) else {
            return Ok(Vec::new());
        };
        let mut items: Vec<(Paint<'static>, Placement, f32, BlendMode)> = Vec::new();
        for layer in layers {
            for (i, clip) in layer.visible_at(t) {
                let local = (t - clip.start).to_f64();
                let mut opacity = clip.opacity.sample(local).clamp(0.0, 1.0);
                opacity *= transition_gain(layer, i, t);
                if opacity <= 0.0 {
                    continue;
                }
                let mask_image = self.mask_image(comp, clip)?;
                let shrunk = self.shrink_for(comp, clip, local, (comp.width, comp.height), 1.0)?;
                let Some(paint) = self.paint_for(comp, clip, t, local, shrunk.map(|s| s.size))?
                else {
                    continue;
                };
                let paint = paint.into_owned();
                let Some(place) = place_paint(
                    (comp.width, comp.height),
                    clip,
                    local,
                    &paint,
                    shrunk.map(|s| s.full),
                    mask_image,
                ) else {
                    continue;
                };
                let b = place.bounds;
                if b[0] >= b[2] || b[1] >= b[3] {
                    continue;
                }
                items.push((paint, place, opacity as f32, clip.blend));
            }
        }
        if items.is_empty() {
            return Ok(Vec::new());
        }
        // Clips far apart (a card at the top, captions at the bottom) get
        // a frame each rather than one spanning both, which would be
        // mostly transparent and still converted and laid on in full.
        // Boxes are grouped when they touch once widened to whole 2×2
        // blocks, so no chroma block is laid on twice, and the result is
        // the pixels one frame over the union would give.
        let widen = |b: [u32; 4]| [b[0] & !1, b[1] & !1, (b[2] + 1) & !1, (b[3] + 1) & !1];
        let touch =
            |a: [u32; 4], b: [u32; 4]| a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
        let join = |a: [u32; 4], b: [u32; 4]| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        };
        // Each group: its box, its widened box, and its items in order.
        let mut groups: Vec<([u32; 4], [u32; 4], Vec<usize>)> = Vec::new();
        for (i, (_, place, _, _)) in items.iter().enumerate() {
            let (mut rect, mut wide, mut members) = (place.bounds, widen(place.bounds), vec![i]);
            // Absorbing a group can make this one reach another, so keep
            // going until nothing more touches.
            while let Some(g) = groups.iter().position(|g| touch(g.1, wide)) {
                let (r, w, m) = groups.remove(g);
                rect = join(rect, r);
                wide = join(wide, w);
                members.extend(m);
            }
            groups.push((rect, wide, members));
        }
        let mut items: Vec<Option<_>> = items.into_iter().map(Some).collect();
        let mut out = Vec::with_capacity(groups.len());
        for (rect, _, mut members) in groups {
            // Painter's order within a group, as in one shared frame.
            members.sort_unstable();
            let mut frame = Frame::new(rect[2] - rect[0], rect[3] - rect[1], Color::TRANSPARENT);
            for i in members {
                let (paint, place, opacity, blend) = items[i].take().expect("in one group");
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
            out.push((frame, rect));
        }
        Ok(out)
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
            .flat_map(|layer| layer.visible_at(t).map(move |(i, c)| (layer, i, c)));
        for (layer, i, clip) in visible {
            let local = (t - clip.start).to_f64();
            let mut opacity = clip.opacity.sample(local).clamp(0.0, 1.0);
            opacity *= transition_gain(layer, i, t);
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
            let shrunk =
                self.shrink_for(comp, clip, local, (width, height), blur_reduction(sigma))?;
            let Some(paint) = self.paint_for(comp, clip, t, local, shrunk.map(|s| s.size))? else {
                continue;
            };
            let Some(placement) = place_paint(
                (width, height),
                clip,
                local,
                &paint,
                shrunk.map(|s| s.full),
                mask_image,
            ) else {
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
        // A fade dips the picture through a color, so the veil goes over
        // everything the layers drew.
        if let Some((color, alpha)) = fade_veil(layers, t) {
            frame.veil(color, alpha);
        }
        Ok(())
    }
}

/// A video frame to fetch smaller: its full size, and the size to fetch.
#[derive(Debug, Clone, Copy)]
struct Shrunk {
    full: (f64, f64),
    size: [u32; 2],
}

/// The steps, as fractions of a frame's side, that a frame whose scale
/// is animated is fetched at.
const ZOOM_STEPS: u32 = 16;

/// The length a side of `len` pixels drawn at `scale` of it is fetched
/// at: the drawn length rounded up to the next of `steps` steps (1: to
/// the pixel), and never more than `len`.
fn shrunk_len(len: u32, scale: f64, steps: u32) -> u32 {
    let step = if steps > 1 {
        len.div_ceil(steps).max(1)
    } else {
        1
    };
    // A hair under a whole number is that number, not the next one up.
    let drawn = (f64::from(len) * scale - 1e-6).ceil().max(1.0) as u32;
    (drawn.div_ceil(step) * step).min(len)
}

/// How many times smaller a blur of `sigma` output pixels draws its
/// layer (see [`draw_blurred`]).
fn blur_reduction(sigma: f64) -> f64 {
    if sigma >= 4.0 {
        (sigma / 2.0).floor().min(8.0)
    } else {
        1.0
    }
}

/// Places `paint` for `clip`. `full`, for a video frame fetched smaller,
/// is the frame's full size, which the clip's crop and fit are worked
/// out on; the placement is then carried over to the smaller texels.
fn place_paint(
    (width, height): (u32, u32),
    clip: &ResolvedClip,
    local: f64,
    paint: &Paint,
    full: Option<(f64, f64)>,
    mask_image: Option<Arc<Image>>,
) -> Option<Placement> {
    let size = paint.size();
    let window = crop_window(clip, full.unwrap_or(size))?;
    let place = Placement::new(
        width,
        height,
        clip,
        local,
        window,
        paint.content(),
        mask_image,
    )?;
    match full {
        Some((w, h)) if (w, h) != size => place.in_texels([w / size.0, h / size.1]),
        _ => Some(place),
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
    let k = blur_reduction(sigma);
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
    if k == 1.0 && origin[0].fract() == 0.0 && origin[1].fract() == 0.0 {
        rows.par_chunks_mut(width).enumerate().for_each(|(i, row)| {
            let y = y0 + i as u32;
            for x in x0..x1 {
                let src = layer.texel(
                    i64::from(x) - origin[0] as i64,
                    i64::from(y) - origin[1] as i64,
                );
                put(row, x as usize, src, opacity, blend, plain);
            }
        });
        return;
    }
    // Bilinear magnification is separable: every layer row the frame
    // rows touch is stretched to the frame's width once, and each frame
    // row is then one blend of two stretched rows. The weights are the
    // ones `Image::sample` computes, texels outside the layer are
    // transparent as there, and the result is the same to the bit.
    let columns: Vec<(i64, f32)> = (x0..x1)
        .map(|x| split((f64::from(x) + 0.5 - origin[0]) / k - 0.5))
        .collect();
    let row_of = |y: u32| split((f64::from(y) + 0.5 - origin[1]) / k - 0.5);
    let (lw, lh) = (i64::from(layer.width), i64::from(layer.height));
    let first = row_of(y0).0.max(0);
    let last = (row_of(y1 - 1).0 + 1).min(lh - 1);
    let span = columns.len();
    let mut stretched = vec![LinearRgba::TRANSPARENT; span * (last - first + 1).max(0) as usize];
    stretched
        .par_chunks_mut(span)
        .enumerate()
        .for_each(|(i, out)| {
            let r = first + i as i64;
            let src = &layer.pixels[(r * lw) as usize..][..lw as usize];
            let texel = |x: i64| {
                if x >= 0 && x < lw {
                    src[x as usize]
                } else {
                    LinearRgba::TRANSPARENT
                }
            };
            for (o, &(xi, tx)) in out.iter_mut().zip(&columns) {
                *o = if xi >= 0 && xi + 1 < lw {
                    lerp(src[xi as usize], src[xi as usize + 1], tx)
                } else {
                    lerp(texel(xi), texel(xi + 1), tx)
                };
            }
        });
    let stretched_row = |r: i64| {
        (first..=last)
            .contains(&r)
            .then(|| &stretched[(r - first) as usize * span..][..span])
    };
    rows.par_chunks_mut(width).enumerate().for_each(|(i, row)| {
        let (yi, ty) = row_of(y0 + i as u32);
        let out = &mut row[x0 as usize..x1 as usize];
        match (stretched_row(yi), stretched_row(yi + 1)) {
            (Some(top), Some(bottom)) => {
                for (j, (&t, &b)) in top.iter().zip(bottom).enumerate() {
                    put(out, j, lerp(t, b, ty), opacity, blend, plain);
                }
            }
            (top, bottom) => {
                for j in 0..span {
                    let t = top.map_or(LinearRgba::TRANSPARENT, |r| r[j]);
                    let b = bottom.map_or(LinearRgba::TRANSPARENT, |r| r[j]);
                    put(out, j, lerp(t, b, ty), opacity, blend, plain);
                }
            }
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

impl Placement {
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
    // Unrotated and magnified, every row samples the same source
    // columns, so they are worked out once for the whole draw.
    let columns = match spans {
        Some((img, interior)) if place.sin == 0.0 && magnified(place) => {
            let fast0 = interior[0].clamp(i64::from(x0), i64::from(x1)) as u32;
            let fast1 = interior[2].clamp(i64::from(x0), i64::from(x1)) as u32;
            span_columns(img, place, fast0, fast1)
        }
        _ => Vec::new(),
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
                let span = Span {
                    img,
                    place,
                    x0: fast0,
                    x1: fast1,
                    y,
                    columns: &columns,
                };
                draw_span(row, ox, &span, opacity, blend, plain);
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

/// A run of output pixels `[x0, x1)` of row `y` resampled from an
/// axis-aligned image whose texels around every sample lie inside the
/// window (see [`Placement::interior`]). `columns`, when not empty, holds
/// the source column and weight of each pixel of the run, the same for
/// every row of an unrotated magnified picture.
struct Span<'a> {
    img: &'a Image,
    place: &'a Placement,
    x0: u32,
    x1: u32,
    y: u32,
    columns: &'a [(usize, f32)],
}

/// Whether a placement enlarges its picture on both axes, which is
/// resampled with one bilinear sample per pixel.
fn magnified(place: &Placement) -> bool {
    place.scale[0].abs() >= 1.0 && place.scale[1].abs() >= 1.0
}

/// The texel column left of each center sample of the pixels `[x0, x1)`
/// and the weight of the one right of it, as [`draw_span`] computes them
/// for one row.
fn span_columns(img: &Image, place: &Placement, x0: u32, x1: u32) -> Vec<(usize, f32)> {
    let w = img.width as usize;
    let du = place.cos / place.scale[0];
    let (u0, _) = place.inverse(f64::from(x0) + 0.5, 0.5);
    (0..x1.saturating_sub(x0))
        .map(|k| {
            let fx = u0 + f64::from(k) * du - 0.5;
            let xi = (fx.max(0.0) as usize).min(w - 2);
            (xi, (fx - xi as f64) as f32)
        })
        .collect()
}

/// Resamples a [`Span`]. Magnified or unit-scale pictures take one
/// bilinear sample at the pixel center, which is the usual resampling;
/// minified ones keep the subsample average so that detail is filtered
/// rather than dropped.
fn draw_span(
    row: &mut [LinearRgba],
    ox: u32,
    span: &Span,
    opacity: f32,
    blend: BlendMode,
    plain: bool,
) {
    const CENTER: [(f64, f64); 1] = [(0.5, 0.5)];
    let Span {
        img,
        place,
        x0,
        x1,
        y,
        columns,
    } = *span;
    let magnified = magnified(place);
    let samples: &[(f64, f64)] = if magnified { &CENTER } else { &SUBSAMPLES };
    let weight = 1.0 / samples.len() as f32;
    let (w, h) = (img.width as usize, img.height as usize);
    let du = place.cos / place.scale[0];
    let mut pairs: [Option<RowPair>; SUBSAMPLES.len()] = [None, None, None, None];
    for (k, (sx, sy)) in samples.iter().enumerate() {
        let (u, v) = place.inverse(f64::from(x0) + sx, f64::from(y) + sy);
        let fy = v - 0.5;
        // Truncation is the floor once negatives are clamped to 0, and
        // spares a library call.
        let yi = (fy.max(0.0) as usize).min(h - 2);
        pairs[k] = Some(RowPair {
            top: &img.pixels[yi * w..][..w],
            bottom: &img.pixels[(yi + 1) * w..][..w],
            ty: (fy - yi as f64) as f32,
            u0: u,
        });
    }
    if !columns.is_empty()
        && let Some(pair) = &pairs[0]
    {
        let out = &mut row[(x0 - ox) as usize..(x1 - ox) as usize];
        for (i, &(xi, tx)) in columns.iter().enumerate() {
            let top = lerp(pair.top[xi], pair.top[xi + 1], tx);
            let bottom = lerp(pair.bottom[xi], pair.bottom[xi + 1], tx);
            put(out, i, lerp(top, bottom, pair.ty), opacity, blend, plain);
        }
        return;
    }
    for (k, x) in (x0..x1).enumerate() {
        let mut acc = LinearRgba::TRANSPARENT;
        for pair in pairs.iter().flatten() {
            let fx = pair.u0 + k as f64 * du - 0.5;
            let xi = (fx.max(0.0) as usize).min(w - 2);
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
pub(crate) fn composite(src: LinearRgba, dst: LinearRgba, mode: BlendMode) -> LinearRgba {
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
        // On premultiplied values these two come out without dividing by
        // either alpha: the general formula below, multiplied through.
        BlendMode::Multiply => {
            let (ks, kb) = (1.0 - src.a, 1.0 - dst.a);
            return LinearRgba {
                r: src.r * kb + dst.r * ks + src.r * dst.r,
                g: src.g * kb + dst.g * ks + src.g * dst.g,
                b: src.b * kb + dst.b * ks + src.b * dst.b,
                a: src.a + dst.a - src.a * dst.a,
            };
        }
        BlendMode::Screen => {
            return LinearRgba {
                r: src.r + dst.r - src.r * dst.r,
                g: src.g + dst.g - src.g * dst.g,
                b: src.b + dst.b - src.b * dst.b,
                a: src.a + dst.a - src.a * dst.a,
            };
        }
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
    use geneva_timeline::schema::Fit;

    fn render(text: &str, t: &str) -> Frame {
        let loaded = load(text);
        assert!(loaded.is_ok(), "{:#?}", loaded.diagnostics);
        let comp = loaded.composition.unwrap();
        let t = geneva_timeline::Time::parse(t).unwrap().resolve(comp.fps);
        CpuRenderer::new(NoAssets).render_frame(&comp, t).unwrap()
    }

    fn doc(body: &str) -> String {
        format!(
            r##"{{"geneva":"1.0","output":{{"width":64,"height":32,"fps":30,"duration":"2s"}},{body}}}"##
        )
    }

    #[test]
    fn a_frame_drawn_smaller_is_fetched_at_its_drawn_size_or_a_step_above() {
        // Held still: to the pixel, a whole-number product included.
        assert_eq!(shrunk_len(192, 0.5, 1), 96);
        assert_eq!(shrunk_len(108, 0.25, 1), 27);
        assert_eq!(shrunk_len(1440, 0.75, 1), 1080);
        assert_eq!(shrunk_len(658, 0.365, 1), 241);
        // Zooming: to the next sixteenth, so a zoom meets few sizes.
        assert_eq!(shrunk_len(108, 0.25, ZOOM_STEPS), 28);
        assert_eq!(shrunk_len(3840, 0.61, ZOOM_STEPS), 2400);
        assert_eq!(shrunk_len(3840, 0.6, ZOOM_STEPS), 2400);
        // Never past the frame, never below a pixel.
        assert_eq!(shrunk_len(100, 1.0, ZOOM_STEPS), 100);
        assert_eq!(shrunk_len(100, 0.97, ZOOM_STEPS), 98, "steps of 7");
        assert_eq!(shrunk_len(100, 0.0001, 1), 1);
    }

    #[test]
    fn a_placement_carried_to_smaller_texels_lands_on_the_same_points() {
        // Rotated, scaled, anchored off center and cropped: every output
        // point must meet the same point of the picture, measured in the
        // picture's pixels, whichever size of it is drawn.
        let comp = load(&doc(r##""layers":[{"clips":[
              {"source":{"kind":"shape","shape":"rect","width":400,"height":200,"fill":"#ffffff"},
               "crop":{"x":40,"y":10,"width":340,"height":160},
               "transform":{"position":{"x":"40%","y":"60%"},"anchor":{"x":"25%","y":"70%"},
                            "scale":{"x":0.3,"y":0.2},"rotation":30}},
              {"source":{"kind":"shape","shape":"rect","width":400,"height":200,"fill":"#ffffff"},
               "mask":{"shape":"ellipse"}}]}]"##))
        .composition
        .unwrap();
        let clips = &comp.layers[0].clips;
        let window = crop_window(&clips[0], (400.0, 200.0)).unwrap();
        let place = Placement::new(64, 32, &clips[0], 0.0, window, None, None).unwrap();
        let f = [4.0, 2.5];
        let small = place.in_texels(f).unwrap();
        assert_eq!(small.bounds, place.bounds);
        for (x, y) in [(0.5, 0.5), (20.25, 11.0), (40.0, 30.75), (63.5, 16.0)] {
            let (u, v) = place.inverse(x, y);
            let (su, sv) = small.inverse(x, y);
            assert!((su * f[0] - u).abs() < 1e-9 && (sv * f[1] - v).abs() < 1e-9);
        }
        let window = crop_window(&clips[1], (400.0, 200.0)).unwrap();
        let masked = Placement::new(64, 32, &clips[1], 0.0, window, None, None).unwrap();
        assert!(
            masked.in_texels(f).is_none(),
            "a mask is laid out on the picture's pixels"
        );
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
        let text = r##"{"geneva":"1.0","output":{"width":8,"height":8,"fps":30,"duration":"1s","background":"transparent"},
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
