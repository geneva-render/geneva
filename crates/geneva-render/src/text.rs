//! Text layout and rasterization.
//!
//! Layout (shaping, bidi, line breaking, font fallback) is delegated to
//! `cosmic-text`; glyphs are rasterized with `swash` into coverage masks and
//! composited here in linear light, so text takes the same color path as
//! every other source. A text source renders to an image that is then
//! placed like any other box.

use std::collections::{HashMap, HashSet};

use cosmic_text::fontdb::Weight;
use cosmic_text::{
    Align, Attrs, Buffer, CacheKeyFlags, Family, FontSystem, LetterSpacing, Metrics, Shaping,
    Style, SwashCache, SwashContent, Wrap,
};
use geneva_color::{Color, LinearRgba};
use geneva_timeline::schema::{TextAlign, TextStyle};
use geneva_timeline::{FillTrack, ResolvedText};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Stroke, Style as ZenoStyle, Vector};

use crate::assets::Image;
use crate::fill::Fill;

/// Default font size in pixels.
const DEFAULT_SIZE: f64 = 48.0;
/// Default line height as a multiple of the font size.
const DEFAULT_LINE_HEIGHT: f64 = 1.2;

/// Lays out and rasterizes text sources.
pub struct TextEngine {
    fonts: FontSystem,
    cache: SwashCache,
    scale: ScaleContext,
    /// Faces loaded from assets, by asset id.
    asset_faces: HashMap<String, LoadedFace>,
    /// The faces the document shipped, by database id. A family with one
    /// of these in it is the document's, and the machine's faces of that
    /// family are dropped.
    asset_ids: HashSet<cosmic_text::fontdb::ID>,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// The room `render` leaves around the glyphs for a stroke and a shadow,
/// on every side. A caller that has to place the image where the glyphs
/// alone would have gone subtracts this, and one that lays text out
/// ignores it: neither a stroke nor a shadow changes where text sits.
#[must_use]
pub fn inset_for(text: &ResolvedText) -> f64 {
    let outline = text.spec.outline.as_ref().map_or(0.0, |o| o.width.max(0.0));
    // A shadow's reach is its furthest over the whole clip, so a shadow
    // that grows does not grow the image and move the glyphs with it.
    outline + text.shadow.iter().fold(0.0f64, |m, s| m.max(s.reach))
}

/// Drawn with when the machine has no fonts at all (a bare container, a
/// fresh Wine prefix), rather than no text: the shaper cannot run without a
/// single face. SIL OFL 1.1, like the golden cases that use it.
const LAST_RESORT_FONT: &[u8] =
    include_bytes!("../../../tests/golden/fonts/LiberationSans-Regular.ttf");

impl TextEngine {
    /// Creates an engine with the system fonts available for fallback.
    pub fn new() -> Self {
        Self::with_fonts(FontSystem::new())
    }

    fn with_fonts(mut fonts: FontSystem) -> Self {
        if fonts.db().faces().next().is_none() {
            let db = fonts.db_mut();
            db.load_font_data(LAST_RESORT_FONT.to_vec());
            db.set_sans_serif_family("Liberation Sans");
        }
        Self {
            fonts,
            cache: SwashCache::new(),
            scale: ScaleContext::new(),
            asset_faces: HashMap::new(),
            asset_ids: HashSet::new(),
        }
    }

    /// Whether the font asset has been registered.
    pub fn has_font(&self, asset_id: &str) -> bool {
        self.asset_faces.contains_key(asset_id)
    }

    /// Whether the machine has any face of a family, by the same name
    /// matching the shaper uses. A document naming a family that is not
    /// here still renders, in whatever the shaper falls back to, which is
    /// a different picture on a different machine: this is what lets that
    /// be reported rather than discovered later.
    pub fn family_is_available(&self, family: &str) -> bool {
        self.fonts
            .db()
            .faces()
            .any(|f| f.families.iter().any(|(name, _)| name == family))
    }

