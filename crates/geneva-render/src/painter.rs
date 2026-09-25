//! Painting a clip's source: what the renderers share above the
//! composite. Solids and shapes are described; image assets are read;
//! video frames come from the asset source; text is shaped and drawn by
//! the [`TextEngine`]; markup is prepared once per clip and drawn by
//! `html.rs`. Pictures that cannot change from frame to frame (an
//! image asset, static text, a markup box with nothing moving inside)
//! are kept and handed out again under a key, so a renderer that keeps
//! its own copies (the GPU renderer's textures) knows when it has one.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use geneva_color::LinearRgba;
use geneva_timeline::schema::ShapeKind;
use geneva_timeline::{Composition, Ratio, ResolvedClip, ResolvedSource};

use crate::RenderError;
use crate::assets::{AssetSource, Image};
use crate::html::MarkupLayers;
use crate::text::TextEngine;

/// A markup source ready to draw.
struct Scene {
    /// The clip this scene was made for, so a remark can name it.
    path: String,
    prepared: geneva_html::Prepared,
    images: HashMap<String, Image>,
    /// The box as drawn, kept when no animation inside can change it.
    still: Option<Image>,
    /// Pictures of the groups inside that do not change from frame to
    /// frame, which an animated source is redrawn from.
    groups: crate::html::GroupCache,
}

/// Paints clip sources for a renderer, keeping what does not change.
pub struct Painter<A: AssetSource> {
    assets: A,
    text: TextEngine,
    /// Rendered text images that do not change with time, by a hash of
    /// the clip and its text, so a caption is laid out once per clip.
    text_cache: HashMap<u64, Image>,
    /// Markup prepared earlier, by a hash of the clip: the parsed
    /// document and its pictures, and the drawn box when nothing in the
    /// markup moves, which is then the same picture at every time.
    html_cache: HashMap<u64, Scene>,
    /// What the painter has to tell the reader about the render itself,
    /// deduplicated: a group cut down to the buffer bound does it on
    /// most of a clip's frames, and saying so once is the useful number.
    warnings: std::collections::BTreeSet<String>,
}

/// A paint, and a key when the picture is one the painter keeps: two
/// paints with the same key are the same pixels, so a renderer that
/// copies pictures somewhere (a texture) can keep the copy by the key.
/// A paint without a key may differ at every time.
#[derive(Debug)]
pub struct Painted<'a> {
    /// What to draw.
    pub paint: Paint<'a>,
    /// The picture's identity, when it is kept.
    pub key: Option<u64>,
}

impl<A: AssetSource> std::fmt::Debug for Painter<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Painter").finish_non_exhaustive()
    }
}

impl<A: AssetSource> Painter<A> {
    /// What the painter has to say about the render so far, taken and
    /// cleared. Swept from the scenes here rather than recorded as each
    /// frame is drawn, because the painted layers borrow the group cache
    /// until the renderer has composited them.
    pub fn take_warnings(&mut self) -> Vec<String> {
        for scene in self.html_cache.values_mut() {
            if scene.groups.take_clipped() {
                self.warnings.insert(format!(
                    "a group in {:?} reaches further than a group buffer may, so the part past \
                     the bound is not drawn. An element far wider than the frame, such as a news \
                     crawl, belongs in a composition of its own, which has no such bound.",
                    scene.path
                ));
            }
        }
        std::mem::take(&mut self.warnings).into_iter().collect()
    }

    /// A painter reading from `assets`.
    pub fn new(assets: A) -> Self {
        Self {
            assets,
            text: TextEngine::new(),
            text_cache: HashMap::new(),
            html_cache: HashMap::new(),
            warnings: std::collections::BTreeSet::new(),
        }
    }

    /// The asset source.
    pub fn assets_mut(&mut self) -> &mut A {
        &mut self.assets
    }

    /// Registers every font asset of the composition, once each, so that
    /// markup can name them by id or by family.
    fn load_font_assets(&mut self, comp: &Composition) -> Result<(), RenderError> {
        let mut ids: Vec<&String> = comp
            .assets
            .iter()
            .filter(|(id, a)| {
                a.kind == geneva_timeline::schema::AssetKind::Font && !self.text.has_font(id)
            })
            .map(|(id, _)| id)
            .collect();
        // In a fixed order, so two faces of one family register the same
        // way every run.
        ids.sort();
        for id in ids {
            let data = self.assets.font(comp, id)?;
            if self.text.add_font(id, data.as_ref().clone()).is_none() {
                return Err(RenderError::Asset {
                    id: id.to_owned(),
                    reason: "not a usable font file".to_owned(),
                });
            }
        }
        Ok(())
    }

