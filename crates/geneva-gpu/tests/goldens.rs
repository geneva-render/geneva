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
    "markup-stroke",
    "markup-fallback",
    "markup-lines",
    "markup-origin",
    "blur",
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

/// The markup clip of a case, with its time.
fn markup_clip(case: &geneva_golden::Case) -> &geneva_timeline::ResolvedClip {
    case.composition
        .layers
        .iter()
        .flat_map(|l| l.clips.iter())
        .find(|c| matches!(c.source, geneva_timeline::ResolvedSource::Html(_)))
        .expect("a markup clip")
}

/// The largest difference between two pictures' channels, and where:
/// the pixel, its two values, and the box of every pixel differing
/// by more than a thousandth.
fn widest(a: &geneva_render::Image, b: &geneva_render::Image) -> (f32, String) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let mut worst = (0.0f32, 0);
    let mut hull: Option<[usize; 4]> = None;
    for (i, (p, q)) in a.pixels.iter().zip(&b.pixels).enumerate() {
        let (x, y) = (i % a.width as usize, i / a.width as usize);
        let mut over = false;
        for d in [p.r - q.r, p.g - q.g, p.b - q.b, p.a - q.a] {
            if d.abs() > worst.0 {
                worst = (d.abs(), i);
            }
            over |= d.abs() > 1e-3;
        }
        if over {
            hull = Some(hull.map_or([x, y, x, y], |h| {
                [h[0].min(x), h[1].min(y), h[2].max(x), h[3].max(y)]
            }));
        }
    }
    let i = worst.1;
    let at = format!(
        "({}, {}): painted {:?}, flattened {:?}; differing pixels span {:?}",
        i % a.width as usize,
        i / a.width as usize,
        a.pixels[i],
        b.pixels[i],
        hull
    );
    (worst.0, at)
}

/// The layers of a markup box, composited on the CPU through the
/// painter's own composite, are the box the painter paints: the walk
/// that hands the GPU its runs and groups is held to the walk that
/// paints. Every tenth of a second of both markup cases, no device
/// needed.
#[test]
fn markup_layers_flatten_to_the_painted_box() {
    use geneva_render::{Paint, Painter};
    for name in ["markup-opening", "markup-card"] {
        let case = load_case(&root().join(name)).expect("the case loads");
        let clip = markup_clip(&case);
        let mut painter = Painter::new(FileAssets::new(&case.root));
        let mut times: Vec<geneva_timeline::Ratio> = case.frames.iter().map(|(_, t)| *t).collect();
        let tenths = ((clip.end - clip.start).to_f64() * 10.0).ceil() as i64;
        times.extend((0..tenths).map(|k| clip.start + geneva_timeline::Ratio::new(k, 10)));
        for time in times {
            let t = time.to_f64();
            let local = (time - clip.start).to_f64();
            let painted = painter
                .paint(&case.composition, clip, time, local)
                .expect("paints");
            let Paint::Image(image) = painted.paint else {
                panic!("markup paints a picture");
            };
            let image = image.into_owned();
            let layers = painter
                .markup_layers(&case.composition, clip, local)
                .expect("lays out");
            // Markup with nothing moving inside is drawn once and kept
            // whole; it has no layers to hand over.
            let Some(layers) = layers else {
                let geneva_timeline::ResolvedSource::Html(html) = &clip.source else {
                    unreachable!("a markup clip");
                };
                assert!(html.motion.is_empty(), "{name}: animated markup has layers");
                break;
            };
            let flat = layers.flatten();
            let (diff, at) = widest(&image, &flat);
            assert!(
                diff <= 2e-3,
                "{name} @ {t:.2}s: the flattened layers differ from the painted box by {diff} at {at}"
            );
        }
    }
}

/// Every frame of the opening on the GPU against the CPU, under the
/// case's tolerance: the gate of the markup composite on the device.
/// Slow on a software device, so run on request:
/// `cargo test -p geneva-gpu --test goldens -- --ignored --nocapture`.
#[test]
#[ignore = "every frame of the opening on both renderers: minutes on a software device; run on request"]
fn every_frame_of_the_opening_is_within_tolerance() {
    let Some(gpu) = device() else {
        return;
    };
    let case = load_case(&root().join("markup-opening")).expect("the case loads");
    let mut cpu = CpuRenderer::with_asset_root(&case.root);
    let mut on_gpu = GpuRenderer::new(gpu, FileAssets::new(&case.root));
    let fps = case.composition.fps;
    let frames = (case.composition.duration * fps).to_f64().round();
    let mut worst: Option<(usize, geneva_golden::Comparison)> = None;
    let mut differing = 0.0;
    let started = std::time::Instant::now();
    let mut cpu_time = std::time::Duration::ZERO;
    let mut gpu_time = std::time::Duration::ZERO;
    for n in 0..frames as usize {
        let time = geneva_timeline::Ratio::from_int(n as i64) / fps;
        let at = std::time::Instant::now();
        let reference = cpu
            .render_frame(&case.composition, time)
            .expect("the CPU renders");
        cpu_time += at.elapsed();
        let at = std::time::Instant::now();
        let drawn = on_gpu
            .render_frame(&case.composition, time)
            .expect("the GPU renders");
        gpu_time += at.elapsed();
        let (a, b) = (rgba8(&drawn), rgba8(&reference));
        let any = compare(&a, &b, 0);
        let over = compare(&a, &b, 2);
        differing += any.mismatch_fraction;
        assert!(
            over.passes(&case.tolerance),
            "frame {n}: outside the tolerance ({})",
            over.summary()
        );
        if worst
            .as_ref()
            .is_none_or(|(_, w)| any.mismatch_fraction > w.mismatch_fraction)
        {
            worst = Some((n, any));
        }
    }
    let (n, w) = worst.expect("frames");
    eprintln!(
        "{frames} frames in {:.1} s (cpu {:.1} s, gpu {:.1} s): mean differing {:.3}%, worst frame {n} with {:.3}% differing, max diff {:?}, psnr {:.2} dB",
        started.elapsed().as_secs_f64(),
        cpu_time.as_secs_f64(),
        gpu_time.as_secs_f64(),
        differing / frames * 100.0,
        w.mismatch_fraction * 100.0,
        w.max_channel_diff,
        w.psnr
    );
}