    /// Registers a font file's bytes under an asset id. Returns the family
    /// name the file declares, or `None` if it contains no usable face.
    pub fn add_font(&mut self, asset_id: &str, data: Vec<u8>) -> Option<String> {
        let db = self.fonts.db_mut();
        let before: Vec<cosmic_text::fontdb::ID> = db.faces().map(|f| f.id).collect();
        db.load_font_data(data);
        let fresh: Vec<cosmic_text::fontdb::ID> = db
            .faces()
            .filter(|f| !before.contains(&f.id))
            .map(|f| f.id)
            .collect();
        let face = db.faces().find(|f| fresh.contains(&f.id))?;
        let family = face.families.first().map(|(name, _)| name.clone())?;
        let loaded = LoadedFace {
            family: family.clone(),
            weight: face.weight,
            style: face.style,
        };
        // Every face the file carries is the document's, a collection of
        // several included.
        self.asset_ids.extend(fresh.iter().copied());
        // The machine's faces of this family go, so that a document that
        // ships a family draws in it and not in the copy the machine
        // happens to have, which can be another version or another set of
        // weights. The families the document does not ship are untouched,
        // and still serve as the fallback for what it does not cover.
        let strangers: Vec<cosmic_text::fontdb::ID> = self
            .fonts
            .db()
            .faces()
            .filter(|f| f.families.iter().any(|(name, _)| *name == family))
            .filter(|f| !self.asset_ids.contains(&f.id))
            .map(|f| f.id)
            .collect();
        let db = self.fonts.db_mut();
        for id in strangers {
            db.remove_face(id);
        }
        self.asset_faces.insert(asset_id.to_owned(), loaded);
        Some(family)
    }

    /// Renders a text source at clip-local time `t` seconds into an image
    /// whose size is the text's box (text bounds plus padding).
    pub fn render(&mut self, text: &ResolvedText, t: f64) -> Image {
        let spec = &text.spec;
        let base_color = text.color.sample(t);
        let base = self.style(&spec.style, None, base_color);
        let active_word = text
            .words
            .iter()
            .position(|(_, s, e)| s.to_f64() <= t && t < e.to_f64());
        let highlight = spec.highlight.as_ref().map(|h| {
            let color = text
                .highlight_color
                .as_ref()
                .map_or(base_color, |c| c.sample(t));
            self.style(h, Some(&spec.style), color)
        });

        let padding = spec.padding.unwrap_or(0.0).max(0.0) as f32;
        let line_height = spec.line_height.unwrap_or(DEFAULT_LINE_HEIGHT).max(0.1) as f32;
        let wrap_width = (text.max_width as f32 - 2.0 * padding).max(1.0);
        let align = match spec.align.unwrap_or(TextAlign::Center) {
            TextAlign::Left => Align::Left,
            TextAlign::Center => Align::Center,
            TextAlign::Right => Align::Right,
        };

        // Spans: metadata indexes into `styles` so glyph colors are looked
        // up here rather than carried through the layout engine.
        let mut styles: Vec<Resolved> = vec![base.clone()];
        let mut buffer = Buffer::new(
            &mut self.fonts,
            Metrics::new(base.size, base.size * line_height),
        );
        // Lines break between words; a word longer than the line breaks
        // inside rather than running past the edge.
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(wrap_width), None);
        let default_attrs = attrs_for(&base, 0);
        if text.words.is_empty() {
            buffer.set_text(&text.text, &default_attrs, Shaping::Advanced, Some(align));
        } else {
            let mut spans: Vec<(String, usize)> = Vec::new();
            for (i, (word, _, _)) in text.words.iter().enumerate() {
                if i > 0 {
                    spans.push((" ".to_owned(), 0));
                }
                let style_index = match (&highlight, active_word) {
                    (Some(h), Some(active)) if active == i => {
                        styles.push(h.clone());
                        styles.len() - 1
                    }
                    _ => 0,
                };
                spans.push((word.clone(), style_index));
            }
            let rich = spans
                .iter()
                .map(|(s, i)| (s.as_str(), attrs_for(&styles[*i], *i)));
            buffer.set_rich_text(rich, &default_attrs, Shaping::Advanced, Some(align));
        }
        buffer.shape_until_scroll(&mut self.fonts, true);

