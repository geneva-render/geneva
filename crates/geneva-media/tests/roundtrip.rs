//! Probe, decode, encode and mix against the committed test clip.

#![cfg(feature = "media")]

use std::path::{Path, PathBuf};

use geneva_color::{Color, ColorTags, Matrix, Primaries, Range, ResolvedTags, Transfer};
use geneva_media::{
    AudioReader, AudioSettings, EncodeSettings, Encoder, VideoReader, VideoSettings, mix, probe,
};
use geneva_render::Frame;
use geneva_timeline::schema::{AudioCodec, HardwarePolicy, VideoCodec, VideoTune};
use geneva_timeline::{Ratio, load};

fn clip() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/media/clip.mp4")
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

#[test]
fn probe_reads_streams_and_color_tags() {
    let info = probe(&clip()).unwrap();
    assert!(info.container.contains("mp4"));
    let v = info.video.as_ref().unwrap();
    assert_eq!((v.width, v.height), (192, 108));
    assert_eq!(v.fps, Ratio::from_int(25));
    assert_eq!(v.codec, "h264");
    assert_eq!(v.frames, Some(50));
    assert_eq!(v.color.primaries, Some(Primaries::Bt709));
    assert_eq!(v.color.transfer, Some(Transfer::Bt709));
    assert_eq!(v.color.matrix, Some(Matrix::Bt709));
    assert!(!v.has_alpha);
    let a = info.audio.as_ref().unwrap();
    assert_eq!(a.sample_rate, 48000);
    assert_eq!(a.codec, "aac");
    let d = info.duration.unwrap().to_f64();
    assert!((d - 2.0).abs() < 0.1, "duration {d}");
}

#[test]
fn video_frames_are_decoded_in_any_order() {
    let mut reader = VideoReader::open(&clip(), ColorTags::default()).unwrap();
    assert_eq!((reader.width(), reader.height()), (192, 108));
    assert_eq!(reader.tags(), ResolvedTags::SDR_VIDEO);
    let first = reader.frame_at(Ratio::ZERO).unwrap().clone();
    assert_eq!(first.pixels.len(), 192 * 108);
    let later = reader.frame_at(Ratio::new(3, 2)).unwrap().clone();
    assert_ne!(
        first.pixels, later.pixels,
        "the test pattern moves over time"
    );
    // Going backwards seeks and decodes again; the result must match.
    let again = reader.frame_at(Ratio::ZERO).unwrap().clone();
    assert_eq!(first.pixels, again.pixels);
    // Consecutive frames are distinct, and the same time yields the same frame.
    let f10 = reader.frame_at(Ratio::new(10, 25)).unwrap().clone();
    let f10b = reader.frame_at(Ratio::new(10, 25)).unwrap().clone();
    let f11 = reader.frame_at(Ratio::new(11, 25)).unwrap().clone();
    assert_eq!(f10.pixels, f10b.pixels);
    assert_ne!(f10.pixels, f11.pixels);
    // Past the end, the last frame is held rather than failing.
    let last = reader.frame_at(Ratio::from_int(30)).unwrap().clone();
    assert_eq!(last.pixels.len(), 192 * 108);
}

#[test]
fn audio_reads_are_exact_in_length_and_stereo() {
    let mut reader = AudioReader::open(&clip()).unwrap();
    assert_eq!(reader.sample_rate(), 48000);
    let samples = reader
        .read(Ratio::new(1, 2), Ratio::from_int(1), 44100)
        .unwrap();
    assert_eq!(samples.len(), 44100 * 2);
    let level = rms(&samples);
    assert!(level > 0.05, "expected an audible sine, got rms {level}");
    let (l, r): (Vec<f32>, Vec<f32>) = samples.chunks_exact(2).map(|p| (p[0], p[1])).unzip();
    assert!(
        (rms(&l) - rms(&r)).abs() < 1e-3,
        "mono is duplicated to both channels"
    );
    // Reading past the end pads with silence.
    let tail = reader
        .read(Ratio::new(19, 10), Ratio::from_int(1), 48000)
        .unwrap();
    assert_eq!(tail.len(), 48000 * 2);
    assert!(rms(&tail[90_000..]) < 1e-4);
}

#[test]
fn mono_output_is_written_as_mono() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mono.m4a");
    let settings = EncodeSettings {
        video: None,
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
        copied_audio: None,
        audio: Some(AudioSettings {
            codec: AudioCodec::Aac,
            bitrate_kbps: 96,
            sample_rate: 44100,
            channels: 1,
        }),
    };
    let mut enc = Encoder::new(&out, settings).unwrap();
    // Left carries the tone, right is silent: the mono file holds their
    // average at half the level.
    let tone: Vec<f32> = (0..44100 * 2)
        .map(|i| {
            if i % 2 == 0 {
                ((i / 2) as f32 * 440.0 * std::f32::consts::TAU / 44100.0).sin() * 0.5
            } else {
                0.0
            }
        })
        .collect();
    enc.push_audio(&tone).unwrap();
    enc.finish().unwrap();

    let a = probe(&out).unwrap().audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (44100, 1));
    let mut reader = AudioReader::open(&out).unwrap();
    let back = reader.read(Ratio::ZERO, Ratio::from_int(1), 44100).unwrap();
    let level = rms(&back[2000..]);
    assert!((0.12..0.24).contains(&level), "downmixed level {level}");
}

#[test]
fn encoded_solid_color_survives_the_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("solid.mp4");
    let settings = EncodeSettings {
        video: Some(VideoSettings {
            width: 320,
            height: 180,
            fps: Ratio::from_int(25),
            codec: VideoCodec::H264,
            crf: Some(16),
            preset: Some("veryfast".to_owned()),
            hardware: HardwarePolicy::Never,
            color: ResolvedTags::SDR_VIDEO,
            profile: None,
            keyframe_interval: None,
            max_bitrate_kbps: None,
            bitrate_kbps: None,
            level: None,
            tune: None,
            fixed_keyframes: false,
            hdr_metadata: None,
            stitch: None,
        }),
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
        copied_audio: None,
        audio: Some(AudioSettings {
            codec: AudioCodec::Aac,
            bitrate_kbps: 96,
            sample_rate: 48000,
            channels: 2,
        }),
    };
    let mut enc = Encoder::new(&out, settings).unwrap();
    let frame = Frame::new(320, 180, Color::from_rgba8(255, 136, 0, 255));
    let tone: Vec<f32> = (0..48000 * 2)
        .map(|i| ((i / 2) as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.5)
        .collect();
    for _ in 0..25 {
        enc.push_frame(&frame).unwrap();
    }
    enc.push_audio(&tone).unwrap();
    enc.finish().unwrap();

    let info = probe(&out).unwrap();
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height), (320, 180));
    assert_eq!(v.fps, Ratio::from_int(25));
    assert_eq!(v.frames, Some(25));
    assert_eq!(v.color.matrix, Some(Matrix::Bt709));
    assert_eq!(v.color.range, Some(Range::Limited));
    assert_eq!(v.color.transfer, Some(Transfer::Bt709));
    assert_eq!(info.audio.unwrap().sample_rate, 48000);

    let mut reader = VideoReader::open(&out, ColorTags::default()).unwrap();
    let img = reader.frame_at(Ratio::new(12, 25)).unwrap();
    let center = img.pixels[90 * 320 + 160].to_srgb8();
    for (got, want) in center[..3].iter().zip([255u8, 136, 0]) {
        assert!(
            got.abs_diff(want) <= 4,
            "decoded {center:?}, expected #ff8800"
        );
    }
    let mut audio = AudioReader::open(&out).unwrap();
    let back = audio
        .read(Ratio::new(1, 10), Ratio::new(1, 2), 48000)
        .unwrap();
    let level = rms(&back);
    assert!((level - 0.5 / 2f32.sqrt()).abs() < 0.05, "sine rms {level}");
}

