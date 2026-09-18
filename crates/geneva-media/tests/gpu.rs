//! Video through the GPU renderer against the CPU renderer: the test
//! clip's frames converted from their 4:2:0 planes on the device, and a
//! rotated clip, whose frames go up as the pictures the decoder converts.
//! Without an adapter the tests pass having checked nothing, and say so.

#![cfg(feature = "media")]

use std::path::{Path, PathBuf};

use geneva_color::ResolvedTags;
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

/// A scene with transparency, gradients through blur, and a shape on an
/// odd-sized frame, so every pack path (edge chroma blocks, straight
/// alpha) is exercised.
fn pack_scene(width: u32, height: u32) -> String {
    format!(
        r##"{{"geneva":"0.4","output":{{"width":{width},"height":{height},"fps":25,"duration":"1s","background":"#00000000"}},
        "layers":[
          {{"clips":[{{"source":{{"kind":"shape","shape":"rect","width":"70%","height":"60%","fill":"#ff8040c0","radius":12}},
             "effects":[{{"kind":"blur","radius":5}}],"transform":{{"position":{{"x":"45%","y":"50%"}},"rotation":10}}}}]}},
          {{"clips":[{{"source":{{"kind":"shape","shape":"ellipse","width":"40%","height":"70%","fill":"#40c0ff"}},
             "transform":{{"position":{{"x":"70%","y":"40%"}}}},"opacity":0.7}}]}}]}}"##
    )
}

/// The frame packed on the device against the CPU's pack of the CPU's
/// frame, for every layout the encoders take, SDR and HDR.
#[test]
fn the_pack_on_the_device_matches_the_cpu_for_every_layout() {
    use geneva_color::{Matrix, Primaries, Range, Transfer};
    use geneva_media::convert::{PlaneFormat, Planes};
    use geneva_media::{FramePacker, PlaneRenderer, PlaneTarget};
    use geneva_render::NoAssets;
    let Some(gpu) = device() else {
        return;
    };
    let sdr = ResolvedTags::SDR_VIDEO;
    let hdr = ResolvedTags {
        primaries: Primaries::Bt2020,
        transfer: Transfer::Pq,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
    };
    let full = ResolvedTags {
        range: Range::Full,
        ..sdr
    };
    let cases: [(&str, ResolvedTags, PlaneFormat, u32); 8] = [
        ("yuv420p", sdr, PlaneFormat::Yuv420p8, 1),
        ("yuv420p full", full, PlaneFormat::Yuv420p8, 1),
        ("yuv422p", sdr, PlaneFormat::Yuv422p8, 1),
        ("yuv420p10", sdr, PlaneFormat::Yuv420p10, 3),
        ("yuv422p10", sdr, PlaneFormat::Yuv422p10, 3),
        ("yuv444p10", sdr, PlaneFormat::Yuv444p10, 3),
        ("yuv420p10 PQ", hdr, PlaneFormat::Yuv420p10, 3),
        ("rgba", sdr, PlaneFormat::Rgba8, 1),
    ];
    for (width, height) in [(64u32, 36u32), (33, 19)] {
        let comp = load(&pack_scene(width, height)).composition.unwrap();
        let mut cpu = FramePacker::new(CpuRenderer::new(NoAssets));
        let mut on_gpu = GpuRenderer::new(gpu.clone(), NoAssets);
        let t = Ratio::new(1, 5);
        for (name, tags, format, allowed) in cases {
            let mut a = Planes::new(format, width, height);
            let mut b = Planes::new(format, width, height);
            for (renderer, planes) in [
                (&mut cpu as &mut dyn PlaneRenderer, &mut a),
                (&mut on_gpu as &mut dyn PlaneRenderer, &mut b),
            ] {
                renderer
                    .render_planes(
                        &comp,
                        t,
                        &mut [PlaneTarget {
                            tags,
                            format,
                            planes,
                        }],
                        None,
                    )
                    .expect("renders");
            }
            for (k, (pa, pb)) in a.planes.iter().zip(&b.planes).enumerate() {
                let wide = format.bytes_per_sample() == 2;
                let samples = |p: &geneva_media::convert::Plane| -> Vec<u32> {
                    if wide {
                        p.data
                            .chunks_exact(2)
                            .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
                            .collect()
                    } else {
                        p.data.iter().map(|&c| u32::from(c)).collect()
                    }
                };
                let (sa, sb) = (samples(pa), samples(pb));
                let mut worst = 0u32;
                let mut differing = 0usize;
                if format.is_rgb() {
                    // Straight alpha: a color under a near-zero alpha is
                    // the premultiplied value divided by almost nothing,
                    // so the two are compared premultiplied, and not at
                    // all where either alpha rounds to zero.
                    for (x, y) in sa.chunks_exact(4).zip(sb.chunks_exact(4)) {
                        let d = x[3].abs_diff(y[3]);
                        worst = worst.max(d);
                        differing += usize::from(d > 0);
                        if x[3] == 0 || y[3] == 0 {
                            continue;
                        }
                        for c in 0..3 {
                            let pa = (x[c] * x[3] + 127) / 255;
                            let pb = (y[c] * y[3] + 127) / 255;
                            let d = pa.abs_diff(pb);
                            worst = worst.max(d);
                            differing += usize::from(x[c] != y[c]);
                        }
                    }
                } else {
                    for (x, y) in sa.iter().zip(&sb) {
                        let d = x.abs_diff(*y);
                        worst = worst.max(d);
                        differing += usize::from(d > 0);
                    }
                }
                eprintln!(
                    "{width}x{height} {name} plane {k}: worst {worst} code(s), {differing} of {} samples differ",
                    sa.len()
                );
                assert!(
                    worst <= allowed,
                    "{width}x{height} {name} plane {k}: samples differ by {worst} codes, more than {allowed}"
                );
            }
        }
    }
}

/// Frames prepared ahead come out the same as frames asked for cold,
/// and a frame asked for out of order is drawn afresh.
#[test]
fn frames_prepared_ahead_are_the_same_frames() {
    use geneva_media::convert::{PlaneFormat, Planes};
    use geneva_media::{PlaneRenderer, PlaneTarget};
    use geneva_render::NoAssets;
    let Some(gpu) = device() else {
        return;
    };
    let comp = load(&pack_scene(64, 36)).composition.unwrap();
    let tags = ResolvedTags::SDR_VIDEO;
    let format = PlaneFormat::Yuv420p8;
    let mut renderer = GpuRenderer::new(gpu, NoAssets);
    let mut cold = Vec::new();
    for n in 0..6u64 {
        let mut planes = Planes::new(format, 64, 36);
        renderer
            .render_planes(
                &comp,
                comp.frame_time(n),
                &mut [PlaneTarget {
                    tags,
                    format,
                    planes: &mut planes,
                }],
                None,
            )
            .unwrap();
        cold.push(planes);
    }
    // Two ahead, then in order; then out of order.
    renderer.prepare_planes(&comp, comp.frame_time(0), &[(tags, format)]);
    renderer.prepare_planes(&comp, comp.frame_time(1), &[(tags, format)]);
    for n in [0u64, 1, 2, 5, 3] {
        if n + 1 < 6 {
            renderer.prepare_planes(&comp, comp.frame_time(n + 1), &[(tags, format)]);
        }
        let mut planes = Planes::new(format, 64, 36);
        renderer
            .render_planes(
                &comp,
                comp.frame_time(n),
                &mut [PlaneTarget {
                    tags,
                    format,
                    planes: &mut planes,
                }],
                None,
            )
            .unwrap();
        assert_eq!(
            planes, cold[n as usize],
            "frame {n} differs when prepared ahead"
        );
    }
}