        // Tight bounds of all glyphs, in buffer coordinates.
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut bottom = 0.0f32;
        let mut glyph_count = 0usize;
        for run in buffer.layout_runs() {
            for g in run.glyphs.iter() {
                min_x = min_x.min(g.x);
                max_x = max_x.max(g.x + g.w);
                glyph_count += 1;
            }
            bottom = bottom.max(run.line_top + run.line_height);
        }
        if glyph_count == 0 {
            min_x = 0.0;
            max_x = 0.0;
        }
        let outline_w = spec
            .outline
            .as_ref()
            .map_or(0.0, |o| o.width.max(0.0) as f32);
        // The shadows at this moment. Their room in the image is the reach
        // over the whole clip, not this moment's, so the glyphs stay put.
        let shadows: Vec<(f32, f32, f32, LinearRgba)> = text
            .shadow
            .iter()
            .map(|s| {
                (
                    s.x.sample(t) as f32,
                    s.y.sample(t) as f32,
                    s.blur.sample(t).max(0.0) as f32,
                    s.color.sample(t),
                )
            })
            .collect();
        // Room for strokes and shadows around the text, worked out in one
        // place so a caller can subtract exactly what was added.
        let extra = inset_for(text) as f32;
        let inset = padding + extra;
        let text_w = (max_x - min_x).max(0.0);
        let width = (text_w + 2.0 * inset).ceil().max(1.0) as u32;
        let height = (bottom + 2.0 * inset).ceil().max(1.0) as u32;
        let origin_x = inset - min_x;
        let origin_y = inset;

        let mut image = Image {
            width,
            height,
            pixels: vec![LinearRgba::TRANSPARENT; width as usize * height as usize],
            content: None,
        };

        if let Some(bg) = spec.background {
            let radius = spec.radius.unwrap_or(0.0).max(0.0) as f32;
            fill_rounded_rect(
                &mut image,
                extra,
                extra,
                width as f32 - 2.0 * extra,
                height as f32 - 2.0 * extra,
                radius,
                bg.0.to_linear(),
            );
        }

        // A gradient's tile sits on the text's box, the image less the
        // room left for strokes and shadows, and moves by the offset
        // sampled at this moment.
        let text_box = (
            f64::from(extra),
            f64::from(extra),
            f64::from(width) - 2.0 * f64::from(extra),
            f64::from(height) - 2.0 * f64::from(extra),
        );
        let fill_now = |f: &FillTrack| {
            Fill::new(
                &f.background,
                (
                    text_box.0 + f.x.sample(t),
                    text_box.1 + f.y.sample(t),
                    f.width.unwrap_or(text_box.2),
                    f.height.unwrap_or(text_box.3),
                ),
            )
        };
        // One per entry in `styles`: the base, then the highlight's.
        let fills: Vec<Option<Fill>> = (0..styles.len())
            .map(|i| {
                if i == 0 {
                    text.fill.as_ref().map(fill_now)
                } else {
                    text.highlight_fill.as_ref().map(fill_now)
                }
            })
            .collect();

        // Glyph placements, computed once and reused for every pass.
        let mut placed: Vec<PlacedGlyph> = Vec::new();
        for run in buffer.layout_runs() {
            for g in run.glyphs.iter() {
                let physical = g.physical((origin_x, origin_y + run.line_y), 1.0);
                let style = g.metadata.min(styles.len() - 1);
                placed.push(PlacedGlyph {
                    cache_key: physical.cache_key,
                    x: physical.x,
                    y: physical.y,
                    font_id: g.font_id,
                    glyph_id: g.glyph_id,
                    font_size: g.font_size,
                    weight: g.font_weight,
                    color: styles[style].color,
                    style,
                });
            }
        }

