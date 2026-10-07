//! The HTML source, end to end: markup and CSS in, pixels out.

use geneva_color::LinearRgba;
use geneva_render::{CpuRenderer, NoAssets, Renderer};
use geneva_timeline::{Ratio, load};

fn frame(body: &str) -> geneva_render::Frame {
    let text = format!(
        r#"{{"geneva":"1.0","output":{{"width":200,"height":100,"fps":30,"duration":"1s",
        "background":"transparent"}},{body}}}"#
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let comp = l.composition.unwrap();
    CpuRenderer::new(NoAssets)
        .render_frame(&comp, Ratio::ZERO)
        .unwrap()
}

fn at(f: &geneva_render::Frame, x: u32, y: u32) -> LinearRgba {
    f.get(x, y)
}

/// A colour is close enough when every channel is within a hair.
fn near(a: LinearRgba, r: f32, g: f32, b: f32, alpha: f32) -> bool {
    (a.r - r).abs() < 0.02
        && (a.g - g).abs() < 0.02
        && (a.b - b).abs() < 0.02
        && (a.a - alpha).abs() < 0.02
}

#[test]
fn a_flex_row_places_two_boxes_side_by_side() {
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<div class='row'><div class='a'></div><div class='b'></div></div>",
        "css":".row { display: flex; height: 100% } .a { flex: 1; background: #ff0000 } .b { flex: 1; background: #0000ff }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    assert!(
        near(at(&f, 50, 50), 1.0, 0.0, 0.0, 1.0),
        "{:?}",
        at(&f, 50, 50)
    );
    assert!(
        near(at(&f, 150, 50), 0.0, 0.0, 1.0, 1.0),
        "{:?}",
        at(&f, 150, 50)
    );
}

#[test]
fn a_box_with_no_height_fits_its_content() {
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,
        "html":"<div class='p'></div>","css":".p { height: 30px; background: #00ff00 }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    // The box is 30 tall, so the row below it is untouched.
    assert!(near(at(&f, 100, 15), 0.0, 1.0, 0.0, 1.0));
    assert!(near(at(&f, 100, 40), 0.0, 0.0, 0.0, 0.0));
}

#[test]
fn padding_border_and_radius_draw_where_css_says() {
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<div class='c'></div>",
        "css":".c { height: 100%; background: #202020; border-left: 10px solid #00ff00 }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    assert!(
        near(at(&f, 5, 50), 0.0, 1.0, 0.0, 1.0),
        "{:?}",
        at(&f, 5, 50)
    );
    let inside = at(&f, 100, 50);
    assert!(inside.g < 0.1 && inside.a > 0.9, "{inside:?}");
}

#[test]
fn text_is_drawn_inside_the_box() {
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<p>HHHHHH</p>","css":"p { margin: 0; font: 700 40px Liberation Sans; color: #ffffff }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    let lit = (0..200)
        .flat_map(|x| (0..60).map(move |y| (x, y)))
        .filter(|(x, y)| at(&f, *x, *y).a > 0.5)
        .count();
    assert!(lit > 200, "expected glyphs, got {lit} lit pixels");
}

#[test]
fn a_bad_document_is_a_diagnostic_not_a_panic() {
    let text = r#"{"geneva":"1.0","output":{"width":200,"height":100,"fps":30,"duration":"1s"},
        "layers":[{"clips":[{"source":{"kind":"html","html":"<div><p>oops</div>"}}]}]}"#;
    let l = load(text);
    assert!(!l.is_ok());
    let d = l.diagnostics.iter().find(|d| d.code == "E451").unwrap();
    assert!(d.message.contains("closes"), "{}", d.message);
}

#[test]
fn an_unsupported_property_is_a_warning_and_the_rest_still_draws() {
    let text = r#"{"geneva":"1.0","output":{"width":200,"height":100,"fps":30,"duration":"1s"},
        "layers":[{"clips":[{"source":{"kind":"html","html":"<div class='a'></div>",
        "css":".a { float: left; height: 10px; background: red }"}}]}]}"#;
    let l = load(text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    assert!(l.diagnostics.iter().any(|d| d.code == "W450"));
}

#[test]
fn a_linear_gradient_runs_the_way_its_angle_points() {
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<div class='g'></div>",
        "css":".g { height: 100%; background: linear-gradient(to right, #ff0000, #0000ff) }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    let left = at(&f, 2, 50);
    let right = at(&f, 197, 50);
    assert!(left.r > 0.9 && left.b < 0.1, "{left:?}");
    assert!(right.b > 0.9 && right.r < 0.1, "{right:?}");
    // A browser's midpoint is sRGB 0.5 on each end's channel, which is
    // 0.214 once the box is in linear light.
    let middle = at(&f, 100, 50);
    assert!(
        (middle.r - 0.214).abs() < 0.03 && (middle.b - 0.214).abs() < 0.03,
        "the middle mixes both ends: {middle:?}"
    );
}

