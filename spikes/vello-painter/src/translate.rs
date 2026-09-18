//! The markup display list as a `vello_cpu` scene.
//!
//! One box at a time, in paint order: shadows, background, borders,
//! clips. What the painter in `geneva-render` does with loops over
//! pixels, this does with `vello_cpu` calls, so the two can be held
//! against each other pixel for pixel and second for second.
//!
//! Only what the box scenes of this spike use is here. Text, images and
//! group compositing are the parts the spike does not answer from this
//! file; `main` says which.

use geneva_color::Color;
use geneva_html::layout::{Painted, Rectangle};
use geneva_html::style::{Background, Direction, Stop};
use vello_cpu::kurbo::{Affine, BezPath, Point, Rect, RoundedRect, RoundedRectRadii, Shape};
use vello_cpu::peniko::color::{AlphaColor, Srgb};
use vello_cpu::peniko::{
    ColorStop, ColorStops, Extend, Gradient, GradientKind, LinearGradientPosition,
    RadialGradientPosition,
};
use vello_cpu::{PaintType, RenderContext};

/// A colour as vello takes it: sRGB with straight alpha, which is the
/// same form [`Color`] is in, so nothing is converted.
fn color(c: Color) -> AlphaColor<Srgb> {
    AlphaColor::new([c.r, c.g, c.b, c.a])
}

/// A rounded rectangle with a radius per corner, which is what CSS
/// gives and what `RoundedRect` takes.
fn rounded(rect: Rectangle, radius: [f64; 4]) -> RoundedRect {
    let [x, y, w, h] = rect.map(f64::from);
    let [tl, tr, br, bl] = radius;
    RoundedRect::new(
        x,
        y,
        x + w,
        y + h,
        RoundedRectRadii {
            top_left: tl,
            top_right: tr,
            bottom_right: br,
            bottom_left: bl,
        },
    )
}

/// The stops of a CSS gradient, with the ones that name no position
/// spaced evenly between those that do, as the cascade leaves them.
fn stops(list: &[Stop]) -> ColorStops {
    let mut at: Vec<f64> = list.iter().map(|s| s.at.unwrap_or(f64::NAN)).collect();
    if let Some(first) = at.first_mut()
        && first.is_nan()
    {
        *first = 0.0;
    }
    if let Some(last) = at.last_mut()
        && last.is_nan()
    {
        *last = 1.0;
    }
    let mut i = 0;
    while i < at.len() {
        if at[i].is_nan() {
            let start = i - 1;
            let mut end = i;
            while at[end].is_nan() {
                end += 1;
            }
            let span = at[end] - at[start];
            for (k, j) in (start + 1..end).enumerate() {
                at[j] = at[start] + span * (k + 1) as f64 / (end - start) as f64;
            }
        }
        i += 1;
    }
    ColorStops(
        list.iter()
            .zip(at)
            .map(|(s, offset)| ColorStop {
                offset: offset as f32,
                color: color(s.color).into(),
            })
            .collect(),
    )
}

/// Where a CSS linear gradient's line starts and ends on a box, which
/// is the angle carried to the box's corners, not to its edges.
fn linear_line(rect: Rectangle, direction: Direction) -> (Point, Point) {
    let [x, y, w, h] = rect.map(f64::from);
    let angle = match direction {
        Direction::Angle(deg) => deg.to_radians(),
        // A corner's angle is the box's diagonal, so the line is
        // perpendicular to the opposite corner: worked out here as CSS
        // does, from the box's proportions.
        Direction::Corner { top, right } => {
            let a = (w / h).atan();
            match (top, right) {
                (true, true) => a,
                (false, true) => std::f64::consts::PI - a,
                (false, false) => std::f64::consts::PI + a,
                (true, false) => -a,
            }
        }
    };
    // CSS: 0deg points up, angles run clockwise, and the line is long
    // enough that its ends sit where the corners project onto it.
    let (sin, cos) = (angle.sin(), -angle.cos());
    let half = (w * sin.abs() + h * cos.abs()) / 2.0;
    let centre = Point::new(x + w / 2.0, y + h / 2.0);
    (
        Point::new(centre.x - sin * half, centre.y - cos * half),
        Point::new(centre.x + sin * half, centre.y + cos * half),
    )
}

/// Sets the context's paint to a box's background.
fn set_background(ctx: &mut RenderContext, rect: Rectangle, background: &Background) {
    let [x, y, w, h] = rect.map(f64::from);
    match background {
        Background::Color(c) => ctx.set_paint(PaintType::Solid(color(*c))),
        Background::Linear {
            direction,
            stops: s,
        } => {
            let (start, end) = linear_line(rect, *direction);
            ctx.set_paint(PaintType::Gradient(Gradient {
                kind: GradientKind::Linear(LinearGradientPosition { start, end }),
                stops: stops(s),
                extend: Extend::Pad,
                ..Default::default()
            }));
        }
        Background::Radial {
            circle,
            at,
            stops: s,
        } => {
            let centre = Point::new(x + at.0 * w, y + at.1 * h);
            // CSS `farthest-corner` is the default: the ramp ends at the
            // corner furthest from the centre.
            let dx = (centre.x - x).max(x + w - centre.x);
            let dy = (centre.y - y).max(y + h - centre.y);
            let radius = if *circle {
                dx.hypot(dy)
            } else {
                // An ellipse fitted to the box is a circle scaled, which
                // vello takes as a transform on the paint rather than as
                // a radius per axis.
                dx.max(dy)
            };
            let mut gradient = Gradient {
                kind: GradientKind::Radial(RadialGradientPosition {
                    start_center: centre,
                    start_radius: 0.0,
                    end_center: centre,
                    end_radius: radius as f32,
                }),
                stops: stops(s),
                extend: Extend::Pad,
                ..Default::default()
            };
            if !*circle && dx > 0.0 && dy > 0.0 {
                // Scale about the centre so the circle becomes the
                // ellipse the box asks for.
                let (sx, sy) = (dx / radius, dy / radius);
                ctx.set_paint_transform(
                    Affine::translate((centre.x, centre.y))
                        * Affine::scale_non_uniform(sx, sy)
                        * Affine::translate((-centre.x, -centre.y)),
                );
                gradient.kind = GradientKind::Radial(RadialGradientPosition {
                    start_center: centre,
                    start_radius: 0.0,
                    end_center: centre,
                    end_radius: radius as f32,
                });
            }
            ctx.set_paint(PaintType::Gradient(gradient));
        }
    }
}