        // CSS lists shadows front to back, so the last is laid down first.
        for (dx, dy, blur, color) in shadows.iter().rev() {
            let mut mask = Mask::new(width, height);
            for g in &placed {
                if outline_w > 0.0 {
                    self.stroke_into(&mut mask, g, outline_w, *dx, *dy);
                }
                self.fill_mask_into(&mut mask, g, *dx, *dy);
            }
            mask.blur(*blur);
            mask.composite(&mut image, *color);
        }
        if outline_w > 0.0 {
            let color = spec
                .outline
                .as_ref()
                .map_or(LinearRgba::TRANSPARENT, |o| o.color.0.to_linear());
            let mut mask = Mask::new(width, height);
            for g in &placed {
                self.stroke_into(&mut mask, g, outline_w, 0.0, 0.0);
            }
            mask.composite(&mut image, color);
        }
        for g in &placed {
            self.fill_into(&mut image, g, fills[g.style].as_ref());
        }
        image
    }

    /// The style and weight to ask for, snapped to what the family
    /// actually carries.
    ///
    /// The font database holds the machine's fonts beside the document's,
    /// and an attribute no face of the family has is then matched across
    /// all of them: an exact weight or a real italic in another family
    /// outranks the right family at the nearest weight or an upright
    /// face. A Mac carries faces at 500 and 600, this Linux box carries
    /// FreeSans at 600, and neither carries the other, so the same
    /// document drew in three typefaces on three machines. Both axes are
    /// settled here instead, over the family's own faces, so the family
    /// match is exact and nothing else can outrank it.
    ///
    /// A family with no italic face is drawn upright rather than in
    /// another family's italic. There is no synthetic oblique.
    ///
    /// A family the database does not have is left alone, since it falls
    /// back to the machine's fonts either way.
    fn available_face(&self, family: Option<&str>, weight: u16, italic: bool) -> (u16, bool) {
        let Some(family) = family else {
            return (weight, italic);
        };
        let faces: Vec<(u16, bool)> = self
            .fonts
            .db()
            .faces()
            .filter(|f| f.families.iter().any(|(name, _)| name == family))
            .map(|f| (f.weight.0, f.style != Style::Normal))
            .collect();
        if faces.is_empty() {
            return (weight, italic);
        }
        // The style the family has, upright where it has no italic.
        let italic = italic && faces.iter().any(|(_, slanted)| *slanted);
        let mut weights: Vec<u16> = faces
            .iter()
            .filter(|(_, slanted)| *slanted == italic)
            .map(|(w, _)| *w)
            .collect();
        weights.sort_unstable();
        weights.dedup();
        if weights.is_empty() || weights.contains(&weight) {
            return (weight, italic);
        }
        // CSS Fonts 4, 5.2: below 400 look down first, above 500 look up
        // first, and between the two look up to 500 before looking down.
        let nearest_above = |from: u16| weights.iter().copied().find(|w| *w >= from);
        let nearest_below = |from: u16| weights.iter().rev().copied().find(|w| *w <= from);
        let picked = if weight < 400 {
            nearest_below(weight).or_else(|| nearest_above(weight))
        } else if weight > 500 {
            nearest_above(weight).or_else(|| nearest_below(weight))
        } else {
            weights
                .iter()
                .copied()
                .find(|w| *w > weight && *w <= 500)
                .or_else(|| nearest_below(weight))
                .or_else(|| nearest_above(weight))
        };
        (picked.unwrap_or(weight), italic)
    }