#[test]
fn a_gradient_interpolates_as_a_browser_does() {
    // Black to white: a browser's midpoint is sRGB 0.5, which is 0.214
    // in the linear light the frame is delivered in.
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<div class='g'></div>","css":".g { width: 200px; height: 100px; background: linear-gradient(90deg, #000000, #ffffff) }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    let mid = at(&f, 100, 50);
    assert!((mid.r - 0.214).abs() < 0.03, "{mid:?}");
}

#[test]
fn a_translucent_box_blends_as_a_browser_does() {
    // 30% teal over the dark ground: a browser gives 0.3 * 238 + 0.7 * 16
    // = 83 on the green channel. Blended in linear light it would be
    // 139.
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<div class='ground'><div class='haze'></div></div>",
        "css":".ground { position: relative; width: 200px; height: 100px; background: #07100C } .haze { position: absolute; inset: 0; background: rgba(0, 238, 225, 0.3) }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    let p = at(&f, 100, 50);
    let g = geneva_color::Transfer::Srgb.from_linear(f64::from(p.g)) * 255.0;
    assert!((g - 83.0).abs() < 2.0, "green {g}, pixel {p:?}");
}

#[test]
fn a_radial_gradient_is_brightest_at_its_centre() {
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<div class='g'></div>",
        "css":".g { height: 100%; background: radial-gradient(circle at 50% 50%, #ffffff, #000000) }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    let centre = at(&f, 100, 50);
    let corner = at(&f, 3, 3);
    assert!(centre.r > 0.9, "{centre:?}");
    assert!(corner.r < 0.1, "{corner:?}");
}

#[test]
fn a_text_shadow_does_not_move_the_text() {
    // The engine pads the rendered image to make room for a shadow. If
    // that padding reached layout, adding a glow would shift the words.
    let ink = |css: &str| {
        let f = frame(&format!(
            r#""layers":[{{"clips":[{{"source":{{"kind":"html","width":200,"height":100,
            "html":"<p>Hi</p>","css":"{css}"}},
            "transform":{{"anchor":"top left","position":"0 0"}}}}]}}]"#
        ));
        let mut box_ = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..100 {
            for x in 0..200 {
                // The glyphs are the only fully opaque thing; a blurred
                // shadow never reaches this.
                if at(&f, x, y).a > 0.95 {
                    box_ = (box_.0.min(x), box_.1.min(y), box_.2.max(x), box_.3.max(y));
                }
            }
        }
        box_
    };
    let plain = ink("p { margin: 0; font: 700 40px Liberation Sans; color: #ffffff }");
    let glow = ink(
        "p { margin: 0; font: 700 40px Liberation Sans; color: #ffffff; text-shadow: 0 0 12px #00ff00 }",
    );
    assert_ne!(plain, (u32::MAX, u32::MAX, 0, 0), "nothing was drawn");
    assert_eq!(plain, glow, "the glyphs moved when a shadow was added");
}

#[test]
fn a_background_clipped_to_text_fills_the_glyphs_and_not_the_box() {
    let f = frame(
        r#""layers":[{"clips":[{"source":{"kind":"html","width":200,"height":100,
        "html":"<p>HHHHH</p>",
        "css":"p { margin: 0; font: 700 60px Liberation Sans; color: #00ff00; background: linear-gradient(90deg, #ff0000, #0000ff); -webkit-background-clip: text; background-clip: text; -webkit-text-fill-color: transparent }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]"#,
    );
    let mut reds = 0;
    let mut blues = 0;
    let mut greens = 0;
    let mut opaque = 0;
    for y in 0..100 {
        for x in 0..200 {
            let p = at(&f, x, y);
            if p.a > 0.95 {
                opaque += 1;
                if p.r > 0.7 && p.b < 0.3 {
                    reds += 1;
                }
                if p.b > 0.7 && p.r < 0.3 {
                    blues += 1;
                }
                if p.g > 0.3 {
                    greens += 1;
                }
            }
        }
    }
    assert!(opaque > 0, "the glyphs were drawn");
    // The line box would be fully opaque if the box were painted; glyphs
    // cover far less of it.
    assert!(
        opaque < 200 * 60 / 2,
        "the box itself is not painted ({opaque} px)"
    );
    assert!(
        reds > 0 && blues > 0,
        "red at one end, blue at the other ({reds}, {blues})"
    );
    assert_eq!(greens, 0, "the colour gives way to the fill");
}

/// A frame of a markup clip at `tenths` of a second, on a 200 by 100
/// transparent output two seconds long.
fn markup_at(html: &str, css: &str, tenths: i64) -> geneva_render::Frame {
    let text = format!(
        r#"{{"geneva":"1.0","output":{{"width":200,"height":100,"fps":30,"duration":"2s",
        "background":"transparent"}},"layers":[{{"clips":[{{"source":{{"kind":"html","width":200,"height":100,
        "html":{html},"css":{css}}},"duration":"2s",
        "transform":{{"anchor":"top left","position":"0 0"}}}}]}}]}}"#,
        html = serde_json::to_string(html).unwrap(),
        css = serde_json::to_string(css).unwrap(),
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let comp = l.composition.unwrap();
    CpuRenderer::new(NoAssets)
        .render_frame(&comp, Ratio::new(tenths, 10))
        .unwrap()
}

#[test]
fn a_half_transparent_box_is_composited_as_one_picture() {
    // Two children of a box at half opacity overlap. Grouped, the
    // overlap is the top child at half strength over nothing; multiplied
    // down, the lower child would show through it.
    let f = markup_at(
        "<div class='p'><div class='a'></div><div class='b'></div></div>",
        ".p { opacity: 0.5; position: relative; width: 200px; height: 100px } \
         .a { position: absolute; left: 0; top: 0; width: 100px; height: 100px; background: #ff0000 } \
         .b { position: absolute; left: 50px; top: 0; width: 100px; height: 100px; background: #0000ff }",
        0,
    );
    let p = at(&f, 75, 50);
    assert!(
        near(p, 0.0, 0.0, 0.5, 0.5),
        "blue at half, no red beneath: {p:?}"
    );
}

#[test]
fn an_animation_inside_the_markup_moves_its_element() {
    let f = |tenths| {
        markup_at(
            "<div class='stage'><div class='dot'></div></div>",
            "@keyframes go { to { transform: translateX(100px) } } \
             .stage { position: relative; width: 200px; height: 100px } \
             .dot { position: absolute; left: 0; top: 0; width: 50px; height: 100px; background: #00ff00; \
                    animation: go 1s linear forwards }",
            tenths,
        )
    };
    assert!(
        near(at(&f(0), 25, 50), 0.0, 1.0, 0.0, 1.0),
        "at the start it is on the left"
    );
    assert!(
        near(at(&f(5), 75, 50), 0.0, 1.0, 0.0, 1.0),
        "halfway it has moved 50px"
    );
    assert!(near(at(&f(5), 25, 50), 0.0, 0.0, 0.0, 0.0));
    assert!(
        near(at(&f(15), 125, 50), 0.0, 1.0, 0.0, 1.0),
        "and it holds at the end"
    );
}