    /// The markup of `clip` prepared, once per clip, and the key it is
    /// kept under. Markup names a font by `font-family`, which may be
    /// the id of a font asset or the family a font asset carries, so
    /// every font asset is registered first; the ones the markup does
    /// not name cost a read.
    fn scene(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
        html: &geneva_timeline::ResolvedHtml,
    ) -> Result<u64, RenderError> {
        self.load_font_assets(comp)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        clip.path.hash(&mut hasher);
        let scene_key = hasher.finish();
        if !self.html_cache.contains_key(&scene_key) {
            let prepared = crate::html::prepare(html).map_err(|reason| RenderError::Asset {
                id: clip.path.clone(),
                reason,
            })?;
            // A picture in markup is a path relative to the markup, as
            // it is on a page; the resolver has already checked the
            // shape and that the file is there.
            let mut images = HashMap::new();
            for src in geneva_html::image_sources(&prepared) {
                let path = if html.base.is_empty() {
                    src.clone()
                } else {
                    format!("{}/{src}", html.base.trim_end_matches('/'))
                };
                if let Ok(image) = self.assets.image_at(&path) {
                    images.insert(src, crate::html::to_encoded(image.clone()));
                }
            }
            self.html_cache.insert(
                scene_key,
                Scene {
                    path: clip.path.clone(),
                    prepared,
                    images,
                    still: None,
                    groups: crate::html::GroupCache::default(),
                },
            );
        }
        Ok(scene_key)
    }

    /// What `clip` paints at time `t` as layers still to composite, when
    /// it is markup with an animation inside: the boxes painted, the
    /// groups left with their opacity, blur, clips and transform for a
    /// renderer that composites them itself. `None` for any other
    /// source, and for markup with nothing moving, which [`Self::paint`]
    /// draws once and keeps whole.
    pub fn markup_layers(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
        local: f64,
    ) -> Result<Option<MarkupLayers<'_>>, RenderError> {
        let ResolvedSource::Html(html) = &clip.source else {
            return Ok(None);
        };
        if html.motion.is_empty() {
            return Ok(None);
        }
        let scene_key = self.scene(comp, clip, html)?;
        let scene = self.html_cache.get_mut(&scene_key).expect("inserted above");
        let layers = crate::html::render_layers(
            html,
            &scene.prepared,
            &mut self.text,
            &scene.images,
            local,
            &mut scene.groups,
            scene_key,
        )
        .map_err(|reason| RenderError::Asset {
            id: clip.path.clone(),
            reason,
        })?;
        Ok(Some(layers))
    }

    /// What `clip` paints at time `t` (`local` is the clip-relative time,
    /// in seconds). A nested composition is the renderer's to draw, since
    /// it is a frame of its own; asking for one here is an error.
    pub fn paint(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
        t: Ratio,
        local: f64,
    ) -> Result<Painted<'_>, RenderError> {
        self.paint_shrunk(comp, clip, t, local, [1, 1])
    }

    /// [`paint`](Self::paint), with a video's frame asked for `shrink`
    /// times smaller on each axis (see
    /// [`AssetSource::video_frame_shrunk`]); the picture is whatever size
    /// the source returns.
    pub fn paint_shrunk(
        &mut self,
        comp: &Composition,
        clip: &ResolvedClip,
        t: Ratio,
        local: f64,
        shrink: [u32; 2],
    ) -> Result<Painted<'_>, RenderError> {
        let mut key = None;
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
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                "image".hash(&mut hasher);
                asset.hash(&mut hasher);
                key = Some(hasher.finish());
                Paint::Image(Cow::Borrowed(self.assets.image(comp, asset)?))
            }
            ResolvedSource::Composition(_) => {
                return Err(RenderError::Unsupported {
                    what: "a nested composition (the renderer draws it, not the painter)"
                        .to_owned(),
                    path: clip.path.clone(),
                });
            }
            ResolvedSource::Video { asset, in_, .. } => {
                let source_time = *in_ + (t - clip.start) * clip.speed;
                let frame = if shrink == [1, 1] {
                    self.assets.video_frame(comp, asset, source_time)?
                } else {
                    self.assets
                        .video_frame_shrunk(comp, asset, source_time, shrink)?
                };
                Paint::Image(Cow::Borrowed(frame))
            }
            ResolvedSource::Html(html) => {
                // With nothing inside the markup moving, the box is drawn
                // once and reused for every frame; an animation on an
                // element inside means a fresh drawing at each time.
                let scene_key = self.scene(comp, clip, html)?;
                let scene = self.html_cache.get_mut(&scene_key).expect("inserted above");
                let failed = |reason| RenderError::Asset {
                    id: clip.path.clone(),
                    reason,
                };
                if html.motion.is_empty() {
                    key = Some(scene_key);
                    if scene.still.is_none() {
                        let drawn = crate::html::render(
                            html,
                            &scene.prepared,
                            &mut self.text,
                            &scene.images,
                            0.0,
                            &mut scene.groups,
                        )
                        .map_err(failed)?;
                        // Drawn once and kept whole; the pictures of the
                        // groups inside it are not needed again.
                        scene.groups.clear();
                        scene.still = Some(drawn);
                    }
                    Paint::Image(Cow::Borrowed(scene.still.as_ref().expect("drawn above")))
                } else {
                    let drawn = crate::html::render(
                        html,
                        &scene.prepared,
                        &mut self.text,
                        &scene.images,
                        local,
                        &mut scene.groups,
                    )
                    .map_err(failed)?;
                    Paint::Image(Cow::Owned(drawn))
                }
            }
            ResolvedSource::Text(text) => {
                // Every font asset, not only the ones this source names by
                // id: a text source may name a family instead, and the
                // faces of a shipped family only displace the machine's
                // once the file has been registered. Markup has always
                // done this, and the two paths disagreeing made the same
                // document draw in the document's font under an id and in
                // the machine's under the family it declares.
                self.load_font_assets(comp)?;
                if text.words.is_empty() && text.is_static() {
                    // Static text: laid out once per clip. A colour or
                    // shadow with keyframes is drawn fresh each frame.
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    clip.path.hash(&mut hasher);
                    text.text.hash(&mut hasher);
                    text.max_width.to_bits().hash(&mut hasher);
                    format!("{:?}", text.spec).hash(&mut hasher);
                    let text_key = hasher.finish();
                    key = Some(text_key);
                    let engine = &mut self.text;
                    let image = self
                        .text_cache
                        .entry(text_key)
                        .or_insert_with(|| engine.render(text, local));
                    Paint::Image(Cow::Borrowed(image))
                } else {
                    Paint::Image(Cow::Owned(self.text.render(text, local)))
                }
            }
        };
        Ok(Painted { paint, key })
    }
}

