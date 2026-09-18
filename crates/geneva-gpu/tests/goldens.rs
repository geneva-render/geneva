//! The golden cases on the GPU. The cases the compositor draws must pass
//! under their tolerance against the references the CPU renderer wrote;
//! every case is rendered by both renderers and compared pixel by pixel,
//! with the numbers printed for the record (`--nocapture`); and the same
//! frame drawn twice on the same device is the same to the bit.
//!
//! Without an adapter (no Vulkan loader, no lavapipe) the tests pass
//! having checked nothing, and say so.

use std::path::{Path, PathBuf};

use geneva_golden::{Rgba8Image, compare, discover_cases, load_case, run_case_on};
use geneva_gpu::{Gpu, GpuError, GpuRenderer, Preference};
use geneva_render::{CpuRenderer, FileAssets, Frame, RenderError, Renderer};

/// The cases that must pass on the GPU today.
const DRAWN: &[&str] = &[
    "solid-and-shapes",
    "image-transform",
    "hdr-to-sdr",
    "blend-modes",
    "nested-composition",
    "masks-and-fades",
    "text-basics",
    "markup-card",
    "markup-opening",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

fn failures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/golden-failures-gpu")
}

fn device() -> Option<Gpu> {
    match Gpu::probe(Preference::Any) {
        Ok(gpu) => {
            eprintln!("device: {}", gpu.report().line());
            Some(gpu)
        }
        Err(GpuError::NoAdapter { .. }) => {
            eprintln!("no GPU adapter here; nothing was checked");
            None
        }
        Err(e) => panic!("an adapter was found but could not be used: {e}"),
    }
}

fn rgba8(frame: &Frame) -> Rgba8Image {
    Rgba8Image::new(frame.width(), frame.height(), frame.to_rgba8())
}

#[test]
fn drawn_cases_pass_against_the_references() {
    let Some(gpu) = device() else {
        return;
    };
    let mut failed = Vec::new();
    for name in DRAWN {
        let mut make = |root: &Path| -> Box<dyn Renderer> {
            Box::new(GpuRenderer::new(gpu.clone(), FileAssets::new(root)))
        };
        let outcome = run_case_on(&root().join(name), &failures(), &mut make, None)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        for f in &outcome.frames {
            let summary = f
                .comparison
                .as_ref()
                .map_or("written".to_owned(), geneva_golden::Comparison::summary);
            eprintln!(
                "{name} @ {}: {} ({summary})",
                f.time,
                if f.passed { "ok" } else { "FAIL" }
            );
            assert!(
                f.comparison.is_some(),
                "{name} @ {}: no reference; the CPU renderer writes them",
                f.time
            );
        }
        if !outcome.passed() {
            failed.push(*name);
        }
    }
    assert!(
        failed.is_empty(),
        "GPU golden failures: {failed:?}\nartifacts under {}",
        failures().display()
    );
}

#[test]
fn every_case_is_compared_with_the_cpu_renderer_pixel_by_pixel() {
    let Some(gpu) = device() else {
        return;
    };
    eprintln!(
        "{:<20} {:>8} {:>10} {:>9} {:>14} {:>9} {:>8}",
        "case", "frame", "differing", "over 2", "max diff", "psnr dB", "ssim"
    );
    for dir in discover_cases(&root()) {
        let case = load_case(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        let mut cpu = CpuRenderer::with_asset_root(&case.root);
        let mut on_gpu = GpuRenderer::new(gpu.clone(), FileAssets::new(&case.root));
        for (text, time) in &case.frames {
            let reference = cpu
                .render_frame(&case.composition, *time)
                .unwrap_or_else(|e| panic!("{} @ {text} on the CPU: {e}", case.name));
            let drawn = match on_gpu.render_frame(&case.composition, *time) {
                Ok(frame) => frame,
                Err(RenderError::Unsupported { what, .. }) => {
                    eprintln!("{:<20} {:>8} not drawn yet ({what})", case.name, text);
                    break;
                }
                Err(e) => panic!("{} @ {text} on the GPU: {e}", case.name),
            };
            let (a, b) = (rgba8(&drawn), rgba8(&reference));
            let any = compare(&a, &b, 0);
            let over = compare(&a, &b, 2);
            assert!(any.same_size, "{} @ {text}: sizes differ", case.name);
            eprintln!(
                "{:<20} {:>8} {:>10} {:>9} {:>14} {:>9.2} {:>8.5}",
                case.name,
                text,
                format!("{:.3}%", any.mismatch_fraction * 100.0),
                over.mismatched_pixels,
                format!("{:?}", any.max_channel_diff),
                any.psnr,
                any.ssim
            );
            assert!(
                over.passes(&case.tolerance),
                "{} @ {text}: the GPU frame is outside the case's tolerance of the CPU frame ({})",
                case.name,
                over.summary()
            );
        }
    }
}

#[test]
fn the_same_frame_is_the_same_to_the_bit() {
    let Some(gpu) = device() else {
        return;
    };
    let case = load_case(&root().join("solid-and-shapes")).expect("the case loads");
    let mut renderer = GpuRenderer::new(gpu, FileAssets::new(&case.root));
    let time = case.frames[1].1;
    let first = renderer
        .render_frame(&case.composition, time)
        .expect("renders");
    let second = renderer
        .render_frame(&case.composition, time)
        .expect("renders again");
    assert_eq!(first.pixels().len(), second.pixels().len());
    let same = first.pixels().iter().zip(second.pixels()).all(|(a, b)| {
        a.r.to_bits() == b.r.to_bits()
            && a.g.to_bits() == b.g.to_bits()
            && a.b.to_bits() == b.b.to_bits()
            && a.a.to_bits() == b.a.to_bits()
    });
    assert!(same, "two renders of one frame differ");
}