    /// Resolves a style block against defaults (and a parent for highlights).
    ///
    /// A font asset supplies its face's weight and style unless the style
    /// block sets them, so a bold font file renders bold without a separate
    /// `weight`.
    /// `color` is sampled by the caller, since it can move over the clip.
    fn style(&self, s: &TextStyle, parent: Option<&TextStyle>, color: LinearRgba) -> Resolved {
        let pick = |f: &dyn Fn(&TextStyle) -> Option<f64>| f(s).or_else(|| parent.and_then(f));
        let font = s
            .font
            .clone()
            .or_else(|| parent.and_then(|p| p.font.clone()));
        let face = font.as_ref().and_then(|f| self.asset_faces.get(f));
        let family = font
            .clone()
            .map(|f| face.map_or(f, |face| face.family.clone()));
        let weight = s
            .weight
            .or_else(|| parent.and_then(|p| p.weight))
            .or_else(|| face.map(|f| f.weight.0))
            .unwrap_or(400);
        let italic = s
            .italic
            .or_else(|| parent.and_then(|p| p.italic))
            .or_else(|| face.map(|f| f.style != Style::Normal))
            .unwrap_or(false);
        let (weight, italic) = self.available_face(family.as_deref(), weight, italic);
        Resolved {
            family,
            size: pick(&|s| s.size).unwrap_or(DEFAULT_SIZE).max(1.0) as f32,
            weight,
            italic,
            letter_spacing: pick(&|s| s.letter_spacing).unwrap_or(0.0) as f32,
            color,
        }
    }

    /// Draws a glyph's filled coverage (or color bitmap) into the image,
    /// in its colour or, with a `fill`, the gradient's colour under each
    /// pixel.
    fn fill_into(&mut self, image: &mut Image, g: &PlacedGlyph, fill: Option<&Fill>) {
        let Some(swash_image) = self.cache.get_image(&mut self.fonts, g.cache_key).as_ref() else {
            return;
        };
        let left = g.x + swash_image.placement.left;
        let top = g.y - swash_image.placement.top;
        let (w, h) = (
            swash_image.placement.width as usize,
            swash_image.placement.height as usize,
        );
        let colour_at =
            |x: i32, y: i32| fill.map_or(g.color, |f| f.at(f64::from(x) + 0.5, f64::from(y) + 0.5));
        match swash_image.content {
            SwashContent::Mask => {
                for row in 0..h {
                    for col in 0..w {
                        let coverage = f32::from(swash_image.data[row * w + col]) / 255.0;
                        if coverage > 0.0 {
                            let (x, y) = (left + col as i32, top + row as i32);
                            blend_pixel(image, x, y, colour_at(x, y).scaled(coverage));
                        }
                    }
                }
            }
            SwashContent::Color => {
                for row in 0..h {
                    for col in 0..w {
                        let p = &swash_image.data[(row * w + col) * 4..(row * w + col) * 4 + 4];
                        let c = Color::from_rgba8(p[0], p[1], p[2], p[3]).to_linear();
                        if c.a > 0.0 {
                            blend_pixel(image, left + col as i32, top + row as i32, c);
                        }
                    }
                }
            }
            SwashContent::SubpixelMask => {
                for row in 0..h {
                    for col in 0..w {
                        let p = &swash_image.data[(row * w + col) * 4..(row * w + col) * 4 + 4];
                        let coverage =
                            (f32::from(p[0]) + f32::from(p[1]) + f32::from(p[2])) / (3.0 * 255.0);
                        if coverage > 0.0 {
                            let (x, y) = (left + col as i32, top + row as i32);
                            blend_pixel(image, x, y, colour_at(x, y).scaled(coverage));
                        }
                    }
                }
            }
        }
    }

