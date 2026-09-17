//! The HTML source, end to end: markup and CSS in, pixels out.

use geneva_color::LinearRgba;
use geneva_render::{CpuRenderer, NoAssets, Renderer};
use geneva_timeline::{Ratio, load};

fn frame(body: &str) -> geneva_render::Frame {
    let text = format!(
        r#"{{"geneva":"0.3","output":{{"width":200,"height":100,"fps":30,"duration":"1s",
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
    let text = r#"{"geneva":"0.3","output":{"width":200,"height":100,"fps":30,"duration":"1s"},
        "layers":[{"clips":[{"source":{"kind":"html","html":"<div><p>oops</div>"}}]}]}"#;
    let l = load(text);
    assert!(!l.is_ok());
    let d = l.diagnostics.iter().find(|d| d.code == "E451").unwrap();
    assert!(d.message.contains("closes"), "{}", d.message);
}

#[test]
fn an_unsupported_property_is_a_warning_and_the_rest_still_draws() {
    let text = r#"{"geneva":"0.3","output":{"width":200,"height":100,"fps":30,"duration":"1s"},
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
        r#"{{"geneva":"0.3","output":{{"width":200,"height":100,"fps":30,"duration":"2s",
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
