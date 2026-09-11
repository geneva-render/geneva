//! End-to-end tests of the `geneva` binary.

use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn geneva() -> Command {
    Command::cargo_bin("geneva").unwrap()
}

#[test]
fn validate_accepts_examples() {
    geneva()
        .args(["validate"])
        .arg(examples().join("overlay.json"))
        .assert()
        .success()
        .stderr(predicate::str::contains("ok"));
}

#[test]
fn validate_reports_errors_with_paths_and_exit_code_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(
        &path,
        r#"{"geneva":"0.1","output":{"width":640,"height":360,"fps":30,"duration":"2s"},
            "layers":[{"clips":[{"source":{"kind":"image","asset":"missing"}}]}]}"#,
    )
    .unwrap();
    geneva()
        .args(["validate"])
        .arg(&path)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("error[E200]"))
        .stderr(predicate::str::contains("/layers/0/clips/0/source/asset"));
}

#[test]
fn validate_json_output_is_machine_readable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(
        &path,
        r#"{"geneva":"0.1","output":{"width":641,"height":360,"fps":30,"duration":1}}"#,
    )
    .unwrap();
    let out = geneva()
        .args(["--format", "json", "validate"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["diagnostics"][0]["code"], "W401");
    assert_eq!(doc["summary"]["warnings"], 1);
}

#[test]
fn frame_renders_a_png() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f.png");
    geneva()
        .args(["frame"])
        .arg(examples().join("shapes.json"))
        .args(["--at", "1s", "-o"])
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("frame 30 at 1s"));
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn frame_by_number_uses_the_output_rate() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f.png");
    let result = geneva()
        .args(["--format", "json", "frame"])
        .arg(examples().join("solid.json"))
        .args(["--frame", "45", "-o"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(result.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(doc["time"], "1.5");
    assert_eq!(doc["frame"], 45);
}

#[test]
fn missing_media_assets_are_reported_before_rendering() {
    let dir = tempfile::tempdir().unwrap();
    geneva()
        .args(["frame"])
        .arg(examples().join("overlay.json"))
        .args(["--at", "5s", "-o"])
        .arg(dir.path().join("f.png"))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("error[E501]"))
        .stderr(predicate::str::contains("/assets/main/src"));
}

#[test]
fn schema_prints_json_schema() {
    geneva()
        .arg("schema")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"$schema\""))
        .stdout(predicate::str::contains("geneva-timeline-0.1"));
}

#[test]
fn missing_file_is_a_usage_error() {
    geneva()
        .args(["validate", "/nonexistent/timeline.json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("reading"));
}

#[cfg(feature = "media")]
fn media_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/media")
}

#[test]
#[cfg(feature = "media")]
fn probe_describes_a_file_in_both_formats() {
    geneva()
        .args(["probe"])
        .arg(media_dir().join("clip.mp4"))
        .assert()
        .success()
        .stdout(predicate::str::contains("h264 192×108 @ 25 fps"))
        .stdout(predicate::str::contains("aac 48000 Hz"));
    let out = geneva()
        .args(["--format", "json", "probe"])
        .arg(media_dir().join("clip.mp4"))
        .output()
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["video"]["width"], 192);
    assert_eq!(doc["video"]["color"]["matrix"], "bt709");
}

#[test]
#[cfg(feature = "media")]
fn render_writes_a_playable_file_with_audio() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("demo.mp4");
    let result = geneva()
        .args(["--format", "json", "render"])
        .arg(media_dir().join("overlay-demo.json"))
        .args(["-o"])
        .arg(&out)
        .args(["--preset", "ultrafast"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(doc["frames"], 38);
    let probe = geneva()
        .args(["--format", "json", "probe"])
        .arg(&out)
        .output()
        .unwrap();
    let info: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    assert_eq!(info["video"]["frames"], 38);
    assert_eq!(info["video"]["fps"], 25.0);
    assert_eq!(info["video"]["color"]["primaries"], "bt709");
    assert_eq!(info["audio"]["channels"], 2);
}

#[test]
#[cfg(feature = "media")]
fn validate_with_probe_reports_missing_files() {
    geneva()
        .args(["validate", "--probe"])
        .arg(examples().join("overlay.json"))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("error[E501]"));
}