#[test]
fn streamed_blocks_equal_one_whole_read() {
    // Ten blocks of a tenth of a second, resampled on one running
    // converter, are the same samples as one read of the second.
    let whole = AudioReader::open(&clip())
        .unwrap()
        .read(Ratio::ZERO, Ratio::from_int(1), 48000)
        .unwrap();
    let mut stream = AudioReader::open(&clip())
        .unwrap()
        .into_stream(Ratio::ZERO, 48000)
        .unwrap();
    let mut streamed = Vec::new();
    for _ in 0..10 {
        streamed.extend(stream.read(4800).unwrap());
    }
    assert_eq!(streamed.len(), whole.len());
    let worst = streamed
        .iter()
        .zip(&whole)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(worst < 1e-4, "blocks differ from the whole read by {worst}");
    // The mixer's blocks of any size make the same mix.
    let root = clip().parent().unwrap().to_path_buf();
    let text = r#"{
      "geneva": "0.2",
      "output": { "width": 64, "height": 64, "fps": 25, "duration": "1.5s" },
      "assets": { "clip": { "src": "clip.mp4" } },
      "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" }, "duration": "1.5s" } ] } ],
      "audio": [ { "clips": [ { "asset": "clip", "start": "0.25s", "duration": "1s", "gain_db": -3, "fade_in": "0.1s" } ] } ]
    }"#;
    let comp = load(text).composition.unwrap();
    let whole = mix::mix(&comp, &root, 48000).unwrap();
    let mut mixer = mix::Mixer::new(&comp, &root, 48000);
    let mut small = Vec::new();
    while let Some(b) = mixer.next_block(1234).unwrap() {
        small.extend(b);
    }
    assert_eq!(small.len(), whole.len());
    let worst = small
        .iter()
        .zip(&whole)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert!(worst < 1e-4, "block size changes the mix by {worst}");
}

#[test]
fn mixing_applies_gain_and_skips_muted_video_audio() {
    let root = clip().parent().unwrap().to_path_buf();
    let text = r#"{
      "geneva": "0.1",
      "output": { "width": 64, "height": 64, "fps": 25, "duration": "1s" },
      "assets": { "clip": { "src": "clip.mp4" } },
      "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip", "audio": false } } ] } ],
      "audio": [ { "clips": [ { "asset": "clip", "duration": "1s", "gain_db": -6 } ] } ]
    }"#;
    let comp = load(text).composition.unwrap();
    let mixed = mix::mix(&comp, &root, 48000).unwrap();
    assert_eq!(mixed.len(), 48000 * 2);
    let mut reader = AudioReader::open(&clip()).unwrap();
    let source = reader.read(Ratio::ZERO, Ratio::from_int(1), 48000).unwrap();
    let ratio = rms(&mixed) / rms(&source);
    assert!(
        (ratio - 0.501).abs() < 0.03,
        "-6 dB should halve the level, got ratio {ratio}"
    );
}

#[test]
fn audio_lands_at_its_timeline_position_in_the_output_file() {
    // A tone placed at 1s on an otherwise silent timeline must start at 1s
    // in the muxed file, which checks audio/video alignment end to end.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sync.mp4");
    let root = clip().parent().unwrap().to_path_buf();
    let text = r#"{
      "geneva": "0.1",
      "output": { "width": 64, "height": 64, "fps": 25, "duration": "2s" },
      "assets": { "clip": { "src": "clip.mp4" } },
      "layers": [ { "clips": [ { "source": { "kind": "solid", "color": "black" } } ] } ],
      "audio": [ { "clips": [ { "asset": "clip", "start": "1s", "duration": "1s" } ] } ]
    }"#;
    let comp = load(text).composition.unwrap();
    let settings = EncodeSettings {
        video: Some(VideoSettings {
            width: 64,
            height: 64,
            fps: Ratio::from_int(25),
            codec: VideoCodec::H264,
            crf: Some(30),
            preset: Some("ultrafast".to_owned()),
            hardware: HardwarePolicy::Never,
            color: ResolvedTags::SDR_VIDEO,
            profile: None,
            keyframe_interval: None,
            max_bitrate_kbps: None,
            bitrate_kbps: None,
            level: None,
            tune: None,
            fixed_keyframes: false,
            hdr_metadata: None,
            stitch: None,
        }),
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
        copied_audio: None,
        audio: Some(AudioSettings {
            codec: AudioCodec::Aac,
            bitrate_kbps: 96,
            sample_rate: 48000,
            channels: 2,
        }),
    };
    let mut enc = Encoder::new(&out, settings).unwrap();
    let frame = Frame::new(64, 64, Color::BLACK);
    for _ in 0..50 {
        enc.push_frame(&frame).unwrap();
    }
    enc.push_audio(&mix::mix(&comp, &root, 48000).unwrap())
        .unwrap();
    enc.finish().unwrap();

    let mut audio = AudioReader::open(&out).unwrap();
    let all = audio.read(Ratio::ZERO, Ratio::from_int(2), 48000).unwrap();
    let before = rms(&all[..48000 * 2 * 9 / 10]);
    let after = rms(&all[48000 * 2 * 11 / 10..48000 * 2 * 15 / 10]);
    assert!(
        before < 0.01,
        "silence expected before 1s, got rms {before}"
    );
    assert!(after > 0.05, "tone expected after 1s, got rms {after}");
    let info = probe(&out).unwrap();
    let d = info.duration.unwrap().to_f64();
    assert!((d - 2.0).abs() < 0.1, "duration {d}");
}

