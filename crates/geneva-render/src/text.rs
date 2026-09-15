//! Text layout and rasterization.
//!
//! Layout (shaping, bidi, line breaking, font fallback) is delegated to
//! `cosmic-text`; glyphs are rasterized with `swash` into coverage masks and
//! composited here in linear light, so text takes the same color path as
//! every other source. A text source renders to an image that is then
//! placed like any other box.

use std::collections::HashMap;

use cosmic_text::fontdb::Weight;
use cosmic_text::{
    Align, Attrs, Buffer, CacheKeyFlags, Family, FontSystem, LetterSpacing, Metrics, Shaping,
    Style, SwashCache, SwashContent, Wrap,
};
use geneva_color::{Color, LinearRgba};
use geneva_timeline::ResolvedText;
use geneva_timeline::schema::{TextAlign, TextStyle};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Stroke, Style as ZenoStyle, Vector};

use crate::assets::Image;

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
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    /// Creates an engine with the system fonts available for fallback.
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
            scale: ScaleContext::new(),
            asset_faces: HashMap::new(),
        }
    }

    /// Whether the font asset has been registered.
    pub fn has_font(&self, asset_id: &str) -> bool {
        self.asset_faces.contains_key(asset_id)
    }

    /// Registers a font file's bytes under an asset id. Returns the family
    /// name the file declares, or `None` if it contains no usable face.
    pub fn add_font(&mut self, asset_id: &str, data: Vec<u8>) -> Option<String> {
        let db = self.fonts.db_mut();
        let before: Vec<cosmic_text::fontdb::ID> = db.faces().map(|f| f.id).collect();
        db.load_font_data(data);
        let face = db.faces().find(|f| !before.contains(&f.id))?;
        let family = face.families.first().map(|(name, _)| name.clone())?;
        let loaded = LoadedFace {
            family: family.clone(),
            weight: face.weight,
            style: face.style,
        };
        self.asset_faces.insert(asset_id.to_owned(), loaded);
        Some(family)
    }

    /// Renders a text source at clip-local time `t` seconds into an image
    /// whose size is the text's box (text bounds plus padding).
    pub fn render(&mut self, text: &ResolvedText, t: f64) -> Image {
        let spec = &text.spec;
        let base = self.style(&spec.style, None);
        let active_word = text
            .words
            .iter()
            .position(|(_, s, e)| s.to_f64() <= t && t < e.to_f64());
        let highlight = spec
            .highlight
            .as_ref()
            .map(|h| self.style(h, Some(&spec.style)));

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
        let shadow = spec.shadow.as_ref();
        let (shadow_dx, shadow_dy, shadow_blur) = shadow.map_or((0.0, 0.0, 0.0), |s| {
            (s.x as f32, s.y as f32, s.blur.max(0.0) as f32)
        });
        // Room for strokes and shadows around the text.
        let extra = outline_w + shadow_blur + shadow_dx.abs().max(shadow_dy.abs());
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

        // Glyph placements, computed once and reused for every pass.
        let mut placed: Vec<PlacedGlyph> = Vec::new();
        for run in buffer.layout_runs() {
            for g in run.glyphs.iter() {
                let physical = g.physical((origin_x, origin_y + run.line_y), 1.0);
                placed.push(PlacedGlyph {
                    cache_key: physical.cache_key,
                    x: physical.x,
                    y: physical.y,
                    font_id: g.font_id,
                    glyph_id: g.glyph_id,
                    font_size: g.font_size,
                    weight: g.font_weight,
                    color: styles[g.metadata.min(styles.len() - 1)].color,
                });
            }
        }

        if let Some(s) = shadow {
            let color = s
                .color
                .map_or(
                    Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.5,
                    },
                    |c| c.0,
                )
                .to_linear();
            let mut mask = Mask::new(width, height);
            for g in &placed {
                if outline_w > 0.0 {
                    self.stroke_into(&mut mask, g, outline_w, shadow_dx, shadow_dy);
                }
                self.fill_mask_into(&mut mask, g, shadow_dx, shadow_dy);
            }
            mask.blur(shadow_blur);
            mask.composite(&mut image, color);
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
            self.fill_into(&mut image, g);
        }
        image
    }

    /// Resolves a style block against defaults (and a parent for highlights).
    ///
    /// A font asset supplies its face's weight and style unless the style
    /// block sets them, so a bold font file renders bold without a separate
    /// `weight`.
    fn style(&self, s: &TextStyle, parent: Option<&TextStyle>) -> Resolved {
        let pick = |f: &dyn Fn(&TextStyle) -> Option<f64>| f(s).or_else(|| parent.and_then(f));
        let font = s
            .font
            .clone()
            .or_else(|| parent.and_then(|p| p.font.clone()));
        let face = font.as_ref().and_then(|f| self.asset_faces.get(f));
        let family = font
            .clone()
            .map(|f| face.map_or(f, |face| face.family.clone()));
        let color = s
            .color
            .or_else(|| parent.and_then(|p| p.color))
            .map_or(Color::WHITE, |c| c.0);
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
        Resolved {
            family,
            size: pick(&|s| s.size).unwrap_or(DEFAULT_SIZE).max(1.0) as f32,
            weight,
            italic,
            letter_spacing: pick(&|s| s.letter_spacing).unwrap_or(0.0) as f32,
            color: color.to_linear(),
        }
    }

    /// Draws a glyph's filled coverage (or color bitmap) into the image.
    fn fill_into(&mut self, image: &mut Image, g: &PlacedGlyph) {
        let Some(swash_image) = self.cache.get_image(&mut self.fonts, g.cache_key).as_ref() else {
            return;
        };
        let left = g.x + swash_image.placement.left;
        let top = g.y - swash_image.placement.top;
        let (w, h) = (
            swash_image.placement.width as usize,
            swash_image.placement.height as usize,
        );
        match swash_image.content {
            SwashContent::Mask => {
                for row in 0..h {
                    for col in 0..w {
                        let coverage = f32::from(swash_image.data[row * w + col]) / 255.0;
                        if coverage > 0.0 {
                            blend_pixel(
                                image,
                                left + col as i32,
                                top + row as i32,
                                g.color.scaled(coverage),
                            );
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
                            blend_pixel(
                                image,
                                left + col as i32,
                                top + row as i32,
                                g.color.scaled(coverage),
                            );
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
    fn blur(&mut self, radius: f32) {
        if radius < 0.5 {
            return;
        }
        let r = (radius * 0.6).round().max(1.0) as usize;
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
