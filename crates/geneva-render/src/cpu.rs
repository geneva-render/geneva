use std::borrow::Cow;

use geneva_color::{Color, LinearRgba};
use geneva_timeline::schema::{BlendMode, Fit, ShapeKind};
use geneva_timeline::{Composition, Ratio, ResolvedClip, ResolvedLayer, ResolvedSource};

use crate::assets::{AssetSource, FileAssets, Image};
use crate::frame::Frame;
use crate::{RenderError, Renderer};

/// Sub-pixel sample offsets: a 2×2 grid at quarter-pixel positions.
const SUBSAMPLES: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

/// The reference software renderer.
///
/// Every clip is drawn by mapping output pixels back into the clip's own
/// coordinate space, so transforms are exact and edges are anti-aliased by
/// supersampling. Compositing happens in premultiplied linear light.
#[derive(Debug)]
pub struct CpuRenderer<A: AssetSource> {
    assets: A,
}

impl CpuRenderer<FileAssets> {
    /// Creates a renderer that loads assets from files under `root`.
    pub fn with_asset_root(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            assets: FileAssets::new(root),
        }
    }
}

impl<A: AssetSource> CpuRenderer<A> {
    /// Creates a renderer with a custom asset source.
    pub fn new(assets: A) -> Self {
        Self { assets }
    }
}

impl<A: AssetSource> Renderer for CpuRenderer<A> {
    fn render_frame(&mut self, comp: &Composition, t: Ratio) -> Result<Frame, RenderError> {
        if t < Ratio::ZERO || t >= comp.duration {
            return Err(RenderError::OutOfRange {
                time: t,
                duration: comp.duration,
            });
        }
        self.render_layers(
            comp,
            &comp.layers,
            comp.width,
            comp.height,
            comp.background,
            t,
        )
    }
}

impl<A: AssetSource> CpuRenderer<A> {
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
        let mut frame = Frame::new(width, height, background);
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
                        t - clip.start,
                    )?;
                    Paint::Image(Cow::Owned(Image::from_frame(&inner)))
                }
                ResolvedSource::Video { .. } => {
                    return Err(RenderError::Unsupported {
                        what: "video decoding".to_owned(),
                        path: clip.path.clone(),
                    });
                }
                ResolvedSource::Text(_) => {
                    return Err(RenderError::Unsupported {
                        what: "text rendering".to_owned(),
                        path: clip.path.clone(),
                    });
                }
            };
            let Some(placement) = Placement::new(width, height, clip, local, paint.size()) else {
                continue;
            };
            draw(&mut frame, &paint, &placement, opacity as f32, clip.blend);
        }
        Ok(frame)
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

/// The affine mapping from a clip's box to the output frame, and its inverse.
struct Placement {
    /// Anchor in box coordinates.
    anchor: [f64; 2],
    /// Anchor position in output coordinates.
    position: [f64; 2],
    /// Combined fit and user scale per axis.
    scale: [f64; 2],
    cos: f64,
    sin: f64,
    /// Output-space bounding box `[x0, y0, x1, y1]`, clamped to the frame.
    bounds: [u32; 4],
    /// True when box pixels map one-to-one onto output pixels.
    pixel_aligned: bool,
}

impl Placement {
    fn new(
        frame_w: u32,
        frame_h: u32,
        clip: &ResolvedClip,
        local: f64,
        (w, h): (f64, f64),
    ) -> Option<Self> {
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
        let anchor = [clip.anchor.x.to_px(w), clip.anchor.y.to_px(h)];
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
            forward([0.0, 0.0]),
            forward([w, 0.0]),
            forward([0.0, h]),
            forward([w, h]),
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
            && integer_offset(position[1] - anchor[1]);
        Some(Self {
            anchor,
            position,
            scale,
            cos,
            sin,
            bounds,
            pixel_aligned,
        })
    }

    /// Maps an output point back into box coordinates.
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

fn draw(frame: &mut Frame, paint: &Paint, place: &Placement, opacity: f32, blend: BlendMode) {
    let [x0, y0, x1, y1] = place.bounds;
    for y in y0..y1 {
        for x in x0..x1 {
            let src = if place.pixel_aligned {
                let (u, v) = place.inverse(f64::from(x) + 0.5, f64::from(y) + 0.5);
                paint.sample(u, v)
            } else {
                let mut acc = LinearRgba::TRANSPARENT;
                for (ox, oy) in SUBSAMPLES {
                    let (u, v) = place.inverse(f64::from(x) + ox, f64::from(y) + oy);
                    let s = paint.sample(u, v);
                    acc.r += s.r;
                    acc.g += s.g;
                    acc.b += s.b;
                    acc.a += s.a;
                }
                acc.scaled(1.0 / SUBSAMPLES.len() as f32)
            };
            if src.a <= 0.0 {
                continue;
            }
            let src = src.scaled(opacity);
            let dst = frame.get(x, y);
            frame.set(x, y, composite(src, dst, blend));
        }
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
    fn unsupported_sources_fail_with_the_clip_path() {
        let loaded = load(&doc(
            r##""layers":[{"clips":[{"source":{"kind":"text","text":"hi"}}]}]"##,
        ));
        let comp = loaded.composition.unwrap();
        let err = CpuRenderer::new(NoAssets)
            .render_frame(&comp, Ratio::ZERO)
            .unwrap_err();
        assert_eq!(err.code(), "E500");
        assert!(err.to_string().contains("/layers/0/clips/0"));
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