/// What a clip paints, in its own coordinate space with the origin at the
/// top-left of its box.
#[derive(Debug, Clone)]
pub enum Paint<'a> {
    /// One color over the whole output.
    Solid {
        /// The color, premultiplied linear.
        color: LinearRgba,
        /// The output's width.
        width: f64,
        /// The output's height.
        height: f64,
    },
    /// A rectangle or an ellipse in a box.
    Shape {
        /// Which shape.
        kind: ShapeKind,
        /// The box's width.
        width: f64,
        /// The box's height.
        height: f64,
        /// The fill, premultiplied linear.
        fill: LinearRgba,
        /// The stroke's color and width, inside the edge.
        stroke: Option<(LinearRgba, f64)>,
        /// Corner radius of a rectangle.
        radius: f64,
    },
    /// A picture: an image asset, a video frame, drawn text, a markup box
    /// or a nested composition's frame.
    Image(Cow<'a, Image>),
}

impl Paint<'_> {
    /// The same paint with any borrowed image copied.
    pub fn into_owned(self) -> Paint<'static> {
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

    /// The part of the paint that is not transparent, when it knows.
    pub fn content(&self) -> Option<[f64; 4]> {
        match self {
            Self::Image(img) => img
                .content
                .map(|[x, y, w, h]| [f64::from(x), f64::from(y), f64::from(w), f64::from(h)]),
            Self::Solid { .. } | Self::Shape { .. } => None,
        }
    }

    /// The paint's width and height.
    pub fn size(&self) -> (f64, f64) {
        match self {
            Self::Solid { width, height, .. } | Self::Shape { width, height, .. } => {
                (*width, *height)
            }
            Self::Image(img) => (f64::from(img.width), f64::from(img.height)),
        }
    }

    /// Color at a point of the box; transparent outside the geometry.
    pub(crate) fn sample(&self, u: f64, v: f64) -> LinearRgba {
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