#[test]
fn a_fade_in_with_a_delay_longhand_starts_from_the_style() {
    let f = |tenths| {
        markup_at(
            "<div class='stage'><div class='box'></div></div>",
            "@keyframes show { to { opacity: 1 } } \
             .stage { position: relative; width: 200px; height: 100px } \
             .box { position: absolute; inset: 0; background: #ff0000; opacity: 0; \
                    animation: show 1s linear forwards; animation-delay: 0.5s }",
            tenths,
        )
    };
    assert!(
        near(at(&f(2), 100, 50), 0.0, 0.0, 0.0, 0.0),
        "nothing before the delay"
    );
    assert!(
        near(at(&f(10), 100, 50), 0.5, 0.0, 0.0, 0.5),
        "halfway up at one second"
    );
    assert!(
        near(at(&f(19), 100, 50), 1.0, 0.0, 0.0, 1.0),
        "held once done"
    );
}

#[test]
fn a_width_animation_lays_the_document_out_again() {
    // A box growing from nothing pushes its sibling along, as it does
    // in a browser: layout runs at each frame.
    let f = |tenths| {
        markup_at(
            "<div class='row'><div class='a'></div><div class='b'></div></div>",
            "@keyframes open { to { width: 100px } } \
             .row { display: flex; width: 200px; height: 100px } \
             .a { width: 0; height: 100px; background: #ff0000; animation: open 1s linear forwards } \
             .b { width: 50px; height: 100px; background: #0000ff }",
            tenths,
        )
    };
    assert!(
        near(at(&f(0), 25, 50), 0.0, 0.0, 1.0, 1.0),
        "blue starts at the left edge"
    );
    assert!(
        near(at(&f(5), 25, 50), 1.0, 0.0, 0.0, 1.0),
        "red has grown to 50px"
    );
    assert!(
        near(at(&f(5), 75, 50), 0.0, 0.0, 1.0, 1.0),
        "and pushed blue along"
    );
}

#[test]
fn a_blur_filter_softens_the_group() {
    let sharp = markup_at(
        "<div class='stage'><div class='box'></div></div>",
        ".stage { position: relative; width: 200px; height: 100px } \
         .box { position: absolute; left: 50px; top: 25px; width: 100px; height: 50px; background: #ffffff }",
        0,
    );
    let soft = markup_at(
        "<div class='stage'><div class='box'></div></div>",
        ".stage { position: relative; width: 200px; height: 100px } \
         .box { position: absolute; left: 50px; top: 25px; width: 100px; height: 50px; background: #ffffff; filter: blur(6px) }",
        0,
    );
    // Just outside the box: nothing sharp, something soft. Inside near
    // the edge: full sharp, less soft.
    assert!(at(&sharp, 45, 50).a < 0.01);
    assert!(at(&soft, 45, 50).a > 0.05, "{:?}", at(&soft, 45, 50));
    assert!(at(&sharp, 52, 50).a > 0.99);
    assert!(at(&soft, 52, 50).a < 0.9, "{:?}", at(&soft, 52, 50));
}

#[test]
fn a_colour_animation_reaches_the_text() {
    let f = |tenths| {
        markup_at(
            "<div><p>HHHH</p></div>",
            "@keyframes tint { from { color: #ff0000 } to { color: #0000ff } } \
             p { margin: 0; font: 700 60px Liberation Sans; color: #ffffff; animation: tint 1s linear forwards }",
            tenths,
        )
    };
    let dominant = |frame: &geneva_render::Frame| {
        let (mut r, mut b) = (0.0f32, 0.0f32);
        for y in 0..100 {
            for x in 0..200 {
                let p = at(frame, x, y);
                if p.a > 0.95 {
                    r += p.r;
                    b += p.b;
                }
            }
        }
        (r, b)
    };
    let (r0, b0) = dominant(&f(0));
    let (r1, b1) = dominant(&f(10));
    assert!(r0 > b0 * 10.0, "red at the start: {r0} {b0}");
    assert!(b1 > r1 * 10.0, "blue at the end: {r1} {b1}");
}

#[test]
fn a_clip_path_polygon_keeps_only_what_is_inside_it() {
    // A white box clipped to its left half, by a diagonal so that a row
    // near the top and one near the bottom disagree about where the edge
    // is.
    let f = markup_at(
        "<div class='stage'><div class='box'></div></div>",
        ".stage { position: relative; width: 200px; height: 100px } \
         .box { position: absolute; inset: 0; background: #ffffff; \
                clip-path: polygon(0 0, 50% 0, 100% 100%, 0 100%) }",
        0,
    );
    assert!(at(&f, 20, 50).a > 0.99, "well inside");
    assert!(at(&f, 180, 10).a < 0.01, "outside, top right");
    assert!(at(&f, 120, 10).a < 0.01, "right of the edge near the top");
    assert!(at(&f, 120, 90).a > 0.99, "left of the edge near the bottom");
    // On the edge itself the coverage is partial: anti-aliased, not a
    // staircase.
    let edge = at(&f, 150, 50).a;
    assert!(edge > 0.05 && edge < 0.95, "{edge}");
}