    /// Adds a glyph's filled coverage to a mask, offset by `(dx, dy)`.
    fn fill_mask_into(&mut self, mask: &mut Mask, g: &PlacedGlyph, dx: f32, dy: f32) {
        let Some(swash_image) = self.cache.get_image(&mut self.fonts, g.cache_key).as_ref() else {
            return;
        };
        if swash_image.content != SwashContent::Mask {
            return;
        }
        let left = g.x + swash_image.placement.left + dx.round() as i32;
        let top = g.y - swash_image.placement.top + dy.round() as i32;
        let w = swash_image.placement.width as usize;
        for (i, v) in swash_image.data.iter().enumerate() {
            mask.add(
                left + (i % w) as i32,
                top + (i / w) as i32,
                f32::from(*v) / 255.0,
            );
        }
    }

    /// Adds a glyph's stroked outline to a mask. The stroke is centered on
    /// the outline, so `width` is doubled to leave a ring of that width
    /// outside the fill once the fill is drawn on top.
    fn stroke_into(&mut self, mask: &mut Mask, g: &PlacedGlyph, width: f32, dx: f32, dy: f32) {
        let Some(font) = self.fonts.get_font(g.font_id, g.weight) else {
            return;
        };
        let mut scaler = self
            .scale
            .builder(font.as_swash())
            .size(g.font_size)
            .hint(false)
            .build();
        let (fx, fy) = (g.cache_key.x_bin.as_float(), g.cache_key.y_bin.as_float());
        let Some(rendered) = Render::new(&[Source::Outline, Source::Bitmap(StrikeWith::BestFit)])
            .format(Format::Alpha)
            .offset(Vector::new(fx, fy))
            .style(ZenoStyle::Stroke(Stroke::new(width * 2.0)))
            .render(&mut scaler, g.glyph_id)
        else {
            return;
        };
        if rendered.content != swash::scale::image::Content::Mask {
            return;
        }
        let left = g.x + rendered.placement.left + dx.round() as i32;
        let top = g.y - rendered.placement.top + dy.round() as i32;
        let w = rendered.placement.width as usize;
        for (i, v) in rendered.data.iter().enumerate() {
            mask.add(
                left + (i % w) as i32,
                top + (i / w) as i32,
                f32::from(*v) / 255.0,
            );
        }
    }
}

/// A font face registered from an asset.
struct LoadedFace {
    family: String,
    weight: Weight,
    style: Style,
}

/// A style block with defaults applied.
#[derive(Debug, Clone)]
struct Resolved {
    family: Option<String>,
    size: f32,
    weight: u16,
    italic: bool,
    letter_spacing: f32,
    color: LinearRgba,
}

fn attrs_for(style: &Resolved, metadata: usize) -> Attrs<'_> {
    let family = match &style.family {
        Some(name) => Family::Name(name.as_str()),
        None => Family::SansSerif,
    };
    let mut attrs = Attrs::new()
        .family(family)
        .weight(Weight(style.weight))
        .style(if style.italic {
            Style::Italic
        } else {
            Style::Normal
        })
        .metadata(metadata)
        .cache_key_flags(CacheKeyFlags::DISABLE_HINTING);
    // cosmic-text adds this to an advance it has already divided by the
    // font's units per em, so the value it wants is a share of the em,
    // not pixels. Every caller here speaks pixels.
    if style.letter_spacing != 0.0 && style.size > 0.0 {
        attrs.letter_spacing_opt = Some(LetterSpacing(style.letter_spacing / style.size));
    }
    attrs
}

struct PlacedGlyph {
    cache_key: cosmic_text::CacheKey,
    x: i32,
    y: i32,
    font_id: cosmic_text::fontdb::ID,
    glyph_id: u16,
    font_size: f32,
    weight: Weight,
    color: LinearRgba,
    /// Which entry of the render's style list it was shaped with.
    style: usize,
}

