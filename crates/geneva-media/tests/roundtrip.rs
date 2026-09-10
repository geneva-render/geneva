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
        }),
        container: None,
        audio: Some(AudioSettings {
            codec: AudioCodec::Aac,
            bitrate_kbps: 96,
            sample_rate: 48000,
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
        }),
        container: None,
        audio: Some(AudioSettings {
            codec: AudioCodec::Aac,
            bitrate_kbps: 96,
            sample_rate: 48000,
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
    let report = stream_copy(&plan, &out).unwrap();
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
    let report = stream_copy(&plan, &out).unwrap();
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
        audio: Some(AudioSettings {
            codec: AudioCodec::Pcm,
            bitrate_kbps: 0,
            sample_rate: 48000,
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
