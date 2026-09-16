//! A text clip whose colour and shadow move over the clip.

use geneva_color::LinearRgba;
use geneva_render::{CpuRenderer, NoAssets, Renderer};
use geneva_timeline::{Ratio, load};

fn frame_at(clip: &str, tenths: i64) -> geneva_render::Frame {
    let text = format!(
        r##"{{"geneva":"0.3","output":{{"width":240,"height":120,"fps":30,"duration":"2s",
        "background":"transparent"}},"layers":[{{"clips":[{{{clip}}}]}}]}}"##
    );
    let l = load(&text);
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let comp = l.composition.unwrap();
    CpuRenderer::new(NoAssets)
        .render_frame(&comp, Ratio::new(tenths, 10))
        .unwrap()
}

/// The box of fully opaque pixels: the glyph cores, never a blurred edge.
fn ink(f: &geneva_render::Frame) -> (u32, u32, u32, u32) {
    let mut b = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..120 {
        for x in 0..240 {
            if f.get(x, y).a > 0.95 {
                b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
            }
        }
    }
    assert_ne!(b.0, u32::MAX, "nothing was drawn");
    b
}

/// The mean of the fully opaque pixels, which is the fill colour.
fn fill(f: &geneva_render::Frame) -> LinearRgba {
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for y in 0..120 {
        for x in 0..240 {
            let p = f.get(x, y);
            if p.a > 0.95 {
                sum[0] += p.r;
                sum[1] += p.g;
                sum[2] += p.b;
                n += 1.0;
            }
        }
    }
    LinearRgba {
        r: sum[0] / n,
        g: sum[1] / n,
        b: sum[2] / n,
        a: 1.0,
    }
}

const TEXT: &str = r##""source":{"kind":"text","text":"Hi","font":"700 40px Liberation Sans",
    "color":{"keyframes":[[0,"#ff0000"],["1.5s","#0000ff"]]}},"duration":"2s""##;

#[test]
fn colour_follows_its_keyframes() {
    let start = fill(&frame_at(TEXT, 0));
    let end = fill(&frame_at(TEXT, 19));
    assert!(
        start.r > 0.9 && start.b < 0.1,
        "red at the start: {start:?}"
    );
    assert!(end.b > 0.9 && end.r < 0.1, "blue at the end: {end:?}");
}

#[test]
fn a_swelling_shadow_leaves_the_glyphs_where_they_were() {
    // The image is padded for the shadow. If it were padded by the
    // shadow at the frame rather than its largest, the text would creep
    // outward as the blur grew.
    let clip = r##""source":{"kind":"text","text":"Hi","font":"700 40px Liberation Sans","color":"#ffffff",
        "shadow":{"color":"#00ff00","blur":{"keyframes":[[0,0],["1.5s",24]]}}},"duration":"2s""##;
    let sharp = ink(&frame_at(clip, 0));
    let glowing = ink(&frame_at(clip, 19));
    assert_eq!(sharp, glowing, "the glyphs moved as the shadow grew");
}

#[test]
fn a_constant_style_still_reads_as_it_did() {
    let clip = r##""source":{"kind":"text","text":"Hi","font":"700 40px Liberation Sans","color":"#00ff00",
        "shadow":"0 2px 8px #0008"},"duration":"2s""##;
    let c = fill(&frame_at(clip, 10));
    assert!(c.g > 0.9 && c.r < 0.1 && c.b < 0.1, "{c:?}");
}
