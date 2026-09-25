//! Text rendering behaviour with a bundled font.

use std::path::PathBuf;

use geneva_render::{CpuRenderer, Frame, Renderer};
use geneva_timeline::{Ratio, load};

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

fn render(body: &str, t: Ratio) -> Frame {
    let text = format!(
        r##"{{"geneva":"1.0","output":{{"width":320,"height":120,"fps":30,"duration":"2s","background":"transparent"}},
            "assets":{{"sans":{{"src":"fonts/LiberationSans-Regular.ttf"}},"bold":{{"src":"fonts/LiberationSans-Bold.ttf"}}}},
            "layers":[{{"clips":[{{"source":{body},"transform":{{"position":{{"x":"50%","y":"50%"}}}}}}]}}]}}"##
    );
    let loaded = load(&text);
    assert!(loaded.is_ok(), "{:#?}", loaded.diagnostics);
    let comp = loaded.composition.unwrap();
    CpuRenderer::with_asset_root(golden_root())
        .render_frame(&comp, t)
        .unwrap()
}

/// Bounding box of pixels with alpha above zero.
fn bounds(frame: &Frame) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            if frame.get(x, y).a > 0.001 {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

fn coverage(frame: &Frame) -> f32 {
    frame.pixels().iter().map(|p| p.a).sum()
}

#[test]
fn text_renders_with_a_bundled_font_and_is_centered() {
    let f = render(
        r##"{"kind":"text","text":"Hello","font":"sans","size":40,"color":"white"}"##,
        Ratio::ZERO,
    );
    let (x0, y0, x1, y1) = bounds(&f).expect("some glyph pixels");
    assert!(
        x1 - x0 > 60 && x1 - x0 > y1 - y0,
        "bounds {:?}",
        (x0, y0, x1, y1)
    );
    let cx = f64::from(x0 + x1) / 2.0;
    let cy = f64::from(y0 + y1) / 2.0;
    assert!(
        (cx - 160.0).abs() < 6.0 && (cy - 60.0).abs() < 12.0,
        "center {cx},{cy}"
    );
    // Glyph pixels are white.
    let white = f.pixels().iter().filter(|p| p.a > 0.99).count();
    assert!(white > 100);
    assert!(
        f.pixels()
            .iter()
            .filter(|p| p.a > 0.99)
            .all(|p| p.r > 0.98 && p.g > 0.98 && p.b > 0.98)
    );
}

#[test]
fn bold_and_larger_text_cover_more() {
    let regular = render(
        r##"{"kind":"text","text":"Weight","font":"sans","size":40}"##,
        Ratio::ZERO,
    );
    let bold = render(
        r##"{"kind":"text","text":"Weight","font":"bold","size":40}"##,
        Ratio::ZERO,
    );
    let big = render(
        r##"{"kind":"text","text":"Weight","font":"sans","size":60}"##,
        Ratio::ZERO,
    );
    assert!(coverage(&bold) > coverage(&regular) * 1.1);
    assert!(coverage(&big) > coverage(&regular) * 1.5);
}

#[test]
fn outline_background_and_shadow_add_coverage_around_glyphs() {
    let plain = render(
        r##"{"kind":"text","text":"Edge","font":"sans","size":40}"##,
        Ratio::ZERO,
    );
    let outlined = render(
        r##"{"kind":"text","text":"Edge","font":"sans","size":40,"outline":{"color":"red","width":2}}"##,
        Ratio::ZERO,
    );
    let boxed = render(
        r##"{"kind":"text","text":"Edge","font":"sans","size":40,"background":"#336699","padding":10}"##,
        Ratio::ZERO,
    );
    let shadowed = render(
        r##"{"kind":"text","text":"Edge","font":"sans","size":40,"shadow":{"x":4,"y":4,"blur":3}}"##,
        Ratio::ZERO,
    );
    assert!(coverage(&outlined) > coverage(&plain) * 1.3);
    assert!(coverage(&boxed) > coverage(&plain) * 3.0);
    assert!(coverage(&shadowed) > coverage(&plain) * 1.2);
    // The outline is red and sits outside the white fill.
    let red = outlined
        .pixels()
        .iter()
        .filter(|p| p.a > 0.9 && p.r > 0.9 && p.g < 0.1)
        .count();
    assert!(red > 50, "red outline pixels {red}");
    let pb = bounds(&plain).unwrap();
    let ob = bounds(&outlined).unwrap();
    assert!(ob.0 < pb.0 && ob.2 > pb.2);
}

#[test]
fn highlighted_word_changes_with_time() {
    let body = r##"{"kind":"text","words":[{"text":"first","start":0,"end":"1s"},{"text":"second","start":"1s","end":"2s"}],
        "font":"sans","size":36,"color":"white","highlight":{"color":"#ff8800"}}"##;
    let early = render(body, Ratio::new(1, 2));
    let late = render(body, Ratio::new(3, 2));
    let orange = |f: &Frame| {
        f.pixels()
            .iter()
            .filter(|p| p.a > 0.9 && p.r > 0.9 && p.b < 0.05)
            .count()
    };
    assert!(orange(&early) > 50 && orange(&late) > 50);
    let ob_early = bounds_of(&early, |p| p.a > 0.9 && p.r > 0.9 && p.b < 0.05).unwrap();
    let ob_late = bounds_of(&late, |p| p.a > 0.9 && p.r > 0.9 && p.b < 0.05).unwrap();
    assert!(
        ob_early.2 < ob_late.0,
        "highlight moves from the first word to the second"
    );
}