#[test]
#[cfg(feature = "media")]
fn render_copies_plain_cuts_and_reencodes_with_exact() {
    let dir = tempfile::tempdir().unwrap();
    let timeline = dir.path().join("cut.json");
    std::fs::write(
        &timeline,
        r#"{"geneva":"0.1","output":{"width":192,"height":108,"fps":25},
            "assets":{"clip":{"src":"clip.mp4"}},
            "layers":[{"clips":[{"source":{"kind":"video","asset":"clip","in":"0.6s","out":"1.5s"}}]}]}"#,
    )
    .unwrap();
    let out = dir.path().join("cut.mp4");
    let result = geneva()
        .args(["--format", "json", "render"])
        .arg(&timeline)
        .args(["--assets"])
        .arg(media_dir())
        .args(["-o"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(doc["mode"], "copy");
    let notes: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "N600")
        .map(|d| d["message"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        notes.iter().any(|n| n.contains("keyframe at 0.48s")),
        "{notes:?}"
    );

    let exact = geneva()
        .args(["--format", "json", "render"])
        .arg(&timeline)
        .args(["--assets"])
        .arg(media_dir())
        .args(["-o"])
        .arg(dir.path().join("exact.mp4"))
        .args(["--exact"])
        .output()
        .unwrap();
    assert!(exact.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&exact.stdout).unwrap();
    assert_eq!(doc["mode"], "direct");
    assert_eq!(doc["frames"], 23);
}

fn run_json(args: &[&str], extra: &[&std::path::Path]) -> serde_json::Value {
    let mut cmd = geneva();
    cmd.args(["--format", "json"]).args(args);
    for p in extra {
        cmd.arg(p);
    }
    let result = cmd.output().unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
fn trim_prints_its_timeline_and_copies_the_streams() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let out = dir.path().join("cut.mp4");

    let shown = run_json(
        &[
            "trim",
            "--from",
            "0.5",
            "--duration",
            "1",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    assert_eq!(shown["output"]["width"], 192);
    assert_eq!(shown["layers"][0]["clips"][0]["source"]["in"], "0.5s");
    assert_eq!(shown["layers"][0]["clips"][0]["source"]["out"], "1.5s");
    assert!(!out.exists());

    let doc = run_json(
        &["trim", "--from", "0.5", "--duration", "1", "-o"],
        &[&out, &clip],
    );
    assert_eq!(doc["mode"], "copy");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["width"], 192);
    let duration = info["duration"].as_f64().unwrap();
    assert!((1.0..1.2).contains(&duration), "{duration}");

    let exact = run_json(
        &["trim", "--from", "0.5", "--duration", "1", "--exact", "-o"],
        &[&dir.path().join("exact.mp4"), &clip],
    );
    assert_eq!(exact["mode"], "direct");
    assert_eq!(exact["frames"], 25);
}

#[test]
fn resize_keeps_the_aspect_ratio_and_scales_directly() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("small.mp4");
    let doc = run_json(
        &["resize", "--width", "96", "--preset", "ultrafast", "-o"],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["mode"], "direct");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["width"], 96);
    assert_eq!(info["video"]["height"], 54);
}

#[test]
fn concat_copies_matching_sources_and_renders_crossfades() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let joined = dir.path().join("joined.mp4");
    let doc = run_json(&["concat", "-o"], &[&joined, &clip, &clip]);
    assert_eq!(doc["mode"], "copy");
    let info = run_json(&["probe"], &[&joined]);
    let duration = info["duration"].as_f64().unwrap();
    assert!((4.0..4.1).contains(&duration), "{duration}");

    let faded = dir.path().join("faded.mp4");
    let doc = run_json(
        &[
            "concat",
            "--crossfade",
            "0.5",
            "--preset",
            "ultrafast",
            "-o",
        ],
        &[&faded, &clip, &clip],
    );
    assert_eq!(doc["mode"], "render");
    assert_eq!(doc["duration"], 3.5);
}

#[test]
fn overlay_places_an_image_for_the_length_of_the_video() {
    let dir = tempfile::tempdir().unwrap();
    let logo = dir.path().join("logo.png");
    geneva()
        .args(["frame"])
        .arg(examples().join("solid.json"))
        .args(["-o"])
        .arg(&logo)
        .assert()
        .success();
    let out = dir.path().join("branded.mp4");
    let shown = run_json(
        &[
            "overlay",
            "--at",
            "bottom-left",
            "--scale",
            "0.1",
            "--opacity",
            "0.8",
            "--show-timeline",
            "-o",
        ],
        &[&out, &media_dir().join("clip.mp4"), &logo],
    );
    assert_eq!(shown["output"]["duration"], "2s");
    assert_eq!(shown["layers"][1]["clips"][0]["opacity"], 0.8);
    let doc = run_json(
        &["overlay", "--scale", "0.1", "--preset", "ultrafast", "-o"],
        &[&out, &media_dir().join("clip.mp4"), &logo],
    );
    assert_eq!(doc["mode"], "render");
    assert_eq!(doc["frames"], 50);
}