#[test]
fn stream_copy_trims_at_keyframes_and_joins_compatible_sources() {
    use geneva_media::{plan_stream_copy, stream_copy};
    use geneva_timeline::schema::Container;
    let dir = tempfile::tempdir().unwrap();
    let root = clip().parent().unwrap().to_path_buf();

    // A plain cut: one video clip at natural size, no other layers.
    let text = r#"{
      "geneva": "0.1",
      "output": { "width": 192, "height": 108, "fps": 25 },
      "assets": { "clip": { "src": "clip.mp4" } },
      "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip", "in": "0.6s", "out": "1.5s" } } ] } ]
    }"#;
    let comp = load(text).composition.unwrap();
    let plan = plan_stream_copy(&comp, &root, Container::Mp4, None)
        .unwrap()
        .expect("copyable");
    assert_eq!(plan.audio.len(), 1);
    let out = dir.path().join("cut.mp4");
    let report = stream_copy(&plan, &out, &[], true).unwrap();
    // Keyframes every 12 frames at 25 fps: the cut moves back to 0.48 s.
    assert_eq!(report.segments[0].1, Ratio::new(12, 25));
    assert!(!report.notes().is_empty());
    let info = probe(&out).unwrap();
    let v = info.video.unwrap();
    assert_eq!(v.codec, "h264");
    assert_eq!((v.width, v.height), (192, 108));
    // 0.48 s to 1.5 s is 25.5 frames; packets cover whole frames.
    assert!(
        (25..=26).contains(&v.frames.unwrap()),
        "frames {:?}",
        v.frames
    );
    assert!(info.audio.is_some());

    // Joining the same file twice is a copy; the output is twice as long.
    let text = r#"{
      "geneva": "0.1",
      "output": { "width": 192, "height": 108, "fps": 25 },
      "assets": { "clip": { "src": "clip.mp4" } },
      "layers": [ { "clips": [
        { "source": { "kind": "video", "asset": "clip", "out": "2s" } },
        { "source": { "kind": "video", "asset": "clip", "out": "2s" } } ] } ]
    }"#;
    let comp = load(text).composition.unwrap();
    let plan = plan_stream_copy(&comp, &root, Container::Mp4, None)
        .unwrap()
        .expect("copyable");
    assert_eq!(plan.segments.len(), 2);
    let out = dir.path().join("joined.mp4");
    let report = stream_copy(&plan, &out, &[], true).unwrap();
    assert_eq!(report.video_packets, 100);
    let info = probe(&out).unwrap();
    assert_eq!(info.video.unwrap().frames, Some(100));
    let d = info.duration.unwrap().to_f64();
    assert!((d - 4.0).abs() < 0.1, "duration {d}");
    // Video and audio both decode across the join.
    let mut reader = VideoReader::open(&out, ColorTags::default()).unwrap();
    let late = reader.frame_at(Ratio::new(7, 2)).unwrap();
    assert_eq!(late.pixels.len(), 192 * 108);
    let mut audio = AudioReader::open(&out).unwrap();
    let tail = audio
        .read(Ratio::from_int(3), Ratio::new(1, 2), 48000)
        .unwrap();
    assert!(rms(&tail) > 0.05);
}

#[test]
fn stream_copy_tolerates_a_printed_duration() {
    use geneva_media::plan_stream_copy;
    use geneva_timeline::schema::Container;
    let root = clip().parent().unwrap().to_path_buf();
    // A duration written with six decimals can differ from a clip's exact
    // length by a fraction of a millisecond; a real gap is still refused.
    let cases = [
        (
            r#""output": { "width": 192, "height": 108, "fps": 25, "duration": "2.0005s" }, "assets": { "clip": { "src": "clip.mp4" } },
           "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" } } ] } ]"#,
            Container::Mp4,
            true,
        ),
        (
            r#""output": { "width": 192, "height": 108, "fps": 25, "duration": "2.01s" }, "assets": { "clip": { "src": "clip.mp4" } },
           "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" } } ] } ]"#,
            Container::Mp4,
            false,
        ),
        (
            r#""output": { "width": 192, "height": 108, "fps": 25, "duration": "2.0005s" }, "assets": { "clip": { "src": "clip.mp4" } },
           "audio": [ { "clips": [ { "asset": "clip" } ] } ]"#,
            Container::M4a,
            true,
        ),
        (
            r#""output": { "width": 192, "height": 108, "fps": 25, "duration": "2.01s" }, "assets": { "clip": { "src": "clip.mp4" } },
           "audio": [ { "clips": [ { "asset": "clip" } ] } ]"#,
            Container::M4a,
            false,
        ),
    ];
    for (body, container, copyable) in cases {
        let text = format!(r#"{{"geneva":"0.1",{body}}}"#);
        let loaded = geneva_timeline::load_with(&text, &Durations);
        let comp = loaded
            .composition
            .unwrap_or_else(|| panic!("{:#?}", loaded.diagnostics));
        assert_eq!(
            plan_stream_copy(&comp, &root, container, None)
                .unwrap()
                .is_some(),
            copyable,
            "{body}"
        );
    }
}

#[test]
fn overlays_laid_onto_direct_frames_match_the_compositor() {
    use geneva_media::convert::{PlaneFormat, blend_overlay, frame_to_planes};
    use geneva_media::{DirectSource, MediaAssets};
    use geneva_render::{CpuRenderer, Renderer};

    let root = clip().parent().unwrap().to_path_buf();
    let text = r#"{
      "geneva": "0.1",
      "output": { "width": 192, "height": 108, "fps": 25, "duration": "2s" },
      "assets": { "clip": { "src": "clip.mp4" } },
      "layers": [
        { "clips": [ { "source": { "kind": "video", "asset": "clip" } } ] },
        { "clips": [ { "source": { "kind": "text", "text": "Hi there", "size": 20, "color": "yellow", "outline": { "color": "black", "width": 2 } },
                      "start": "0.5s", "duration": "1s",
                      "transform": { "position": { "x": "50%", "y": "90%" }, "anchor": { "x": "50%", "y": "100%" } } } ] }
      ]
    }"#;
    let comp = load(text).composition.unwrap();
    assert!(CpuRenderer::<MediaAssets>::overlays_are_plain(&comp));
    assert!(
        DirectSource::open(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
            .unwrap()
            .is_none(),
        "a second layer is not a plain transcode"
    );
    let mut base = DirectSource::open_base(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
        .unwrap()
        .expect("the video with overlays qualifies");
    let mut renderer = CpuRenderer::new(MediaAssets::new(root.clone()));

    // Before the caption: nothing to draw, frames are the decoder's.
    let t = comp.frame_time(2);
    assert!(renderer.render_overlays(&comp, t).unwrap().is_none());

    // During the caption: only its box changes, and the result agrees
    // with the full compositor there.
    let t = comp.frame_time(20);
    let mut planes = base.frame(t).unwrap();
    let before = planes.clone();
    let (overlay, rect) = renderer
        .render_overlays(&comp, t)
        .unwrap()
        .expect("caption shown");
    assert!(rect[1] > 50 && rect[3] <= 108, "{rect:?}");
    blend_overlay(&mut planes, &overlay, rect, comp.color);
    let slow = frame_to_planes(
        &renderer.render_frame(&comp, t).unwrap(),
        comp.color,
        PlaneFormat::Yuv420p8,
    );
    let (fast, was, full) = (
        &planes.planes[0].data,
        &before.planes[0].data,
        &slow.planes[0].data,
    );
    let (mut inside, mut n_inside, mut changed_outside) = (0.0f64, 0.0f64, 0usize);
    for y in 0..108usize {
        for x in 0..192usize {
            let i = y * 192 + x;
            let in_rect = x >= rect[0] as usize
                && x < rect[2] as usize
                && y >= rect[1] as usize
                && y < rect[3] as usize;
            if in_rect {
                inside += (f64::from(fast[i]) - f64::from(full[i])).abs();
                n_inside += 1.0;
            } else if fast[i] != was[i] {
                changed_outside += 1;
            }
        }
    }
    assert_eq!(changed_outside, 0, "pixels outside the overlay box changed");
    assert!(
        inside / n_inside < 4.0,
        "mean luma difference in the box: {}",
        inside / n_inside
    );
    // The caption did land: something in the box differs from the plain frame.
    assert!(fast != was);
}

#[test]
fn stream_copy_is_refused_when_anything_would_change_the_picture() {
    use geneva_media::plan_stream_copy;
    use geneva_timeline::schema::Container;
    let root = clip().parent().unwrap().to_path_buf();
    let cases = [
        // Resized output.
        r#""output": { "width": 384, "height": 216, "fps": 25 }, "assets": { "clip": { "src": "clip.mp4" } },
           "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" } } ] } ]"#,
        // Different frame rate.
        r#""output": { "width": 192, "height": 108, "fps": 30 }, "assets": { "clip": { "src": "clip.mp4" } },
           "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" } } ] } ]"#,
        // An overlay on top.
        r#""output": { "width": 192, "height": 108, "fps": 25 }, "assets": { "clip": { "src": "clip.mp4" } },
           "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" } } ] },
                       { "clips": [ { "source": { "kind": "solid", "color": "white" }, "opacity": 0.2, "duration": "2s" } ] } ]"#,
        // Reduced opacity.
        r#""output": { "width": 192, "height": 108, "fps": 25 }, "assets": { "clip": { "src": "clip.mp4" } },
           "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" }, "opacity": 0.5 } ] } ]"#,
    ];
    for body in cases {
        let text = format!(r#"{{"geneva":"0.1",{body}}}"#);
        let loaded = geneva_timeline::load_with(&text, &Durations);
        let comp = loaded
            .composition
            .unwrap_or_else(|| panic!("{:#?}", loaded.diagnostics));
        assert!(
            plan_stream_copy(&comp, &root, Container::Mp4, None)
                .unwrap()
                .is_none(),
            "{body}"
        );
    }
    // WebM cannot hold H.264, and a different requested codec forces encoding.
    let text = r#"{"geneva":"0.1","output": { "width": 192, "height": 108, "fps": 25 }, "assets": { "clip": { "src": "clip.mp4" } },
           "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip", "out": "2s" } } ] } ]}"#;
    let comp = load(text).composition.unwrap();
    assert!(
        plan_stream_copy(&comp, &root, Container::Webm, None)
            .unwrap()
            .is_none()
    );
    assert!(
        plan_stream_copy(
            &comp,
            &root,
            Container::Mp4,
            Some(geneva_timeline::schema::VideoCodec::Vp9)
        )
        .unwrap()
        .is_none()
    );
    assert!(
        plan_stream_copy(
            &comp,
            &root,
            Container::Mkv,
            Some(geneva_timeline::schema::VideoCodec::H264)
        )
        .unwrap()
        .is_some()
    );
}