#[test]
fn an_animated_clip_path_rolls_the_box_away() {
    let f = |tenths| {
        markup_at(
            "<div class='stage'><div class='sheet'></div></div>",
            "@keyframes roll { to { clip-path: polygon(0 0, 100% 0, 100% 0, 0 0) } } \
             .stage { position: relative; width: 200px; height: 100px } \
             .sheet { position: absolute; inset: 0; background: #ffffff; \
                      clip-path: polygon(0 0, 100% 0, 100% 100%, 0 100%); \
                      animation: roll 1s linear forwards }",
            tenths,
        )
    };
    assert!(at(&f(0), 100, 90).a > 0.99, "whole at the start");
    assert!(
        at(&f(5), 100, 25).a > 0.99,
        "halfway, the top half is still there"
    );
    assert!(at(&f(5), 100, 75).a < 0.01, "and the bottom half has gone");
    assert!(at(&f(15), 100, 5).a < 0.01, "nothing left at the end");
}

#[test]
fn a_shadow_list_draws_every_shadow() {
    // A red shadow to the left and a blue one to the right of a box, and
    // the same two on text: both colours land where they are sent.
    let f = markup_at(
        "<div class='stage'><div class='box'></div><p>H</p></div>",
        ".stage { position: relative; width: 200px; height: 100px } \
         .box { position: absolute; left: 30px; top: 10px; width: 40px; height: 30px; background: #ffffff; \
                box-shadow: -20px 0 0 #ff0000, 20px 0 0 #0000ff } \
         p { position: absolute; left: 110px; top: 40px; margin: 0; font: 700 40px Liberation Sans; color: #ffffff; \
             text-shadow: -20px 0 0 #ff0000, 20px 0 0 #0000ff }",
        0,
    );
    assert!(
        near(at(&f, 15, 25), 1.0, 0.0, 0.0, 1.0),
        "red box shadow: {:?}",
        at(&f, 15, 25)
    );
    assert!(
        near(at(&f, 85, 25), 0.0, 0.0, 1.0, 1.0),
        "blue box shadow: {:?}",
        at(&f, 85, 25)
    );
    let (mut red, mut blue) = (0, 0);
    for y in 40..100 {
        for x in 100..200 {
            let p = at(&f, x, y);
            if p.a > 0.9 && p.r > 0.8 && p.b < 0.2 {
                red += 1;
            }
            if p.a > 0.9 && p.b > 0.8 && p.r < 0.2 {
                blue += 1;
            }
        }
    }
    assert!(
        red > 50 && blue > 50,
        "both text shadows drawn ({red}, {blue})"
    );
}

#[test]
fn a_blurred_shadow_of_a_thin_bar_is_faint() {
    // A 4 px bar with a 20 px blur: the Gaussian spreads the bar's
    // little mass, so even at its centre the shadow is well under full
    // strength, and it has all but gone 20 px out.
    let f = markup_at(
        "<div class='stage'><div class='bar'></div></div>",
        ".stage { position: relative; width: 200px; height: 100px } \
         .bar { position: absolute; left: 98px; top: 10px; width: 4px; height: 80px; \
                box-shadow: 0 0 20px #ffffff }",
        0,
    );
    // Beside the bar, on its centre line.
    let beside = at(&f, 103, 50).a;
    assert!(beside > 0.05 && beside < 0.35, "{beside}");
    assert!(at(&f, 125, 50).a < 0.02, "{}", at(&f, 125, 50).a);
}

/// Frames of one markup clip at each of `tenths`, all through one
/// renderer, so that what the painter keeps between frames is used.
fn markup_run(html: &str, css: &str, tenths: &[i64]) -> Vec<geneva_render::Frame> {
    let text = format!(
        r#"{{"geneva":"1.0","output":{{"width":200,"height":100,"fps":30,"duration":"2s",
        "background":"transparent"}},"layers":[{{"clips":[{{"source":{{"kind":"html","width":200,"height":100,
        "html":{html},"css":{css}}},"duration":"2s",
        "transform":{{"anchor":"top left","position":"0 0"}}}}]}}]}}"#,
        html = serde_json::to_string(html).unwrap(),
        css = serde_json::to_string(css).unwrap(),
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let comp = l.composition.unwrap();
    let mut renderer = CpuRenderer::new(NoAssets);
    tenths
        .iter()
        .map(|t| renderer.render_frame(&comp, Ratio::new(*t, 10)).unwrap())
        .collect()
}

/// The largest difference in any channel between two frames.
fn furthest(a: &geneva_render::Frame, b: &geneva_render::Frame) -> f32 {
    let mut worst = 0.0f32;
    for y in 0..a.height() {
        for x in 0..a.width() {
            let (p, q) = (a.get(x, y), b.get(x, y));
            for d in [p.r - q.r, p.g - q.g, p.b - q.b, p.a - q.a] {
                worst = worst.max(d.abs());
            }
        }
    }
    worst
}

