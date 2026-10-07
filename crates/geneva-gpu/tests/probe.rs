//! The device probe, on whatever this machine has. With no adapter at
//! all (a box without a Vulkan loader, or without Mesa's lavapipe) the
//! probe reports that and the test passes without a device; nothing is
//! learned, and the message says so.

use geneva_gpu::{Gpu, GpuError, GpuRenderer, Preference};
use geneva_render::{Frame, NoAssets, RenderError, Renderer};
use geneva_timeline::{Ratio, load};

#[test]
fn probe_describes_the_device_or_says_there_is_none() {
    let gpu = match Gpu::probe(Preference::Any) {
        Ok(gpu) => gpu,
        Err(GpuError::NoAdapter { .. }) => {
            eprintln!("no GPU adapter here; the probe is not exercised");
            return;
        }
        Err(e) => panic!("an adapter was found but could not be used: {e}"),
    };
    let report = gpu.report();
    assert_ne!(report.name, "");
    assert_ne!(report.backend, "");
    assert!(
        ["discrete", "integrated", "virtual", "software", "unknown"]
            .contains(&report.kind.as_str())
    );
    assert_eq!(report.software, report.kind == "software");
    assert!(report.line().contains(&report.name));
    eprintln!(
        "device: {} [{}] f16={}",
        report.line(),
        report.driver,
        report.shader_f16
    );
}

#[test]
fn a_software_adapter_is_only_picked_when_asked() {
    let Ok(gpu) = Gpu::probe(Preference::Software) else {
        eprintln!("no software adapter here");
        return;
    };
    assert!(gpu.report().software);
    // Asking for hardware must not return it.
    if let Ok(hardware) = Gpu::probe(Preference::Hardware) {
        assert!(!hardware.report().software);
    }
}

#[test]
fn an_empty_scene_is_its_background_and_times_are_checked() {
    let Ok(gpu) = Gpu::probe(Preference::Any) else {
        return;
    };
    let loaded = load(
        r##"{"geneva":"1.0","output":{"width":16,"height":16,"fps":30,"duration":"1s","background":"#336699"},"layers":[]}"##,
    );
    let comp = loaded.composition.expect("a valid scene");
    let mut renderer = GpuRenderer::new(gpu, NoAssets);
    let mut frame = Frame::new(0, 0, geneva_color::Color::BLACK);
    renderer
        .render_into(&comp, Ratio::ZERO, &mut frame)
        .expect("an empty scene renders");
    assert_eq!((frame.width(), frame.height()), (16, 16));
    let want = comp.background.to_linear();
    for p in frame.pixels() {
        for (got, want) in [(p.r, want.r), (p.g, want.g), (p.b, want.b), (p.a, want.a)] {
            assert!(
                (got - want).abs() < 1e-3f32,
                "pixel {p:?} is not the background"
            );
        }
    }
    assert!(matches!(
        renderer.render_into(&comp, Ratio::from_int(5), &mut frame),
        Err(RenderError::OutOfRange { .. })
    ));
}