/// The ring between a box's border box and its padding box, which is
/// what the border draws in. One path with the outer rounded rectangle
/// and the inner one reversed, filled even-odd.
fn border_ring(b: &Painted) -> BezPath {
    let outer = rounded(b.rect, b.paint.radius).to_path(0.1);
    let [top, right, bottom, left] = b.border.map(f64::from);
    let [x, y, w, h] = b.rect.map(f64::from);
    let inner_rect = [
        (x + left) as f32,
        (y + top) as f32,
        (w - left - right).max(0.0) as f32,
        (h - top - bottom).max(0.0) as f32,
    ];
    // CSS shrinks each radius by the border beside it.
    let r = b.paint.radius;
    let inner_radii = [
        (r[0] - left.max(top)).max(0.0),
        (r[1] - right.max(top)).max(0.0),
        (r[2] - right.max(bottom)).max(0.0),
        (r[3] - left.max(bottom)).max(0.0),
    ];
    let mut path = outer;
    path.extend(rounded(inner_rect, inner_radii).to_path(0.1));
    path
}

/// Opens a group's layers: its polygon clip, the clip an ancestor
/// imposes, its opacity and its blur, in the order the painter applies
/// them. Returns how many layers were pushed.
pub fn open_group(ctx: &mut RenderContext, group: &geneva_html::Group) -> usize {
    let mut pushed = 0;
    if let Some(points) = &group.clip_path {
        let mut path = BezPath::new();
        for (i, (x, y)) in points.iter().enumerate() {
            if i == 0 {
                path.move_to((*x, *y));
            } else {
                path.line_to((*x, *y));
            }
        }
        path.close_path();
        ctx.push_clip_layer(&path);
        pushed += 1;
    }
    if let Some((rect, radius)) = group.clip {
        ctx.push_clip_layer(&rounded(rect, radius).to_path(0.1));
        pushed += 1;
    }
    if group.blur > 0.0 {
        ctx.push_filter_layer(vello_common::filter_effects::Filter::from_function(
            vello_common::filter_effects::FilterFunction::Blur {
                radius: group.blur as f32,
            },
        ));
        pushed += 1;
    }
    if group.opacity < 1.0 {
        ctx.push_opacity_layer(group.opacity);
        pushed += 1;
    }
    pushed
}

/// Adds one box of the display list to the scene.
///
/// Returns what it could not draw, so the spike reports the gaps rather
/// than showing a picture that quietly leaves something out.
pub fn box_into(ctx: &mut RenderContext, b: &Painted) -> Vec<&'static str> {
    let mut missing = Vec::new();
    let mut clips = 0;
    if let Some((rect, radius)) = b.clip {
        ctx.push_clip_layer(&rounded(rect, radius).to_path(0.1));
        clips += 1;
    }
    if b.opacity < 1.0 {
        ctx.push_opacity_layer(b.opacity);
        clips += 1;
    }
    // Shadows first, behind the box, furthest back last as CSS lists
    // them front to back.
    for shadow in b.paint.shadow.iter().rev() {
        let [x, y, w, h] = b.rect.map(f64::from);
        let rect = Rect::new(
            x + shadow.x,
            y + shadow.y,
            x + shadow.x + w,
            y + shadow.y + h,
        );
        ctx.set_paint(PaintType::Solid(color(shadow.color)));
        // One radius per shadow, where CSS and our painter carry four.
        let radius = b.paint.radius.iter().sum::<f64>() / 4.0;
        if b.paint.radius.iter().any(|r| (r - radius).abs() > 0.5) {
            missing.push("box-shadow with a radius per corner");
        }
        // CSS blur radius is twice the standard deviation.
        ctx.fill_blurred_rounded_rect(&rect, radius as f32, (shadow.blur / 2.0) as f32, false);
    }
    if let Some(background) = &b.paint.background
        && background.visible()
    {
        set_background(ctx, b.rect, background);
        ctx.fill_path(&rounded(b.rect, b.paint.radius).to_path(0.1));
        ctx.reset_paint_transform();
    }
    if b.border.iter().any(|w| *w > 0.0) {
        let sides = b.paint.border_color;
        let same = sides.iter().all(|c| *c == sides[0]);
        if same {
            if sides[0].a > 0.0 {
                ctx.set_paint(PaintType::Solid(color(sides[0])));
                ctx.set_fill_rule(vello_cpu::peniko::Fill::EvenOdd);
                ctx.fill_path(&border_ring(b));
                ctx.set_fill_rule(vello_cpu::peniko::Fill::NonZero);
            }
        } else {
            // Four sides in four colours meet on the box's diagonals. Our
            // painter draws that; this spike does not.
            missing.push("a border with a colour per side");
        }
    }
    match &b.content {
        geneva_html::layout::Content::Empty => {}
        geneva_html::layout::Content::Text { .. } => missing.push("text"),
        geneva_html::layout::Content::Image { .. } => missing.push("an image"),
    }
    for _ in 0..clips {
        ctx.pop_layer();
    }
    missing
}