#[test]
fn a_panel_wiped_in_over_another_is_all_that_shows() {
    // Once the second panel's wipe is open it covers the frame in an
    // opaque colour, and the painter starts from it: the first panel and
    // its moving word are not painted at all. The frame is what the
    // second panel alone draws.
    let css = ".stage { position: absolute; inset: 0; overflow: hidden; background: #101010 } \
         .panel { position: absolute; inset: 0 } \
         .p1 { background: #ff0000 } \
         .word { position: absolute; left: 20px; top: 20px; width: 60px; height: 30px; background: #ffff00; \
                 animation: slide 2s linear forwards } \
         @keyframes slide { to { transform: translateX(80px) } } \
         .p2 { background: #0000ff; clip-path: polygon(0% 0%, 0% 0%, -20% 100%, -20% 100%); \
               animation: wipe .5s linear forwards } \
         @keyframes wipe { to { clip-path: polygon(0% 0%, 120% 0%, 100% 100%, -20% 100%) } } \
         .dot { position: absolute; left: 150px; top: 60px; width: 20px; height: 20px; background: #00ff00; \
                animation: slide 2s linear forwards }";
    let both = markup_at(
        "<div class='stage'><div class='panel p1'><div class='word'></div></div>\
         <div class='panel p2'><div class='dot'></div></div></div>",
        css,
        10,
    );
    let alone = markup_at(
        "<div class='stage'><div class='panel p2'><div class='dot'></div></div></div>",
        css,
        10,
    );
    assert!(
        near(at(&both, 40, 30), 0.0, 0.0, 1.0, 1.0),
        "{:?}",
        at(&both, 40, 30)
    );
    assert!(
        furthest(&both, &alone) < 1e-6,
        "{}",
        furthest(&both, &alone)
    );
    // Before the wipe opens, the second panel is not drawn and the first
    // shows whole.
    let closed = markup_at(
        "<div class='stage'><div class='panel p1'><div class='word'></div></div>\
         <div class='panel p2'><div class='dot'></div></div></div>",
        &css.replace(
            "animation: wipe .5s linear forwards",
            "animation: wipe .5s 1.5s linear forwards",
        ),
        0,
    );
    assert!(
        near(at(&closed, 150, 80), 1.0, 0.0, 0.0, 1.0),
        "{:?}",
        at(&closed, 150, 80)
    );
}

#[test]
fn a_blend_inside_a_group_still_blends_against_the_group_alone() {
    // The group is animated but at rest (opacity one, no transform), so
    // it would be laid down exactly as painted. A child mixed in
    // `multiply` inside it blends against the group's own empty picture,
    // as in a browser, not against the red already on the surface: over
    // an empty backdrop, multiply leaves the child as it is.
    let f = markup_at(
        "<div class='under'></div><div class='g'><div class='m'></div></div>",
        ".under { position: absolute; left: 0; top: 0; width: 200px; height: 100px; background: #ff0000 } \
         .g { position: absolute; left: 0; top: 0; width: 200px; height: 100px; \
              animation: rest 2s linear forwards } \
         @keyframes rest { from { opacity: 1 } to { opacity: 1 } } \
         .m { position: absolute; left: 50px; top: 25px; width: 100px; height: 50px; \
              background: #00ff00; mix-blend-mode: multiply; animation: rest 2s linear forwards }",
        5,
    );
    assert!(
        near(at(&f, 100, 50), 0.0, 1.0, 0.0, 1.0),
        "{:?}",
        at(&f, 100, 50)
    );
    assert!(
        near(at(&f, 10, 10), 1.0, 0.0, 0.0, 1.0),
        "{:?}",
        at(&f, 10, 10)
    );
}

#[test]
fn a_blur_kept_between_frames_draws_what_a_fresh_one_does() {
    // A soft shape drifting under a steady blur is blurred once and the
    // blurred picture kept. The frames that use it match a renderer that
    // starts cold at each of them.
    let html = "<div class='blob'></div><div class='text'>Hi</div>";
    let css = ".blob { position: absolute; left: 40px; top: 20px; width: 60px; height: 60px; \
                border-radius: 30px; background: #ff00ff; filter: blur(8px); \
                mix-blend-mode: screen; animation: drift 2s linear forwards } \
         @keyframes drift { to { transform: translate(60px, 10px) scale(1.2) } } \
         .text { position: absolute; left: 10px; top: 10px; font-size: 20px; color: #ffffff; \
                 animation: fade 1s linear forwards } \
         @keyframes fade { from { opacity: 0.2 } to { opacity: 1 } }";
    let times = [0, 1, 2, 3, 5, 8, 13];
    let run = markup_run(html, css, &times);
    for (t, kept) in times.iter().zip(&run) {
        let cold = markup_at(html, css, *t);
        let d = furthest(kept, &cold);
        assert!(d < 1e-4, "at {t} tenths: {d}");
    }
}

#[test]
fn a_sentence_with_a_bold_word_wraps_as_one() {
    // A paragraph 120 px wide with a word in another colour inside it.
    // It wraps onto several lines and stays inside its box, and the word
    // keeps its colour. Set as separate pieces, it ran off to the right
    // on one line.
    let f = markup_at(
        "<p>a sentence long enough to wrap, with <b>one word</b> in green and more after it</p>",
        "p { position: absolute; left: 10px; top: 10px; width: 120px; margin: 0; \
             font: 16px Liberation Sans; color: #ffffff } \
         b { color: #00ff00 }",
        0,
    );
    let (mut top, mut bottom, mut right, mut green) = (u32::MAX, 0, 0, 0);
    for y in 0..f.height() {
        for x in 0..f.width() {
            let p = at(&f, x, y);
            if p.a > 0.5 {
                top = top.min(y);
                bottom = bottom.max(y);
                right = right.max(x);
                if p.g > 0.5 && p.r < 0.3 {
                    green += 1;
                }
            }
        }
    }
    assert!(bottom - top > 40, "several lines: {top}..{bottom}");
    assert!(right < 135, "inside the box: {right}");
    assert!(green > 20, "the word in green: {green}");
}