#[test]
fn audio_extracts_mutes_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let wav = dir.path().join("sound.wav");
    let doc = run_json(&["audio", "--extract", "-o"], &[&wav, &clip]);
    assert_eq!(doc["mode"], "render");
    assert_eq!(doc["frames"], 0);
    let info = run_json(&["probe"], &[&wav]);
    assert!(info["video"].is_null());
    assert_eq!(info["audio"]["codec"], "pcm_s16le");

    let m4a = dir.path().join("sound.m4a");
    let doc = run_json(&["audio", "--extract", "-o"], &[&m4a, &clip]);
    assert_eq!(doc["mode"], "copy");
    assert_eq!(run_json(&["probe"], &[&m4a])["audio"]["codec"], "aac");

    let muted = dir.path().join("muted.mp4");
    let doc = run_json(&["audio", "--mute", "-o"], &[&muted, &clip]);
    assert_eq!(doc["mode"], "copy");
    assert!(run_json(&["probe"], &[&muted])["audio"].is_null());

    let replaced = dir.path().join("replaced.mp4");
    let doc = run_json(
        &[
            "audio",
            "--replace",
            wav.to_str().unwrap(),
            "--preset",
            "ultrafast",
            "-o",
        ],
        &[&replaced, &clip],
    );
    assert_eq!(doc["frames"], 50);
    let info = run_json(&["probe"], &[&replaced]);
    assert_eq!(info["audio"]["channels"], 2);

    geneva()
        .args(["audio", "-o"])
        .arg(dir.path().join("none.mp4"))
        .arg(&clip)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--extract"));
}

#[test]
fn convert_hands_decoded_frames_straight_to_the_encoder() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let out = dir.path().join("out.webm");
    let doc = run_json(&["convert", "--preset", "ultrafast", "-o"], &[&out, &clip]);
    assert_eq!(doc["mode"], "direct");
    assert_eq!(doc["frames"], 50);
    let notes: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("handed to the encoder as decoded")),
        "{notes:?}"
    );
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["codec"], "vp9");

    // A picture that changes size is scaled on the same path, not composited.
    let resized = run_json(
        &["resize", "--width", "96", "--preset", "ultrafast", "-o"],
        &[&dir.path().join("small.mp4"), &clip],
    );
    assert_eq!(resized["mode"], "direct");
    let overlaid = run_json(
        &["overlay", "--at", "center", "--preset", "ultrafast", "-o"],
        &[
            &dir.path().join("over.mp4"),
            &clip,
            &media_dir().join("clip.mp4"),
        ],
    );
    assert_eq!(overlaid["mode"], "render");
}

#[test]
fn image_sequences_write_one_file_per_frame() {
    let dir = tempfile::tempdir().unwrap();
    let pattern = dir.path().join("frame-%03d.png");
    let doc = run_json(
        &["convert", "-o"],
        &[&pattern, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["frames"], 50);
    let files: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 50, "{files:?}");
    assert!(files.contains(&"frame-001.png".to_owned()));
    assert!(!files.iter().any(|f| f.contains('%')));
    let png = std::fs::read(dir.path().join("frame-001.png")).unwrap();
    assert_eq!(&png[..4], b"\x89PNG");

    geneva()
        .args(["convert", "-o"])
        .arg(dir.path().join("single.png"))
        .arg(media_dir().join("clip.mp4"))
        .assert()
        .code(3)
        .stderr(predicate::str::contains("%04d"));
}

#[test]
fn subtitles_attach_as_streams_and_extract_again() {
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("en.srt");
    std::fs::write(
        &srt,
        "1\n00:00:00,200 --> 00:00:00,900\nHello\n\n2\n00:00:01,000 --> 00:00:01,800\nWorld\n",
    )
    .unwrap();
    let out = dir.path().join("subs.mkv");
    let doc = run_json(
        &[
            "subtitles",
            "--add",
            srt.to_str().unwrap(),
            "--language",
            "en",
            "-o",
        ],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["mode"], "copy");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["subtitles"][0]["codec"], "subrip");
    assert_eq!(info["subtitles"][0]["language"], "eng");

    let vtt = dir.path().join("back.vtt");
    let doc = run_json(&["subtitles", "--extract", "-o"], &[&vtt, &out]);
    assert_eq!(doc["cues"], 2);
    let text = std::fs::read_to_string(&vtt).unwrap();
    assert!(text.starts_with("WEBVTT"));
    assert!(text.contains("00:00:01.000 --> 00:00:01.800\nWorld"));
}

#[test]
fn mxf_defaults_to_dnxhr_and_pcm() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("delivery.mxf");
    let doc = run_json(
        &["resize", "--width", "256", "-o"],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["mode"], "direct");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["container"], "mxf");
    assert_eq!(info["video"]["codec"], "dnxhd");
    assert_eq!(info["video"]["pixel_format"], "yuv422p");
    assert_eq!(info["audio"]["codec"], "pcm_s24le");
}

#[test]
fn a_container_that_rejects_the_source_audio_is_rendered_not_copied() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("same-size.mxf");
    // The H.264 picture could be copied into MXF as it is, but MXF takes
    // PCM audio only, so the AAC track rules the copy out.
    let doc = run_json(
        &["convert", "--codec", "h264", "-o"],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_ne!(doc["mode"], "copy");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["codec"], "h264");
    assert_eq!(info["audio"]["codec"], "pcm_s24le");
}