/// Asset information that knows the test clip is two seconds long.
struct Durations;

impl geneva_timeline::AssetInfo for Durations {
    fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
        Some(Ratio::from_int(2))
    }
}

#[test]
fn audio_only_outputs_round_trip_through_wav() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("tone.wav");
    let settings = EncodeSettings {
        video: None,
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
        copied_audio: None,
        audio: Some(AudioSettings {
            codec: AudioCodec::Pcm,
            bitrate_kbps: 0,
            sample_rate: 48000,
            channels: 2,
        }),
    };
    let mut enc = Encoder::new(&out, settings).unwrap();
    let tone: Vec<f32> = (0..48000 * 2)
        .map(|i| ((i / 2) as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.5)
        .collect();
    enc.push_audio(&tone).unwrap();
    assert!(enc.push_frame(&Frame::new(2, 2, Color::BLACK)).is_err());
    enc.finish().unwrap();

    let info = probe(&out).unwrap();
    assert!(info.video.is_none());
    assert_eq!(info.audio.as_ref().unwrap().channels, 2);
    let mut reader = AudioReader::open(&out).unwrap();
    let back = reader.read(Ratio::ZERO, Ratio::from_int(1), 48000).unwrap();
    assert_eq!(back.len(), 48000 * 2);
    let rms_in = rms(&tone[..2000]);
    let rms_out = rms(&back[..2000]);
    assert!((rms_in - rms_out).abs() < 0.01, "{rms_in} vs {rms_out}");
}