fn bounds_of(
    frame: &Frame,
    pred: impl Fn(&geneva_color::LinearRgba) -> bool,
) -> Option<(u32, u32, u32, u32)> {
    let mut b: Option<(u32, u32, u32, u32)> = None;
    for y in 0..frame.height() {
        for x in 0..frame.width() {
            if pred(&frame.get(x, y)) {
                b = Some(match b {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    b
}

#[test]
fn wrapping_and_alignment_follow_max_width() {
    let one_line = render(
        r##"{"kind":"text","text":"wrap wrap wrap wrap","font":"sans","size":20}"##,
        Ratio::ZERO,
    );
    let wrapped = render(
        r##"{"kind":"text","text":"wrap wrap wrap wrap","font":"sans","size":20,"max_width":90}"##,
        Ratio::ZERO,
    );
    let (_, y0, _, y1) = bounds(&one_line).unwrap();
    let (_, wy0, _, wy1) = bounds(&wrapped).unwrap();
    assert!(
        wy1 - wy0 > (y1 - y0) * 2,
        "wrapped text should be much taller"
    );
}

#[test]
fn complex_scripts_are_shaped_when_a_font_is_available() {
    // Uses system fonts; skipped where none covers Arabic.
    let f = render(
        r##"{"kind":"text","text":"مرحبا بالعالم","size":40}"##,
        Ratio::ZERO,
    );
    if coverage(&f) < 10.0 {
        eprintln!("no Arabic-capable font on this system; skipping");
        return;
    }
    let (x0, _, x1, _) = bounds(&f).unwrap();
    assert!(x1 - x0 > 100);
}

#[test]
fn letter_spacing_is_pixels_not_ems() {
    // Six characters, so five gaps. Asking for 6px of tracking should
    // widen the run by about 30px, whatever the font size. cosmic-text
    // wants a share of the em here, so passing pixels straight through
    // scaled the tracking by the font size: at 30px that is 180px per
    // gap rather than 6, which does not even fit the frame.
    let plain = r#"{"kind":"text","text":"HHHHHH","font":"sans","size":30}"#;
    let spaced = r#"{"kind":"text","text":"HHHHHH","font":"sans","size":30,"letter_spacing":6}"#;
    let a = bounds(&render(plain, Ratio::ZERO)).expect("plain text draws something");
    let b = bounds(&render(spaced, Ratio::ZERO)).expect("spaced text draws something");
    let grew = f64::from(b.2 - b.0) - f64::from(a.2 - a.0);
    assert!(
        (grew - 30.0).abs() <= 5.0,
        "five gaps of 6px should add about 30px, got {grew}"
    );
}

/// A family a document ships is the one it draws in, whether the source
/// names the asset or the family the file declares.
///
/// Only the bold file is an asset here, so a shipped family has one face
/// and every weight snaps to it. Asking for weight 400 by family has to
/// give the same picture as asking for the asset id: the machine's own
/// Liberation Sans, where there is one, must not get in. It used to,
/// because the font assets were registered only when a source named one
/// by id, so naming the family left the document's faces unloaded and
/// the machine's in place. Nothing said so: the family was available,
/// just not the copy the document carried.
#[test]
fn a_text_source_draws_in_a_shipped_family_named_by_family() {
    let body = |font: &str| {
        format!(r##"{{"kind":"text","text":"HANDGLOVES","font":"{font}","color":"#fff"}}"##)
    };
    let text = |body: String| {
        format!(
            r##"{{"geneva":"1.0","output":{{"width":640,"height":120,"fps":30,"duration":"2s","background":"transparent"}},
                "assets":{{"only_bold":{{"src":"fonts/LiberationSans-Bold.ttf","kind":"font"}}}},
                "layers":[{{"clips":[{{"source":{body},"transform":{{"position":{{"x":"50%","y":"50%"}}}}}}]}}]}}"##
        )
    };
    let width_of = |font: &str| {
        let loaded = load(&text(body(font)));
        assert!(loaded.is_ok(), "{:#?}", loaded.diagnostics);
        let frame = CpuRenderer::with_asset_root(golden_root())
            .render_frame(&loaded.composition.unwrap(), Ratio::ZERO)
            .unwrap();
        let (x0, _, x1, _) = bounds(&frame).expect("some text");
        x1 - x0
    };

    let by_id = width_of("400 32px only_bold");
    let by_family = width_of("400 32px Liberation Sans");
    assert_eq!(
        by_id, by_family,
        "the shipped bold face draws either way; a different width means \
         the machine's regular was used for the family"
    );
}
