//! Video through the GPU renderer against the CPU renderer: the test
//! clip's frames converted from their 4:2:0 planes on the device, and a
//! rotated clip, whose frames go up as the pictures the decoder converts.
//! Without an adapter the tests pass having checked nothing, and say so.

#![cfg(feature = "media")]

use std::path::{Path, PathBuf};

use geneva_golden::{Rgba8Image, Tolerance, compare};
use geneva_gpu::{Gpu, GpuError, GpuRenderer, Preference};
use geneva_media::MediaAssets;
use geneva_render::{AssetSource, CpuRenderer, Frame, Renderer};
use geneva_timeline::{Ratio, load};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/media")
}

fn device() -> Option<Gpu> {
    match Gpu::probe(Preference::Any) {
        Ok(gpu) => Some(gpu),
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

fn scene(src: &str) -> String {
    format!(
        r##"{{"geneva":"0.4","output":{{"width":320,"height":180,"fps":25,"duration":"2s","background":"#202830"}},
        "assets":{{"v":{{"src":"{src}"}}}},
        "layers":[{{"clips":[{{"source":{{"kind":"video","asset":"v"}},"fit":"contain","duration":"2s",
          "transform":{{"position":{{"x":"50%","y":"50%"}},"rotation":{{"keyframes":[{{"t":0,"v":0}},{{"t":"2s","v":20}}]}}}}}}]}}]}}"##
    )
}

/// Renders `src` at a few times both ways and checks the GPU frame
/// against the CPU frame under the default tolerance.
fn agree(gpu: &Gpu, src: &str) {
    let comp = load(&scene(src)).composition.expect("a valid scene");
    let mut cpu = CpuRenderer::new(MediaAssets::new(root()));
    let mut on_gpu = GpuRenderer::new(gpu.clone(), MediaAssets::new(root()));
    for t in ["0s", "0.5s", "1.5s"] {
        let time = geneva_timeline::Time::parse(t).unwrap().resolve(comp.fps);
        let a = cpu.render_frame(&comp, time).expect("CPU frame");
        let b = on_gpu.render_frame(&comp, time).expect("GPU frame");
        let any = compare(&rgba8(&b), &rgba8(&a), 0);
        let over = compare(&rgba8(&b), &rgba8(&a), 2);
        eprintln!(
            "{src} @ {t}: {:.3}% differing, {} over 2, max {:?}, psnr {:.2} dB, ssim {:.5}",
            any.mismatch_fraction * 100.0,
            over.mismatched_pixels,
            any.max_channel_diff,
            any.psnr,
            any.ssim
        );
        assert!(
            over.passes(&Tolerance::default()),
            "{src} @ {t}: the GPU frame is outside the tolerance ({})",
            over.summary()
        );
    }
}

#[test]
fn the_clip_is_converted_on_the_device_and_matches_the_cpu() {
    let Some(gpu) = device() else {
        return;
    };
    // The clip is 8-bit 4:2:0 SDR and unrotated, so the decoder hands
    // over its planes.
    let comp = load(&scene("clip.mp4")).composition.unwrap();
    let mut assets = MediaAssets::new(root());
    assert!(
        assets
            .video_planes(&comp, "v", Ratio::ZERO)
            .expect("the clip decodes")
            .is_some(),
        "the test clip should be handed over as planes"
    );
    agree(&gpu, "clip.mp4");
}

#[test]
fn a_rotated_clip_goes_up_as_a_picture_and_matches_the_cpu() {
    let Some(gpu) = device() else {
        return;
    };
    let comp = load(&scene("sync/rotated-90.mp4")).composition.unwrap();
    let mut assets = MediaAssets::new(root());
    assert!(
        assets
            .video_planes(&comp, "v", Ratio::ZERO)
            .expect("the clip decodes")
            .is_none(),
        "a rotated clip is converted by the decoder, not the shader"
    );
    agree(&gpu, "sync/rotated-90.mp4");
}