/// A coverage buffer used for shadows and outlines.
struct Mask {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl Mask {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as usize,
            height: height as usize,
            data: vec![0.0; width as usize * height as usize],
        }
    }

    fn add(&mut self, x: i32, y: i32, coverage: f32) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let v = &mut self.data[y as usize * self.width + x as usize];
        *v = (*v + coverage).min(1.0);
    }

    /// Approximates a Gaussian blur with three box blurs.
    /// Blurs by a CSS blur radius, which names a Gaussian of half that
    /// standard deviation. Three box blurs of half-width `r` come to a
    /// standard deviation of about `r + 0.5`, so `r` is set from that.
    fn blur(&mut self, radius: f32) {
        if radius < 1.0 {
            return;
        }
        let r = (radius / 2.0 - 0.5).round().max(1.0) as usize;
        for _ in 0..3 {
            self.box_blur(r);
        }
    }

    fn box_blur(&mut self, r: usize) {
        let (w, h) = (self.width, self.height);
        let norm = 1.0 / (2 * r + 1) as f32;
        let mut tmp = vec![0.0f32; w * h];
        for y in 0..h {
            let row = &self.data[y * w..(y + 1) * w];
            let mut acc: f32 = row[..w.min(r + 1)].iter().sum();
            for x in 0..w {
                tmp[y * w + x] = acc * norm;
                if x + r + 1 < w {
                    acc += row[x + r + 1];
                }
                if x >= r {
                    acc -= row[x - r];
                }
            }
        }
        for x in 0..w {
            let mut acc = 0.0f32;
            for y in 0..h.min(r + 1) {
                acc += tmp[y * w + x];
            }
            for y in 0..h {
                self.data[y * w + x] = acc * norm;
                if y + r + 1 < h {
                    acc += tmp[(y + r + 1) * w + x];
                }
                if y >= r {
                    acc -= tmp[(y - r) * w + x];
                }
            }
        }
    }

    fn composite(&self, image: &mut Image, color: LinearRgba) {
        for (i, c) in self.data.iter().enumerate() {
            if *c > 0.0 {
                let p = &mut image.pixels[i];
                *p = color.scaled(*c).over(*p);
            }
        }
    }
}

fn blend_pixel(image: &mut Image, x: i32, y: i32, src: LinearRgba) {
    if x < 0 || y < 0 || x as u32 >= image.width || y as u32 >= image.height {
        return;
    }
    let i = y as usize * image.width as usize + x as usize;
    image.pixels[i] = src.over(image.pixels[i]);
}

/// Fills an axis-aligned rounded rectangle with 2×2 supersampled edges.
fn fill_rounded_rect(
    image: &mut Image,
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: LinearRgba,
) {
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let inside = |px: f32, py: f32| -> bool {
        let u = px - x0;
        let v = py - y0;
        if u < 0.0 || v < 0.0 || u >= w || v >= h {
            return false;
        }
        let qx = (u - w / 2.0).abs() - (w / 2.0 - r);
        let qy = (v - h / 2.0).abs() - (h / 2.0 - r);
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        outside + qx.max(qy).min(0.0) - r <= 0.0
    };
    let (xs, ys, xe, ye) = (
        x0.floor().max(0.0) as u32,
        y0.floor().max(0.0) as u32,
        (x0 + w).ceil().min(image.width as f32) as u32,
        (y0 + h).ceil().min(image.height as f32) as u32,
    );
    for y in ys..ye {
        for x in xs..xe {
            let mut cov = 0.0;
            for (ox, oy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                if inside(x as f32 + ox, y as f32 + oy) {
                    cov += 0.25;
                }
            }
            if cov > 0.0 {
                blend_pixel(image, x as i32, y as i32, color.scaled(cov));
            }
        }
    }
}

#[cfg(test)]
mod face_tests {
    use super::*;

    fn liberation() -> (Vec<u8>, Vec<u8>) {
        let root = "../../tests/golden/fonts";
        (
            std::fs::read(format!("{root}/LiberationSans-Regular.ttf"))
                .expect("the golden root ships it"),
            std::fs::read(format!("{root}/LiberationSans-Bold.ttf"))
                .expect("the golden root ships it"),
        )
    }

