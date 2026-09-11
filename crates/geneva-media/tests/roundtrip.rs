//! Probe, decode, encode and mix against the committed test clip.

#![cfg(feature = "media")]

use std::path::{Path, PathBuf};

use geneva_color::{Color, ColorTags, Matrix, Primaries, Range, ResolvedTags, Transfer};
use geneva_media::{
    AudioReader, AudioSettings, EncodeSettings, Encoder, VideoReader, VideoSettings, mix, probe,
};
use geneva_render::Frame;
use geneva_timeline::schema::{AudioCodec, HardwarePolicy, VideoCodec};
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
            level: None,
        }),
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
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
            level: None,
        }),
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
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
            level: None,
        }),
        container: None,
        subtitles: Vec::new(),
        fast_start: true,
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