#[test]
fn a_smaller_word_leaves_its_line_as_tall_as_the_rest() {
    // Two lines of 40 px text with a 12 px word in the first. The line
    // took its height from the small word alone, so the big glyphs were
    // cut at the top and the second line was drawn over the first. The
    // frame is tall enough for both lines in any face: a machine without
    // Liberation Sans (Windows) draws one whose lines are further apart.
    let text = format!(
        r#"{{"geneva":"1.0","output":{{"width":200,"height":200,"fps":30,"duration":"1s",
        "background":"transparent"}},"layers":[{{"clips":[{{"source":{{"kind":"html","width":200,"height":200,
        "html":{},"css":{}}},"transform":{{"anchor":"top left","position":"0 0"}}}}]}}]}}"#,
        serde_json::to_string("<p>BIG <small>SMALL</small> BIG<br>BIG BIG</p>").unwrap(),
        serde_json::to_string(
            "p { position: absolute; left: 10px; top: 10px; width: 300px; margin: 0; \
                 font: 40px Liberation Sans; color: #ffffff } \
             small { font-size: 12px }"
        )
        .unwrap(),
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let f = CpuRenderer::new(NoAssets)
        .render_frame(&l.composition.unwrap(), Ratio::ZERO)
        .unwrap();
    let inked: Vec<bool> = (0..f.height())
        .map(|y| (0..f.width()).any(|x| at(&f, x, y).a > 0.5))
        .collect();
    let mut bands = Vec::new();
    let mut start = None;
    for (y, ink) in inked.iter().enumerate() {
        match (ink, start) {
            (true, None) => start = Some(y),
            (false, Some(s)) => {
                bands.push((s, y));
                start = None;
            }
            _ => {}
        }
    }
    assert_eq!(bands.len(), 2, "two lines apart: {bands:?}");
    assert!(
        bands[0].1 - bands[0].0 > 26,
        "the first line whole: {bands:?}"
    );
}