    /// A family shipped at 400 and 700 answers every weight with one of
    /// the two, by the CSS rule, whatever the machine's own fonts are.
    #[test]
    fn a_weight_the_family_lacks_snaps_to_one_it_has() {
        let mut engine = TextEngine::new();
        let (regular, bold) = liberation();
        let family = engine.add_font("sans", regular).expect("a usable face");
        engine.add_font("sans-bold", bold).expect("a usable face");
        let at = |w: u16| engine.available_face(Some(&family), w, false).0;
        assert_eq!(at(400), 400);
        assert_eq!(at(700), 700);
        // Above 500 looks up first: 600 is Bold, not Regular.
        assert_eq!(at(600), 700);
        assert_eq!(at(800), 700);
        // 500 looks up as far as 500, finds nothing, and looks down.
        assert_eq!(at(500), 400);
        // Below 400 looks down first, and there is nothing below.
        assert_eq!(at(300), 400);
    }

    /// A machine with no fonts at all still has one to shape with, and it
    /// is the one a document naming no family gets.
    #[test]
    fn a_machine_with_no_fonts_gets_the_last_resort() {
        let empty = FontSystem::new_with_locale_and_db(
            "en-US".to_owned(),
            cosmic_text::fontdb::Database::new(),
        );
        let engine = TextEngine::with_fonts(empty);
        assert!(engine.family_is_available("Liberation Sans"));
        let db = engine.fonts.db();
        let sans = db.query(&cosmic_text::fontdb::Query {
            families: &[cosmic_text::fontdb::Family::SansSerif],
            ..Default::default()
        });
        assert!(sans.is_some(), "sans-serif resolves to it");
    }

    /// A family with no italic face is drawn upright rather than in some
    /// other family's italic.
    #[test]
    fn an_italic_the_family_lacks_falls_back_to_upright() {
        let mut engine = TextEngine::new();
        let (regular, bold) = liberation();
        let family = engine.add_font("sans", regular).expect("a usable face");
        engine.add_font("sans-bold", bold).expect("a usable face");
        assert_eq!(
            engine.available_face(Some(&family), 400, true),
            (400, false)
        );
        // And the weight is still settled within the upright faces.
        assert_eq!(
            engine.available_face(Some(&family), 600, true),
            (700, false)
        );
    }

    /// A family the document ships replaces the machine's copy of it, so
    /// the document draws in the faces it carries rather than in whatever
    /// version the machine has.
    #[test]
    fn a_shipped_family_replaces_the_installed_one() {
        let mut engine = TextEngine::new();
        let (regular, bold) = liberation();
        // Stand in for an installed copy: straight into the database,
        // as loading the system fonts does.
        engine.fonts.db_mut().load_font_data(regular.clone());
        let installed: Vec<_> = engine
            .fonts
            .db()
            .faces()
            .filter(|f| f.families.iter().any(|(n, _)| n == "Liberation Sans"))
            .map(|f| f.id)
            .collect();
        assert!(!installed.is_empty(), "the stand-in is in the database");
        let family = engine.add_font("sans", regular).expect("a usable face");
        engine.add_font("sans-bold", bold).expect("a usable face");
        let left: Vec<_> = engine
            .fonts
            .db()
            .faces()
            .filter(|f| f.families.iter().any(|(n, _)| *n == family))
            .map(|f| f.id)
            .collect();
        assert_eq!(left.len(), 2, "the document's two faces, and no others");
        for id in left {
            assert!(
                engine.asset_ids.contains(&id),
                "every face left is the document's"
            );
        }
    }

    /// A family nobody shipped is left to the machine, which is what
    /// fallback is for.
    #[test]
    fn an_unknown_family_keeps_what_it_asked_for() {
        let engine = TextEngine::new();
        assert_eq!(
            engine.available_face(Some("No Such Family"), 600, true),
            (600, true)
        );
        assert_eq!(engine.available_face(None, 600, false), (600, false));
    }
}
