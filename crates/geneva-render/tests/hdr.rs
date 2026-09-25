//! HDR still images into the SDR working space: the gray steps and the
//! BT.2020 primaries of the `hdr-to-sdr` golden patterns, checked by
//! value.

use std::path::PathBuf;

use geneva_color::LinearRgba;
use geneva_render::{CpuRenderer, Frame, Renderer};
use geneva_timeline::{Ratio, load};

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// Renders one of the patterns at its own size.
fn render(name: &str, transfer: &str) -> Frame {
    let text = format!(
        r##"{{"geneva":"1.0","output":{{"width":256,"height":64,"fps":24,"duration":"1s","background":"black"}},
            "assets":{{"hdr":{{"src":"hdr-to-sdr/{name}.png","color":{{"primaries":"bt2020","transfer":"{transfer}"}}}}}},
            "layers":[{{"clips":[{{"source":{{"kind":"image","asset":"hdr"}}}}]}}]}}"##
    );
    let loaded = load(&text);
    assert!(loaded.is_ok(), "{:#?}", loaded.diagnostics);
    let comp = loaded.composition.unwrap();
    CpuRenderer::with_asset_root(golden_root())
        .render_frame(&comp, Ratio::ZERO)
        .unwrap()
}

/// The middle pixel of patch `i` (32 pixels wide) in the row at `y`.
fn patch(frame: &Frame, i: usize, y: usize) -> LinearRgba {
    frame.pixels()[y * 256 + 16 + 32 * i]
}

fn gray_steps(frame: &Frame) -> Vec<f32> {
    (0..8).map(|i| patch(frame, i, 16).g).collect()
}

fn close(got: f32, want: f32, eps: f32, what: &str) {
    assert!(
        (got - want).abs() <= eps,
        "{what}: {got} (wanted {want} ± {eps})"
    );
}

/// The gray patches are 0, 10, 50, 100, 203, 500, 1000 and 4000 nits.
fn check_grays(steps: &[f32], what: &str) {
    close(steps[0], 0.0, 0.002, &format!("{what} black"));
    // BT.2446 method A, worked out from its equations.
    close(steps[1], 0.0313, 0.006, &format!("{what} 10 nits"));
    close(steps[2], 0.1265, 0.01, &format!("{what} 50 nits"));
    close(steps[3], 0.2266, 0.015, &format!("{what} 100 nits"));
    // Reference white lands at 0.41 of SDR white.
    close(steps[4], 0.406, 0.03, &format!("{what} 203 nits"));
    close(steps[5], 0.7315, 0.03, &format!("{what} 500 nits"));
    // The source peak lands on SDR white; beyond it stays there.
    close(steps[6], 1.0, 0.01, &format!("{what} 1000 nits"));
    close(steps[7], 1.0, 0.01, &format!("{what} 4000 nits"));
    for w in steps.windows(2) {
        assert!(w[1] >= w[0], "{what}: not monotonic: {steps:?}");
    }
}

#[test]
fn pq_gray_steps_roll_off_to_sdr_white() {
    check_grays(&gray_steps(&render("pq", "pq")), "pq");
}

#[test]
fn hlg_gray_steps_match_pq_within_a_tolerance() {
    let hlg = gray_steps(&render("hlg", "hlg"));
    check_grays(&hlg, "hlg");
    let pq = gray_steps(&render("pq", "pq"));
    for (i, (h, p)) in hlg.iter().zip(&pq).enumerate() {
        close(*h, *p, 0.03, &format!("hlg vs pq step {i}"));
    }
}

#[test]
fn bt2020_primaries_land_inside_the_cube_with_their_hue() {
    for (name, transfer) in [("pq", "pq"), ("hlg", "hlg")] {
        let frame = render(name, transfer);
        // Red, green, blue, cyan, magenta, yellow at 100 nits.
        let colors: Vec<[f32; 3]> = (0..6)
            .map(|i| {
                let p = patch(&frame, i, 48);
                [p.r, p.g, p.b]
            })
            .collect();
        for (i, c) in colors.iter().enumerate() {
            assert!(
                c.iter().all(|v| (0.0..=1.0).contains(v)),
                "{name} color {i} leaves the cube: {c:?}"
            );
        }
        let dominant = |c: &[f32; 3], hi: &[usize], lo: &[usize]| {
            hi.iter().all(|&h| lo.iter().all(|&l| c[h] > c[l] + 0.05))
        };
        assert!(
            dominant(&colors[0], &[0], &[1, 2]),
            "{name} red {:?}",
            colors[0]
        );
        assert!(
            dominant(&colors[1], &[1], &[0, 2]),
            "{name} green {:?}",
            colors[1]
        );
        assert!(
            dominant(&colors[2], &[2], &[0, 1]),
            "{name} blue {:?}",
            colors[2]
        );
        assert!(
            dominant(&colors[3], &[1, 2], &[0]),
            "{name} cyan {:?}",
            colors[3]
        );
        assert!(
            dominant(&colors[4], &[0, 2], &[1]),
            "{name} magenta {:?}",
            colors[4]
        );
        assert!(
            dominant(&colors[5], &[0, 1], &[2]),
            "{name} yellow {:?}",
            colors[5]
        );
        // Reference white and black patches on the same row.
        let white = patch(&frame, 6, 48);
        close(white.g, 0.406, 0.03, &format!("{name} white patch"));
        close(
            white.r,
            white.g,
            0.01,
            &format!("{name} white patch is gray"),
        );
        close(
            patch(&frame, 7, 48).g,
            0.0,
            0.002,
            &format!("{name} black patch"),
        );
    }
}

#[test]
fn an_untagged_8bit_image_is_still_plain_srgb() {
    // The checker of the image-transform case: pure black and white.
    let text = r##"{"geneva":"1.0","output":{"width":64,"height":64,"fps":24,"duration":"1s","background":"black"},
        "assets":{"c":{"src":"image-transform/checker.png"}},
        "layers":[{"clips":[{"source":{"kind":"image","asset":"c"},"transform":{"anchor":{"x":0,"y":0},"position":{"x":0,"y":0}}}]}]}"##;
    let comp = load(text).composition.unwrap();
    let frame = CpuRenderer::with_asset_root(golden_root())
        .render_frame(&comp, Ratio::ZERO)
        .unwrap();
    let max = frame.pixels().iter().map(|p| p.g).fold(0.0, f32::max);
    close(max, 1.0, 0.001, "checker white");
}