#[test]
fn a_text_stroke_draws_and_its_colour_animates() {
    let text = r##"{"geneva":"1.1","output":{"width":200,"height":120,"fps":30,"duration":"1s",
        "background":"transparent"},
        "layers":[{"clips":[{"source":{"kind":"html","width":200,"height":120,
        "html":"<div><p>I</p></div>",
        "css":"p { margin: 0; padding: 10px 40px; font: 700 96px sans-serif; color: #fff; -webkit-text-stroke: 12px #ff0000; animation: s 1s linear both } @keyframes s { to { -webkit-text-stroke-color: #0000ff } }"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]}"##;
    let l = load(text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let comp = l.composition.unwrap();
    let mut renderer = CpuRenderer::new(NoAssets);
    // The first opaque pixel along a row through the stem is the stroke
    // drawn over the glyph's left edge.
    let mut edge = |t: Ratio| {
        let f = renderer.render_frame(&comp, t).unwrap();
        (0..200)
            .map(|x| f.get(x, 60))
            .find(|p| p.a > 0.95)
            .expect("the stroke is drawn")
    };
    let start = edge(Ratio::ZERO);
    assert!(start.r > 0.9 && start.b < 0.05, "{start:?}");
    let end = edge(Ratio::new(29, 30));
    assert!(end.b > 0.8 && end.r < 0.1, "{end:?}");
    let mid = edge(Ratio::new(1, 2));
    assert!(mid.r > 0.1 && mid.b > 0.1, "{mid:?}");
}

/// Markup whose animations only step is drawn once per interval between
/// the moments it can change, and the picture is reused for the frames
/// after it. Every frame of a render going forward must be the frame a
/// fresh renderer draws at that moment.
#[test]
fn stepped_markup_reused_between_steps_matches_fresh_frames() {
    use std::fmt::Write as _;
    let mut spans = String::new();
    for k in 0..6 {
        let delay = 0.05 + 0.13 * f64::from(k);
        write!(
            spans,
            "<span style='animation: lit 0.2s steps(2, jump-end) {delay:.3}s both'>w{k}</span>"
        )
        .unwrap();
    }
    let text = format!(
        r##"{{"geneva":"1.1","output":{{"width":240,"height":60,"fps":30,"duration":"1s","background":"#203040"}},
        "layers":[{{"clips":[{{"source":{{"kind":"html","html":"<style>@keyframes lit {{ from {{ color: #ffffff }} to {{ color: #ffd400 }} }} .c {{ display: flex; gap: 4px; font-size: 18px; color: #ffffff }}</style><div class='c'>{spans}</div>"}}}}]}}]}}"##
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let comp = l.composition.unwrap();
    let mut going = CpuRenderer::new(NoAssets);
    let mut changes = 0;
    let mut previous: Option<Vec<LinearRgba>> = None;
    for n in 0..comp.frame_count() {
        let t = comp.frame_time(n);
        let reused = going.render_frame(&comp, t).unwrap();
        let fresh = CpuRenderer::new(NoAssets).render_frame(&comp, t).unwrap();
        assert!(
            reused.pixels() == fresh.pixels(),
            "frame {n} differs from a fresh render"
        );
        if previous.as_deref().is_some_and(|p| p != reused.pixels()) {
            changes += 1;
        }
        previous = Some(reused.pixels().to_vec());
    }
    assert!(changes >= 6, "the words light up: {changes} changes");
}

#[test]
fn a_mask_wipes_a_box_on_from_the_left() {
    // The gradient's opaque half is shown through a tile 220% wide that
    // slides from the right: hidden at the start, whole at the end, and
    // part way through, the left side shown and the right not yet.
    let f = |tenths| {
        markup_at(
            "<div class='s'><div class='w'></div></div>",
            ".s { position: relative; width: 200px; height: 100px } \
             @keyframes wipe { from { mask-position: 100% 0 } to { mask-position: 0 0 } } \
             .w { position: absolute; left: 0; top: 0; width: 200px; height: 100px; background: #ff0000; \
                  mask-image: linear-gradient(90deg, #000 45%, transparent 55%); \
                  mask-size: 220% 100%; mask-repeat: no-repeat; animation: wipe 1s linear both }",
            tenths,
        )
    };
    assert!(at(&f(0), 20, 50).a < 0.01, "{:?}", at(&f(0), 20, 50));
    let mid = f(5);
    assert!(
        near(at(&mid, 10, 50), 1.0, 0.0, 0.0, 1.0),
        "{:?}",
        at(&mid, 10, 50)
    );
    assert!(at(&mid, 190, 50).a < 0.01, "{:?}", at(&mid, 190, 50));
    assert!(near(at(&f(10), 190, 50), 1.0, 0.0, 0.0, 1.0));
}

#[test]
fn an_underline_glides_from_one_anchor_to_the_next() {
    let f = |tenths| {
        markup_at(
            "<div class='row'><div class='a'></div><div class='b'></div><div class='u'></div></div>",
            "@keyframes glide { from { left: anchor(--a left); width: anchor-size(--a width) } \
                                to { left: anchor(--b left); width: anchor-size(--b width) } } \
             .row { position: absolute; left: 10px; top: 0; display: flex; gap: 20px } \
             .a { anchor-name: --a; width: 40px; height: 20px } \
             .b { anchor-name: --b; width: 80px; height: 20px } \
             .u { position: absolute; top: 30px; left: 0; width: 1px; height: 4px; background: #00ff00; \
                  animation: glide 1s linear both }",
            tenths,
        )
    };
    // At the start it lies under the first box (10..50), at the end under
    // the second (70..150), and halfway it is between: 40..100.
    let start = f(0);
    assert!(
        near(at(&start, 30, 31), 0.0, 1.0, 0.0, 1.0),
        "{:?}",
        at(&start, 30, 31)
    );
    assert!(at(&start, 60, 31).a < 0.01);
    let end = f(10);
    assert!(
        near(at(&end, 140, 31), 0.0, 1.0, 0.0, 1.0),
        "{:?}",
        at(&end, 140, 31)
    );
    assert!(at(&end, 30, 31).a < 0.01);
    let mid = f(5);
    assert!(
        near(at(&mid, 45, 31), 0.0, 1.0, 0.0, 1.0),
        "{:?}",
        at(&mid, 45, 31)
    );
    assert!(
        near(at(&mid, 95, 31), 0.0, 1.0, 0.0, 1.0),
        "{:?}",
        at(&mid, 95, 31)
    );
    assert!(at(&mid, 30, 31).a < 0.01 && at(&mid, 115, 31).a < 0.01);
}

#[test]
fn a_static_transform_moves_the_box() {
    let f = markup_at(
        "<div class='a'></div>",
        ".a { position: absolute; left: 0; top: 0; width: 50px; height: 50px; background: #0000ff; \
              transform: translateX(100px) }",
        0,
    );
    assert!(at(&f, 25, 25).a < 0.01);
    assert!(
        near(at(&f, 125, 25), 0.0, 0.0, 1.0, 1.0),
        "{:?}",
        at(&f, 125, 25)
    );
}

#[test]
fn a_background_colour_and_box_shadow_animate() {
    let f = |tenths| {
        markup_at(
            "<div class='s'><div class='a'></div></div>",
            ".s { position: relative; width: 200px; height: 100px } \
             @keyframes lit { from { background-color: #000000 } to { background-color: #ffffff; box-shadow: 0 0 0 #ff0000, 60px 0 0 #00ff00 } } \
             .a { position: absolute; left: 10px; top: 10px; width: 40px; height: 40px; animation: lit 1s linear both }",
            tenths,
        )
    };
    assert!(near(at(&f(0), 30, 30), 0.0, 0.0, 0.0, 1.0));
    assert!(near(at(&f(10), 30, 30), 1.0, 1.0, 1.0, 1.0));
    // The second shadow, offset to the right, is padded from nothing:
    // transparent at the start, solid at the end.
    assert!(at(&f(0), 90, 30).a < 0.01);
    assert!(
        near(at(&f(10), 90, 30), 0.0, 1.0, 0.0, 1.0),
        "{:?}",
        at(&f(10), 90, 30)
    );
}

#[test]
fn a_right_to_left_row_starts_at_the_right() {
    let f = markup_at(
        "<div class='r' dir='rtl'><div class='a'></div><div class='b'></div></div>",
        ".r { position: absolute; left: 0; top: 0; width: 200px; display: flex } \
         .a { width: 50px; height: 50px; background: #ff0000 } \
         .b { width: 50px; height: 50px; background: #0000ff }",
        0,
    );
    assert!(
        near(at(&f, 175, 25), 1.0, 0.0, 0.0, 1.0),
        "{:?}",
        at(&f, 175, 25)
    );
    assert!(
        near(at(&f, 125, 25), 0.0, 0.0, 1.0, 1.0),
        "{:?}",
        at(&f, 125, 25)
    );
    assert!(at(&f, 25, 25).a < 0.01);
}

#[test]
fn a_reused_picture_keeps_nothing_of_the_last_frame() {
    // One renderer draws a moving box at two times: the second frame is
    // drawn into the first one's buffer, which must come back clear.
    let text = format!(
        r#"{{"geneva":"1.0","output":{{"width":200,"height":100,"fps":30,"duration":"2s",
        "background":"transparent"}},"layers":[{{"clips":[{{"source":{{"kind":"html","width":200,"height":100,
        "html":{html},"css":{css}}},"duration":"2s",
        "transform":{{"anchor":"top left","position":"0 0"}}}}]}}]}}"#,
        html = serde_json::to_string("<div class='stage'><div class='dot'></div></div>").unwrap(),
        css = serde_json::to_string(
            "@keyframes go { to { transform: translateX(100px) } } \
             .stage { position: relative; width: 200px; height: 100px } \
             .dot { position: absolute; left: 0; top: 0; width: 50px; height: 100px; background: #00ff00; \
                    animation: go 1s linear forwards }"
        )
        .unwrap(),
    );
    let comp = load(&text).composition.unwrap();
    let mut r = CpuRenderer::new(NoAssets);
    let first = r.render_frame(&comp, Ratio::ZERO).unwrap();
    assert!(near(at(&first, 25, 50), 0.0, 1.0, 0.0, 1.0));
    let second = r.render_frame(&comp, Ratio::new(1, 1)).unwrap();
    assert!(at(&second, 25, 50).a < 0.01, "{:?}", at(&second, 25, 50));
    assert!(near(at(&second, 125, 50), 0.0, 1.0, 0.0, 1.0));
}

