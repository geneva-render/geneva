//! Measuring and painting the display list `geneva-html` produces.
//!
//! Layout asks two questions this crate can answer — how big a run of
//! text is, and how big an image is — and then hands back boxes with
//! absolute coordinates. Painting them needs nothing the compositor does
//! not already do: a rounded rectangle is a signed distance field, and
//! text goes through the same engine every other text source uses.

use std::collections::HashMap;

use geneva_color::{Color, LinearRgba};
use geneva_html::layout::Rectangle;
use geneva_html::{Content, Laid, Measure, Painted, Prepared, Text};
use geneva_timeline::schema::{TextAlign, TextSource, TextStyle};
use geneva_timeline::{ResolvedHtml, ResolvedText};

use crate::assets::Image;
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
    ResolvedText {
        text: text.to_owned(),
        words: Vec::new(),
        max_width,
        spec: TextSource {
            text: Some(text.to_owned()),
            words: None,
            highlight: None,
            style: TextStyle {
                font: style.family.clone(),
                size: Some(style.size),
                weight: Some(style.weight),
                italic: Some(style.italic),
                color: Some(style.color.into()),
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
            shadow: None,
        },
    }
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
        let source = as_text_source(text, style, f64::from(limit));
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

/// Parses, lays out and paints an HTML source.
pub fn render(
    html: &ResolvedHtml,
    prepared: &Prepared,
    text: &mut TextEngine,
    images: &HashMap<String, Image>,
) -> Result<Image, String> {
    let mut context = Context {
        text,
        images,
        memo: HashMap::new(),
    };
    let laid = prepared.layout(
        html.width.map(|w| w as f32),
        html.height.map(|h| h as f32),
        &mut context,
    )?;
    Ok(paint(&laid, context.text, images))
}

/// The union of everything the display list can touch, in the box's own
/// pixels. A full-frame box whose markup draws one card in a corner is
/// mostly empty; the compositor is told so rather than reading all of it.
fn painted_bounds(laid: &Laid) -> Option<[f64; 4]> {
    let mut b: Option<[f64; 4]> = None;
    for painted in &laid.boxes {
        if painted.opacity <= 0.0 {
            continue;
        }
        // A shadow reaches outside its box; everything else a box draws,
        // its text included, is inside it.
        let grow = painted
            .paint
            .shadow
            .map_or(0.0, |s| s.blur.abs() + s.x.abs().max(s.y.abs()) + 1.0);
        let r = [
            f64::from(painted.rect[0]) - grow,
            f64::from(painted.rect[1]) - grow,
            f64::from(painted.rect[0] + painted.rect[2]) + grow,
            f64::from(painted.rect[1] + painted.rect[3]) + grow,
        ];
        b = Some(match b {
            None => r,
            Some(o) => [
                o[0].min(r[0]),
                o[1].min(r[1]),
                o[2].max(r[2]),
                o[3].max(r[3]),
            ],
        });
    }
    b
}

/// Paints the display list in order.
fn paint(laid: &Laid, text: &mut TextEngine, images: &HashMap<String, Image>) -> Image {
    let width = laid.size.0.ceil().max(1.0) as u32;
    let height = laid.size.1.ceil().max(1.0) as u32;
    let content = painted_bounds(laid).map(|[x0, y0, x1, y1]| {
        let x = x0.floor().clamp(0.0, f64::from(width)) as u32;
        let y = y0.floor().clamp(0.0, f64::from(height)) as u32;
        let right = x1.ceil().clamp(0.0, f64::from(width)) as u32;
        let bottom = y1.ceil().clamp(0.0, f64::from(height)) as u32;
        [x, y, right.saturating_sub(x), bottom.saturating_sub(y)]
    });
    let mut image = Image {
        width,
        height,
        pixels: vec![LinearRgba::TRANSPARENT; width as usize * height as usize],
        content,
    };
    for b in &laid.boxes {
        if b.opacity <= 0.0 {
            continue;
        }
        paint_shadow(&mut image, b);
        paint_box(&mut image, b);
        match &b.content {
            Content::Empty => {}
            Content::Text { text: run, style } => paint_text(&mut image, b, run, style, text),
            Content::Image { src } => {
                if let Some(source) = images.get(src) {
                    paint_image(&mut image, b, source);
                }
            }
        }
    }
    image
}

/// Signed distance to a rounded rectangle, negative inside. Radii go
/// round from the top left corner, as CSS writes them.
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

fn paint_box(image: &mut Image, b: &Painted) {
    let has_border =
        b.border.iter().any(|w| *w > 0.0) && b.paint.border_color.iter().any(|c| c.a > 0.0);
    let background = b.paint.background.filter(|c| c.a > 0.0);
    if background.is_none() && !has_border {
        return;
    }
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
            if let Some(bg) = background {
                blend(image, x, y, bg.to_linear(), outer * clip * b.opacity);
            }
            if has_border && outer > hole {
                let colour = b.paint.border_color[side(b, px, py)];
                if colour.a > 0.0 {
                    blend(
                        image,
                        x,
                        y,
                        colour.to_linear(),
                        (outer - hole) * clip * b.opacity,
                    );
                }
            }
        }
    }
}

fn paint_shadow(image: &mut Image, b: &Painted) {
    let Some(shadow) = b.paint.shadow else {
        return;
    };
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
                blend(image, x, y, shadow.color.to_linear(), a * b.opacity);
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
                    shadow.color.to_linear(),
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
    let source = as_text_source(run, style, box_width);
    let drawn = engine.render(&source, 0.0);
    let slack = box_width - f64::from(drawn.width);
    let dx = f64::from(b.content_rect[0])
        + match style.align {
            geneva_html::TextAlign::Left => 0.0,
            geneva_html::TextAlign::Center => slack / 2.0,
            geneva_html::TextAlign::Right => slack,
        };
    let dy = f64::from(b.content_rect[1]);
    blit(image, b, &drawn, dx, dy, 1.0);
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
