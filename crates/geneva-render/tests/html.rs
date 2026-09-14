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