#[test]
fn a_box_wholly_below_the_picture_is_left_out() {
    // The second box starts past the bottom of a 100-pixel picture.
    let f = markup_at(
        "<div class='a'></div><div class='b'>below</div>",
        ".a { height: 150px; background: #ff0000 } .b { height: 40px; background: #0000ff }",
        0,
    );
    assert!(near(at(&f, 100, 50), 1.0, 0.0, 0.0, 1.0));
}

/// A word whose text shadow grows stays where layout puts it: the room
/// left for the shadow changes every frame, the glyphs do not move.
#[test]
fn a_growing_text_shadow_leaves_the_glyphs_still() {
    let frames = markup_run(
        "<div class='c'>one <span class='w'>word</span> two</div>",
        "@keyframes g { from { text-shadow: 0 0 0px rgba(255,226,180,0) } \
                        to { text-shadow: 0 0 12.43px rgba(255,226,180,0.55) } } \
         .c { position: absolute; left: 10px; top: 20px; font: 700 30px Liberation Sans; color: #ffffff } \
         .w { animation: g 2s ease both }",
        &(0..20).collect::<Vec<_>>(),
    );
    // The glyphs' white cores; the shadow is never white.
    let cores = |f: &geneva_render::Frame| -> Vec<bool> {
        (0..f.height())
            .flat_map(|y| (0..f.width()).map(move |x| (x, y)))
            .map(|(x, y)| {
                let p = f.get(x, y);
                p.a > 0.99 && p.b > 0.95
            })
            .collect()
    };
    let first = cores(&frames[0]);
    let count = first.iter().filter(|c| **c).count();
    for (i, f) in frames.iter().enumerate() {
        let moved = cores(f).iter().zip(&first).filter(|(a, b)| a != b).count();
        // Edge pixels the shadow shows through can cross the line; a
        // glyph a pixel off changes a tenth or more.
        assert!(
            moved * 50 <= count,
            "at {i} tenths {moved} of {count} core pixels changed"
        );
    }
}

/// A word in a box sized to it draws on one line, and in the same place,
/// whether it is aligned to the start, the centre or the right; with
/// `white-space: nowrap` and without.
#[test]
fn a_word_in_a_box_as_wide_as_it_stays_on_one_line_however_aligned() {
    use std::fmt::Write as _;
    let (cols, rows) = (4, 21);
    let render = |align: &str, nowrap: bool| {
        let mut html = String::new();
        for i in 0..cols * rows {
            let size = 18.0 + 0.6 * f64::from(i / cols);
            let spacing = [0.0, 0.05, 0.118, 0.2][(i % cols) as usize];
            write!(
                html,
                "<div style='position: absolute; left: {}px; top: {}px; text-align: {align}; {} \
                 font: 700 {size}px Liberation Sans; letter-spacing: {spacing}em; color: #ffffff'>Making</div>",
                10 + (i % cols) * 200,
                10 + (i / cols) * 50,
                if nowrap { "white-space: nowrap;" } else { "" },
            )
            .unwrap();
        }
        let text = format!(
            r#"{{"geneva":"1.0","output":{{"width":800,"height":1060,"fps":30,"duration":"1s",
            "background":"transparent"}},"layers":[{{"clips":[{{"source":{{"kind":"html","width":800,"height":1060,
            "html":{}}},"transform":{{"anchor":"top left","position":"0 0"}}}}]}}]}}"#,
            serde_json::to_string(&html).unwrap()
        );
        let l = load(&text);
        assert!(l.is_ok(), "{:?}", l.diagnostics);
        CpuRenderer::new(NoAssets)
            .render_frame(&l.composition.unwrap(), Ratio::ZERO)
            .unwrap()
    };
    for nowrap in [true, false] {
        let start = render("start", nowrap);
        for i in 0..cols * rows {
            let (x0, y0) = (10 + (i % cols) * 200, 10 + (i / cols) * 50);
            let (mut top, mut bottom) = (u32::MAX, 0);
            for y in y0..y0 + 48 {
                for x in x0..x0 + 195 {
                    if start.get(x, y).a > 0.5 {
                        top = top.min(y);
                        bottom = bottom.max(y);
                    }
                }
            }
            assert!(bottom - top < 32, "cell {i} drew on two lines");
        }
        for align in ["center", "right"] {
            let other = render(align, nowrap);
            assert!(
                furthest(&start, &other) == 0.0,
                "{align} (nowrap {nowrap}) drew the words elsewhere or on two lines"
            );
        }
    }
}

/// `white-space: nowrap` keeps a line whole in a box narrower than it:
/// the line runs past the box rather than breaking.
#[test]
fn nowrap_text_runs_past_a_narrow_box_on_one_line() {
    let height = |css: &str| {
        let f = markup_at(
            "<div class='n'>Making things</div>",
            &format!(
                ".n {{ position: absolute; left: 0; top: 0; width: 40px; font: 700 20px Liberation Sans; color: #ffffff; {css} }}"
            ),
            0,
        );
        let (mut top, mut bottom, mut right) = (u32::MAX, 0, 0);
        for y in 0..f.height() {
            for x in 0..f.width() {
                if f.get(x, y).a > 0.5 {
                    top = top.min(y);
                    bottom = bottom.max(y);
                    right = right.max(x);
                }
            }
        }
        (bottom - top, right)
    };
    let (wrapped, _) = height("");
    let (kept, right) = height("white-space: nowrap");
    assert!(
        wrapped > 30,
        "without nowrap the words take two lines: {wrapped}"
    );
    assert!(
        kept < 25 && right > 100,
        "nowrap kept one line past the box: {kept} tall, to x {right}"
    );
}