#[test]
fn direct_frames_match_the_reference_renderer() {
    use geneva_media::convert::{PlaneFormat, frame_to_planes};
    use geneva_media::{DirectSource, MediaAssets};
    use geneva_render::{CpuRenderer, Renderer};

    let root = clip().parent().unwrap().to_path_buf();
    let doc = |clip_extra: &str| {
        format!(
            r#"{{
              "geneva": "0.1",
              "output": {{ "width": 192, "height": 108, "fps": 25 }},
              "assets": {{ "clip": {{ "src": "clip.mp4" }} }},
              "layers": [ {{ "clips": [ {{
                "source": {{ "kind": "video", "asset": "clip", "in": "0.5s", "out": "1.5s" }}{clip_extra}
              }} ] }} ]
            }}"#
        )
    };
    let comp = load(&doc("")).composition.unwrap();
    let mut direct = DirectSource::open(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
        .unwrap()
        .expect("a plain cut qualifies");
    let mut renderer = CpuRenderer::new(MediaAssets::new(root.clone()));
    for n in [0u64, 7, 24] {
        let t = comp.frame_time(n);
        let fast = direct.frame(t).unwrap();
        let slow = frame_to_planes(
            &renderer.render_frame(&comp, t).unwrap(),
            comp.color,
            PlaneFormat::Yuv420p8,
        );
        let (fast, slow) = (&fast.planes[0].data, &slow.planes[0].data);
        assert_eq!(fast.len(), slow.len());
        // The reference path resamples chroma up and back down through
        // linear light, which moves luma at hard edges in both directions;
        // the two must agree closely on average and show no bias.
        let n_px = fast.len() as f64;
        let (mut abs, mut signed) = (0.0f64, 0.0f64);
        for (a, b) in fast.iter().zip(slow.iter()) {
            let d = f64::from(*a) - f64::from(*b);
            abs += d.abs();
            signed += d;
        }
        let (abs, signed) = (abs / n_px, signed / n_px);
        assert!(abs < 4.0, "frame {n}: mean luma difference {abs}");
        assert!(signed.abs() < 0.5, "frame {n}: luma bias {signed}");
    }

    // An RGB output (PNG) takes the same path: the scaler applies the
    // source's matrix and a table re-encodes its transfer curve as sRGB.
    let rgb_tags = geneva_media::output_tags_for(VideoCodec::Png, comp.color);
    let mut direct = DirectSource::open(&comp, &root, PlaneFormat::Rgba8, rgb_tags)
        .unwrap()
        .expect("an RGB output qualifies");
    for n in [0u64, 7, 24] {
        let t = comp.frame_time(n);
        let fast = direct.frame(t).unwrap();
        let slow = frame_to_planes(
            &renderer.render_frame(&comp, t).unwrap(),
            rgb_tags,
            PlaneFormat::Rgba8,
        );
        let (fast, slow) = (&fast.planes[0].data, &slow.planes[0].data);
        assert_eq!(fast.len(), slow.len());
        let (mut abs, mut signed, mut count) = (0.0f64, 0.0f64, 0.0f64);
        for (a, b) in fast.chunks_exact(4).zip(slow.chunks_exact(4)) {
            assert_eq!(a[3], 255, "frame {n}: opaque");
            for c in 0..3 {
                let d = f64::from(a[c]) - f64::from(b[c]);
                abs += d.abs();
                signed += d;
                count += 1.0;
            }
        }
        let (abs, signed) = (abs / count, signed / count);
        assert!(abs < 4.0, "frame {n}: mean RGB difference {abs}");
        assert!(signed.abs() < 0.5, "frame {n}: RGB bias {signed}");
    }

    // A picture fitted onto a larger frame of one color (a landscape
    // video on a portrait canvas) is scaled to its place on the direct
    // path too, and the bars come out in the background color.
    let portrait = r##"{
      "geneva": "0.1",
      "output": { "width": 108, "height": 192, "fps": 25, "background": "#336699" },
      "assets": { "clip": { "src": "clip.mp4" } },
      "layers": [ { "clips": [ {
        "source": { "kind": "video", "asset": "clip", "in": "0.5s", "out": "1.5s" }
      } ] } ]
    }"##;
    let comp = load(portrait).composition.unwrap();
    let mut direct = DirectSource::open(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
        .unwrap()
        .expect("a fitted picture on an opaque background qualifies");
    assert!(
        direct.reason().contains("onto the background"),
        "{}",
        direct.reason()
    );
    for n in [0u64, 24] {
        let t = comp.frame_time(n);
        let fast = direct.frame(t).unwrap();
        let slow = frame_to_planes(
            &renderer.render_frame(&comp, t).unwrap(),
            comp.color,
            PlaneFormat::Yuv420p8,
        );
        assert_eq!((fast.width, fast.height), (108, 192));
        for (i, (a, b)) in fast.planes.iter().zip(&slow.planes).enumerate() {
            assert_eq!(a.data.len(), b.data.len(), "plane {i}");
            let n_px = a.data.len() as f64;
            let abs: f64 = a
                .data
                .iter()
                .zip(&b.data)
                .map(|(x, y)| (f64::from(*x) - f64::from(*y)).abs())
                .sum::<f64>()
                / n_px;
            assert!(abs < 4.0, "frame {n}, plane {i}: mean difference {abs}");
        }
        // The top rows are bars: the background, not the picture.
        let top = &fast.planes[0].data[..fast.planes[0].stride];
        let reference = slow.planes[0].data[0];
        assert!(
            top.iter().all(|v| v.abs_diff(reference) <= 1),
            "frame {n}: bars are not the background"
        );
    }
    // A `cover` fit crops instead: the region of the source that the
    // frame shows is scaled to the whole frame, no bars.
    let comp = load(&portrait.replace(r#""out": "1.5s" }"#, r#""out": "1.5s" }, "fit": "cover""#))
        .composition
        .unwrap();
    let mut direct = DirectSource::open(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
        .unwrap()
        .expect("a cropped picture qualifies");
    assert!(direct.reason().contains("cropped"), "{}", direct.reason());
    for n in [0u64, 24] {
        let t = comp.frame_time(n);
        let fast = direct.frame(t).unwrap();
        let slow = frame_to_planes(
            &renderer.render_frame(&comp, t).unwrap(),
            comp.color,
            PlaneFormat::Yuv420p8,
        );
        let (a, b) = (&fast.planes[0].data, &slow.planes[0].data);
        assert_eq!(a.len(), b.len());
        let abs: f64 = a
            .iter()
            .zip(b)
            .map(|(x, y)| (f64::from(*x) - f64::from(*y)).abs())
            .sum::<f64>()
            / a.len() as f64;
        assert!(abs < 6.0, "frame {n}: mean luma difference {abs}");
    }

    // A translucent background needs the compositor.
    let comp = load(&portrait.replace("#336699", "#33669980"))
        .composition
        .unwrap();
    assert!(
        DirectSource::open(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
            .unwrap()
            .is_none()
    );

    // Anything that changes the picture disqualifies the direct path.
    let comp = load(&doc(r#", "opacity": 0.5"#)).composition.unwrap();
    assert!(
        DirectSource::open(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
            .unwrap()
            .is_none()
    );
}

fn solid_settings(
    codec: VideoCodec,
    profile: Option<geneva_timeline::schema::VideoProfile>,
    width: u32,
    height: u32,
) -> EncodeSettings {
    EncodeSettings {
        video: Some(VideoSettings {
            width,
            height,
            fps: Ratio::from_int(25),
            codec,
            crf: None,
            preset: None,
            hardware: HardwarePolicy::Never,
            color: ResolvedTags::SDR_VIDEO,
            profile,
            keyframe_interval: None,
            max_bitrate_kbps: None,
            bitrate_kbps: None,
            level: None,
            tune: None,
            fixed_keyframes: false,
            hdr_metadata: None,
            stitch: None,
        }),
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
        copied_audio: None,
        audio: None,
    }
}

#[test]
fn system_x264_is_used_when_installed() {
    let Some(lib) = geneva_media::system_x264() else {
        eprintln!(
            "no system x264 here; nothing to check ({:?})",
            geneva_media::system_x264_error()
        );
        return;
    };
    eprintln!("system x264 build {} from {}", lib.build, lib.path);
    let dir = tempfile::tempdir().unwrap();
    let color = Color::from_rgba8(30, 160, 90, 255);
    for name in ["x264.mp4", "x264.mkv", "x264.mov", "x264.mxf"] {
        let out = dir.path().join(name);
        let mut enc = Encoder::new(&out, solid_settings(VideoCodec::H264, None, 192, 108)).unwrap();
        let note = enc.video_encoder_note().unwrap_or_default();
        assert!(note.contains("system's x264 (build"), "{name}: {note}");
        let frame = Frame::new(192, 108, color);
        for _ in 0..30 {
            enc.push_frame(&frame).unwrap();
        }
        enc.finish().unwrap();
        let info = probe(&out).unwrap();
        let v = info.video.unwrap();
        assert_eq!(v.codec, "h264", "{name}");
        assert_eq!((v.width, v.height), (192, 108), "{name}");
        let mut reader = VideoReader::open(&out, ColorTags::default()).unwrap();
        let px = reader.frame_at(Ratio::new(1, 2)).unwrap().pixels[192 * 54 + 96].to_srgb8();
        for (got, want) in px[..3].iter().zip([30u8, 160, 90]) {
            assert!(
                got.abs_diff(want) <= 4,
                "{name}: got {px:?}, wanted (30, 160, 90)"
            );
        }
    }
}

#[test]
fn intermediate_codecs_keep_a_solid_color_through_ten_bit_layouts() {
    use geneva_timeline::schema::VideoProfile;
    let dir = tempfile::tempdir().unwrap();
    let color = Color::from_rgba8(255, 136, 0, 255);
    for (name, codec, profile, w, h) in [
        ("prores.mov", VideoCodec::Prores, None, 320, 180),
        (
            "prores4444.mov",
            VideoCodec::Prores,
            Some(VideoProfile::P4444),
            320,
            180,
        ),
        ("dnxhr.mxf", VideoCodec::Dnxhd, None, 256, 144),
        (
            "dnxhrx.mov",
            VideoCodec::Dnxhd,
            Some(VideoProfile::DnxhrHqx),
            256,
            144,
        ),
        ("png.mov", VideoCodec::Png, None, 64, 64),
        ("mjpeg.mkv", VideoCodec::Mjpeg, None, 64, 64),
    ] {
        let out = dir.path().join(name);
        let mut enc = Encoder::new(&out, solid_settings(codec, profile, w, h)).unwrap();
        let frame = Frame::new(w, h, color);
        for _ in 0..5 {
            enc.push_frame(&frame).unwrap();
        }
        enc.finish().unwrap();
        let mut reader = VideoReader::open(&out, ColorTags::default()).unwrap();
        let px = reader.frame_at(Ratio::ZERO).unwrap().pixels[w as usize * 10 + 10].to_srgb8();
        let tolerance = if codec == VideoCodec::Mjpeg { 6 } else { 3 };
        for (got, want) in px[..3].iter().zip([255u8, 136, 0]) {
            assert!(
                got.abs_diff(want) <= tolerance,
                "{name}: got {px:?}, wanted (255, 136, 0)"
            );
        }
    }
}

#[test]
fn every_audio_codec_round_trips_a_tone() {
    let dir = tempfile::tempdir().unwrap();
    let tone: Vec<f32> = (0..48000 * 2)
        .map(|i| ((i / 2) as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.5)
        .collect();
    for (name, codec) in [
        ("tone.mp3", AudioCodec::Mp3),
        ("tone.ogg", AudioCodec::Vorbis),
        ("tone.m4a", AudioCodec::Alac),
        ("tone.mkv", AudioCodec::Ac3),
        ("tone.wav", AudioCodec::Pcm24),
    ] {
        let out = dir.path().join(name);
        let settings = EncodeSettings {
            video: None,
            container: None,
            subtitles: Vec::new(),
            fast_start: true,
            copied_audio: None,
            audio: Some(AudioSettings {
                codec,
                bitrate_kbps: 160,
                sample_rate: 48000,
                channels: 2,
            }),
        };
        let mut enc = Encoder::new(&out, settings).unwrap();
        enc.push_audio(&tone).unwrap();
        enc.finish().unwrap();
        let mut reader = AudioReader::open(&out).unwrap();
        let back = reader
            .read(Ratio::new(1, 4), Ratio::new(1, 2), 48000)
            .unwrap();
        let rms_out = rms(&back);
        assert!(
            (rms_out - 0.3535).abs() < 0.05,
            "{name}: rms {rms_out} after {codec:?}"
        );
    }
}

#[test]
fn subtitle_tracks_are_written_as_streams_and_read_back() {
    use geneva_media::SubtitleSettings;
    use geneva_media::subtitles::{Cue, parse};

    let dir = tempfile::tempdir().unwrap();
    let cues = parse("1\n00:00:00,200 --> 00:00:00,900\nHello <i>there</i>\n\n2\n00:00:01,000 --> 00:00:01,800\nSecond\nline\n").unwrap();
    for (name, keeps_tags) in [("subs.mp4", false), ("subs.mkv", true), ("subs.webm", true)] {
        let out = dir.path().join(name);
        let codec = if name.ends_with("webm") {
            VideoCodec::Vp9
        } else {
            VideoCodec::H264
        };
        let mut settings = solid_settings(codec, None, 64, 64);
        settings.subtitles = vec![SubtitleSettings {
            language: Some("en".to_owned()),
            title: None,
            cues: cues.clone(),
        }];
        let mut enc = Encoder::new(&out, settings).unwrap();
        let frame = Frame::new(64, 64, Color::BLACK);
        for _ in 0..50 {
            enc.push_frame(&frame).unwrap();
        }
        enc.finish().unwrap();

        let info = probe(&out).unwrap();
        assert_eq!(info.subtitles.len(), 1, "{name}");
        assert_eq!(info.subtitles[0].language.as_deref(), Some("eng"), "{name}");
        let back = geneva_media::read_subtitles(&out, 0).unwrap();
        let expected: Vec<Cue> = cues
            .iter()
            .map(|c| Cue {
                start: c.start,
                end: c.end,
                text: if keeps_tags {
                    c.text.clone()
                } else {
                    geneva_media::subtitles::strip_tags(&c.text)
                },
            })
            .collect();
        assert_eq!(back, expected, "{name}");
    }
}

#[test]
fn scaled_and_repacked_direct_frames_match_the_reference_renderer() {
    use geneva_media::convert::{PlaneFormat, frame_to_planes};
    use geneva_media::{DirectSource, MediaAssets};
    use geneva_render::{CpuRenderer, Renderer};

    let root = clip().parent().unwrap().to_path_buf();
    let doc = |width: u32, height: u32| {
        format!(
            r#"{{
              "geneva": "0.1",
              "output": {{ "width": {width}, "height": {height}, "fps": 25 }},
              "assets": {{ "clip": {{ "src": "clip.mp4" }} }},
              "layers": [ {{ "clips": [ {{
                "source": {{ "kind": "video", "asset": "clip", "in": "0.5s", "out": "1.5s" }}
              }} ] }} ]
            }}"#
        )
    };
    // A downscale and a 10-bit 4:2:2 repack at the source size: both
    // qualify, and both must agree with the linear-light reference within
    // the difference between gamma-space and linear-light resampling.
    let cases = [
        (96, 54, PlaneFormat::Yuv420p8, 1u32),
        (192, 108, PlaneFormat::Yuv422p10, 4),
        (96, 54, PlaneFormat::Yuv422p10, 4),
    ];
    for (width, height, format, unit) in cases {
        let comp = load(&doc(width, height)).composition.unwrap();
        let mut direct = DirectSource::open(&comp, &root, format, comp.color)
            .unwrap()
            .expect("a fitted clip qualifies");
        assert!(direct.reason().contains("scaled and repacked"));
        let mut renderer = CpuRenderer::new(MediaAssets::new(root.clone()));
        for n in [0u64, 12] {
            let t = comp.frame_time(n);
            let fast = direct.frame(t).unwrap();
            let slow = frame_to_planes(
                &renderer.render_frame(&comp, t).unwrap(),
                comp.color,
                format,
            );
            assert_eq!(fast.planes[0].width, width as usize);
            assert_eq!(fast.planes[0].height, height as usize);
            let luma = |planes: &geneva_media::convert::Planes| -> Vec<f64> {
                let p = &planes.planes[0];
                if unit == 1 {
                    p.data.iter().map(|v| f64::from(*v)).collect()
                } else {
                    p.data
                        .chunks_exact(2)
                        .map(|b| f64::from(u16::from_le_bytes([b[0], b[1]])))
                        .collect()
                }
            };
            let (fast, slow) = (luma(&fast), luma(&slow));
            assert_eq!(fast.len(), slow.len());
            let n_px = fast.len() as f64;
            let (mut abs, mut signed) = (0.0f64, 0.0f64);
            for (a, b) in fast.iter().zip(slow.iter()) {
                abs += (a - b).abs();
                signed += a - b;
            }
            let (abs, signed) = (
                abs / n_px / f64::from(unit),
                signed / n_px / f64::from(unit),
            );
            assert!(
                abs < 5.0,
                "{width}x{height} {format:?} frame {n}: mean luma difference {abs}"
            );
            assert!(
                signed.abs() < 1.5,
                "{width}x{height} {format:?} frame {n}: luma bias {signed}"
            );
        }
    }

    // A picture that does not fill the frame is scaled to its place on
    // the background (opaque black by default).
    let comp = load(&doc(96, 96)).composition.unwrap();
    let mut direct = DirectSource::open(&comp, &root, PlaneFormat::Yuv420p8, comp.color)
        .unwrap()
        .expect("a fitted picture qualifies");
    let planes = direct.frame(comp.frame_time(0)).unwrap();
    assert_eq!((planes.width, planes.height), (96, 96));
    let luma = &planes.planes[0];
    assert!(
        luma.data[..luma.stride].iter().all(|v| *v <= 16),
        "top bar is black"
    );
    assert!(
        luma.data[48 * luma.stride..][..luma.stride]
            .iter()
            .any(|v| *v > 16),
        "middle shows the picture"
    );
}

#[test]
fn smart_cut_plans_copies_between_keyframes_and_clean_boundaries() {
    use geneva_media::{Segment, plan_smart_cut, read_copied, system_x264};
    use geneva_timeline::schema::Container;

    if system_x264().is_none() {
        eprintln!("no system x264; skipping");
        return;
    }
    let root = clip().parent().unwrap().to_path_buf();
    // 0.6 s into a 2 s clip with a keyframe every 12 frames: frame 15 of
    // the source is the first wanted, 24 the first IDR after it.
    let comp = load(
        r#"{
          "geneva": "0.1",
          "output": { "width": 192, "height": 108, "fps": 25 },
          "assets": { "clip": { "src": "clip.mp4" } },
          "layers": [ { "clips": [ {
            "source": { "kind": "video", "asset": "clip", "in": "0.6s", "out": "1.6s" }
          } ] } ]
        }"#,
    )
    .composition
    .unwrap();
    let plan = plan_smart_cut(&comp, &root, Container::Mp4, None, comp.color)
        .unwrap()
        .expect("an exact cut of an H.264 clip qualifies");
    assert_eq!(plan.copied_frames, 16);
    assert_eq!(plan.encoded_frames, 9);
    assert_eq!(plan.segments.len(), 2);
    assert_eq!(plan.segments[0], Segment::Encode { frames: 0..9 });
    let Segment::Copy {
        source,
        packets,
        offset,
        frames,
    } = plan.segments[1].clone()
    else {
        panic!("second segment copies");
    };
    assert_eq!(
        (source, packets.clone(), offset, frames),
        (0, 24..40, -15, 9..25)
    );
    assert!(plan.sources[0].packet(24).idr);
    // Only the pictures around the cut were indexed: from the keyframe
    // before the first wanted picture to the IDR after the last.
    assert_eq!(plan.sources[0].base, 12);
    assert_eq!(plan.sources[0].packets.len(), 36);
    assert_eq!(plan.reorder, 2, "B-pyramid decodes two pictures ahead");
    assert_eq!(plan.sps_id, 1);
    // The clip's own AAC goes into the MP4 as coded.
    let audio = plan.audio.as_ref().expect("audio copied");
    assert_eq!(audio.segments.len(), 1);
    assert_eq!(
        (
            audio.segments[0].from,
            audio.segments[0].to,
            audio.segments[0].offset
        ),
        (Ratio::new(3, 5), Ratio::new(8, 5), Ratio::ZERO)
    );

    // The copied packets cover exactly the frames of the stretch.
    let mut shown = Vec::new();
    read_copied(&plan.sources[source], packets, offset, &mut |p| {
        shown.push(p.frame);
        true
    })
    .unwrap();
    shown.sort_unstable();
    assert_eq!(shown, (9..25).collect::<Vec<i64>>());
    assert!(plan.reason().contains("16 of 25"));

    // A cut on a keyframe copies everything; a re-encode is never planned.
    let comp = load(
        r#"{
          "geneva": "0.1",
          "output": { "width": 192, "height": 108, "fps": 25 },
          "assets": { "clip": { "src": "clip.mp4" } },
          "layers": [ { "clips": [ {
            "source": { "kind": "video", "asset": "clip", "in": "0.48s", "out": "1.44s" }
          } ] } ]
        }"#,
    )
    .composition
    .unwrap();
    let plan = plan_smart_cut(&comp, &root, Container::Mp4, None, comp.color)
        .unwrap()
        .unwrap();
    assert_eq!((plan.copied_frames, plan.encoded_frames), (24, 0));

    // Anything but H.264 in the output rules it out.
    assert!(
        plan_smart_cut(&comp, &root, Container::Webm, None, comp.color)
            .unwrap()
            .is_none()
    );
}

/// A frame of vertical stripes with the given period: cheap to code
/// on its own (every row repeats the one above), expensive to predict
/// from a flat frame.
fn stripes_frame(width: u32, height: u32, period: usize) -> Frame {
    let mut frame = Frame::new(width, height, Color::BLACK);
    let w = width as usize;
    for (i, px) in frame.pixels_mut().iter_mut().enumerate() {
        let v = if (i % w) / period % 2 == 0 { 30 } else { 225 };
        *px = Color::from_rgba8(v, v, v, 255).to_linear();
    }
    frame
}

/// Positions of the keyframes in a file's video stream, as packet
/// indices in decode order (the display index too, for closed GOPs).
fn keyframe_positions(path: &Path) -> Vec<usize> {
    let mut ictx = ffmpeg_next::format::input(path).unwrap();
    let video = ictx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .unwrap()
        .index();
    let mut keys = Vec::new();
    let mut n = 0;
    for (stream, packet) in ictx.packets() {
        if stream.index() != video {
            continue;
        }
        if packet.is_key() {
            keys.push(n);
        }
        n += 1;
    }
    keys
}

/// Encodes 72 frames at 24 fps, flat gray then stripes from frame 36:
/// a hard scene change.
fn encode_scene_cut(out: &Path, tune: Option<VideoTune>, fixed_keyframes: bool) {
    let mut settings = solid_settings(VideoCodec::H264, None, 192, 108);
    let v = settings.video.as_mut().unwrap();
    v.fps = Ratio::from_int(24);
    v.crf = Some(23);
    v.preset = Some("veryfast".to_owned());
    v.keyframe_interval = Some(1.0);
    v.tune = tune;
    v.fixed_keyframes = fixed_keyframes;
    let mut enc = Encoder::new(out, settings).unwrap();
    let before = Frame::new(192, 108, Color::from_rgba8(128, 128, 128, 255));
    let after = stripes_frame(192, 108, 8);
    for n in 0..72 {
        enc.push_frame(if n < 36 { &before } else { &after })
            .unwrap();
    }
    enc.finish().unwrap();
}

#[test]
fn fixed_keyframes_pin_x264_keyframes_to_the_interval() {
    if geneva_media::system_x264().is_none() {
        eprintln!("no system x264 here; nothing to check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let free = dir.path().join("free.mp4");
    encode_scene_cut(&free, None, false);
    let keys = keyframe_positions(&free);
    assert!(
        keys.contains(&36),
        "x264 puts a keyframe at the scene change by default; got {keys:?}"
    );
    let fixed = dir.path().join("fixed.mp4");
    encode_scene_cut(&fixed, None, true);
    assert_eq!(keyframe_positions(&fixed), vec![0, 24, 48]);
}

#[test]
fn every_tune_name_is_taken_by_x264_and_changes_the_encode() {
    if geneva_media::system_x264().is_none() {
        eprintln!("no system x264 here; nothing to check");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.mp4");
    encode_scene_cut(&plain, None, false);
    let plain_bytes = std::fs::read(&plain).unwrap();
    for tune in [
        VideoTune::Film,
        VideoTune::Animation,
        VideoTune::Grain,
        VideoTune::StillImage,
        VideoTune::FastDecode,
        VideoTune::ZeroLatency,
    ] {
        let out = dir.path().join(format!("{}.mp4", tune.as_str()));
        encode_scene_cut(&out, Some(tune), false);
        let info = probe(&out).unwrap();
        assert_eq!(info.video.unwrap().frames, Some(72), "{tune:?}");
        assert_ne!(
            std::fs::read(&out).unwrap(),
            plain_bytes,
            "{tune:?} left the encode unchanged"
        );
    }
}

#[test]
fn a_tune_without_an_equivalent_is_reported_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = solid_settings(VideoCodec::Vp9, None, 192, 108);
    settings.video.as_mut().unwrap().tune = Some(VideoTune::Grain);
    let enc = Encoder::new(&dir.path().join("grain.webm"), settings).unwrap();
    let notes = enc.video_setting_notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].starts_with("tune \"grain\" is not applied"),
        "{notes:?}"
    );
    drop(enc);
    // VP9 has an equivalent for film, so nothing to report.
    let mut settings = solid_settings(VideoCodec::Vp9, None, 192, 108);
    settings.video.as_mut().unwrap().tune = Some(VideoTune::Film);
    let enc = Encoder::new(&dir.path().join("film.webm"), settings).unwrap();
    assert!(enc.video_setting_notes().is_empty());
}

/// The sync corpus (`tests/media/sync`): the tone starts at 1.0 s of
/// every file, even where the audio track itself begins later. Both
/// readers must put it there, whatever the file's timestamps look like.
#[test]
fn corpus_audio_starts_where_it_should_through_both_readers() {
    let corpus = clip().parent().unwrap().join("sync");
    let onset = |samples: &[f32]| {
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        samples
            .iter()
            .position(|s| s.abs() > peak / 3.0)
            .map_or(f64::NAN, |i| i as f64 / 2.0 / 48000.0)
    };
    let mut problems = Vec::new();
    for (name, want) in [
        ("cfr.mp4", 1.0),
        ("bframes-editlist.mp4", 1.0),
        ("bframes-negative-cts.mp4", 1.0),
        ("start-at-10s.mp4", 1.0),
        ("audio-late.mp4", 1.0),
        ("audio-late.mkv", 1.0),
        ("ntsc-2997.mp4", 1.0),
        ("aac-44k.mp4", 1.0),
        ("opus.webm", 1.0),
        ("mpegts.ts", 1.0),
    ] {
        let path = corpus.join(name);
        let mut reader = AudioReader::open(&path).unwrap();
        let whole = reader.read(Ratio::ZERO, Ratio::from_int(3), 48000).unwrap();
        let got = onset(&whole);
        let off = (got - want).abs();
        if off.is_nan() || off > 0.003 {
            problems.push(format!(
                "{name}: read() puts the tone at {got:.4}s, wanted {want}"
            ));
        }
        let mut stream = AudioReader::open(&path)
            .unwrap()
            .into_stream(Ratio::ZERO, 48000)
            .unwrap();
        let mut streamed = Vec::new();
        for _ in 0..30 {
            streamed.extend(stream.read(4800).unwrap());
        }
        let got = onset(&streamed);
        let off = (got - want).abs();
        if off.is_nan() || off > 0.003 {
            problems.push(format!(
                "{name}: into_stream() puts the tone at {got:.4}s, wanted {want}"
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// HDR output settings: PQ or HLG with BT.2020, through the ten-bit
/// software VP9 encoder.
fn hdr_settings(transfer: Transfer) -> EncodeSettings {
    let mut settings = solid_settings(VideoCodec::Vp9, None, 64, 64);
    let v = settings.video.as_mut().unwrap();
    v.color = ResolvedTags {
        primaries: Primaries::Bt2020,
        transfer,
        matrix: Matrix::Bt2020Ncl,
        range: Range::Limited,
    };
    v.crf = Some(4);
    v.hdr_metadata = (transfer == Transfer::Pq).then(geneva_media::HdrMetadata::defaults);
    settings
}

/// A frame of one linear gray, possibly brighter than reference white.
fn gray_frame(level: f32) -> Frame {
    let mut frame = Frame::new(64, 64, Color::BLACK);
    for px in frame.pixels_mut() {
        *px = geneva_color::LinearRgba {
            r: level,
            g: level,
            b: level,
            a: 1.0,
        };
    }
    frame
}

/// The luma code at the center of the frame shown at `t`, read without
/// tone-mapping, from a ten-bit 4:2:0 stream.
fn luma_code_at(path: &Path, t: Ratio) -> u16 {
    let mut reader = VideoReader::open_with(path, ColorTags::default(), false).unwrap();
    let raw = reader.raw_frame_at(t).unwrap();
    assert_eq!(raw.format(), ffmpeg_next::format::Pixel::YUV420P10LE);
    let at = 32 * raw.stride(0) + 32 * 2;
    u16::from_le_bytes([raw.data(0)[at], raw.data(0)[at + 1]])
}

#[test]
fn hdr_output_writes_ten_bit_pq_and_hlg_with_their_tags() {
    let dir = tempfile::tempdir().unwrap();
    // Reference white and a 1000-nit gray, in units of reference white.
    let white = gray_frame(1.0);
    let bright = gray_frame((1000.0 / geneva_color::hdr::REFERENCE_WHITE_NITS) as f32);
    for (transfer, code_white, code_bright) in [
        // Limited-range ten-bit codes: 64 + 876 × the signal.
        (Transfer::Pq, 64.0 + 876.0 * 0.5807, 64.0 + 876.0 * 0.7518),
        (Transfer::Hlg, 64.0 + 876.0 * 0.75, 64.0 + 876.0 * 1.0),
    ] {
        let out = dir.path().join(format!("{transfer:?}.mkv"));
        let mut enc = Encoder::new(&out, hdr_settings(transfer)).unwrap();
        for _ in 0..25 {
            enc.push_frame(&white).unwrap();
        }
        for _ in 0..25 {
            enc.push_frame(&bright).unwrap();
        }
        enc.finish().unwrap();
        let info = probe(&out).unwrap();
        let v = info.video.unwrap();
        assert_eq!(v.color.transfer, Some(transfer), "{transfer:?}");
        assert_eq!(v.color.primaries, Some(Primaries::Bt2020), "{transfer:?}");
        assert_eq!(v.pixel_format, "yuv420p10le", "{transfer:?}");
        let got_white = f64::from(luma_code_at(&out, Ratio::new(12, 25)));
        let got_bright = f64::from(luma_code_at(&out, Ratio::new(37, 25)));
        assert!(
            (got_white - code_white).abs() <= 4.0,
            "{transfer:?} white: code {got_white}, wanted {code_white:.0}"
        );
        assert!(
            (got_bright - code_bright).abs() <= 4.0,
            "{transfer:?} 1000 nits: code {got_bright}, wanted {code_bright:.0}"
        );
        let meta = geneva_media::hdr_metadata_of(&out).unwrap();
        if transfer == Transfer::Pq {
            let meta = meta.expect("PQ output carries static metadata");
            assert_eq!(meta.peak_nits(), Some(1000.0));
        } else {
            assert!(meta.is_none(), "HLG carries no static metadata");
        }
    }
}

#[test]
fn an_hdr_output_refuses_an_eight_bit_codec() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = hdr_settings(Transfer::Pq);
    settings.video.as_mut().unwrap().codec = VideoCodec::H264;
    let err = Encoder::new(&dir.path().join("x.mp4"), settings)
        .err()
        .expect("H.264 cannot carry HDR");
    assert!(err.to_string().contains("ten-bit"), "{err}");
}
