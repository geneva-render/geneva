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
        .stdout(predicate::str::contains(format!(
            "geneva-timeline-{}",
            geneva_timeline::FORMAT_VERSION
        )));
}

#[test]
#[cfg(feature = "media")]
fn crop_takes_the_region_straight_from_the_decoder() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cropped.mp4");
    // The corpus clip is 192×108: keep the middle half of its width.
    let doc = run_json(
        &["convert", "--crop", "24,0,96x108", "-o"],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["mode"], "direct");
    assert!(
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["message"]
                .as_str()
                .unwrap()
                .contains("cropped and scaled")),
        "{doc:#}"
    );
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["width"], 96);
    assert_eq!(info["video"]["height"], 108);
    // A size alone crops the middle; the output takes the crop's size.
    let out = dir.path().join("middle.mp4");
    let doc = run_json(
        &[
            "trim",
            "--from",
            "0.5s",
            "--duration",
            "1s",
            "--crop",
            "96x54",
            "-o",
        ],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["mode"], "direct", "{doc:#}");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["width"], 96);
    assert_eq!(info["video"]["height"], 54);
    // A portrait target fills with the blurred copy by default: the
    // picture whole over a blurred, cover-fitted, silent copy of itself,
    // composited. Bars are what `--fill bars` asks for.
    let bars = dir.path().join("bars.mp4");
    let doc = run_json(
        &["convert", "--for", "tiktok", "--fill", "bars", "-o"],
        &[&bars, &media_dir().join("clip.mp4")],
    );
    assert!(
        !doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["message"]
                .as_str()
                .unwrap()
                .contains("blurred, scaled-up copy")),
        "bars were asked for: {doc:#}"
    );
    let out = dir.path().join("reel.mp4");
    let doc = run_json(
        &["convert", "--for", "tiktok", "-o"],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["mode"], "render", "{doc:#}");
    assert!(
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["message"]
                .as_str()
                .unwrap()
                .contains("blurred, scaled-up copy")),
        "{doc:#}"
    );
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["width"], 192);
    assert_eq!(info["video"]["height"], 342);
    let shown = geneva()
        .args(["convert", "--for", "tiktok", "--show-timeline", "-o"])
        .arg(&out)
        .arg(media_dir().join("clip.mp4"))
        .output()
        .unwrap();
    let tl: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(tl["layers"][0]["id"], "fill");
    assert_eq!(tl["layers"][0]["clips"][0]["fit"], "cover");
    assert_eq!(tl["layers"][0]["clips"][0]["source"]["audio"], false);
    assert_eq!(tl["layers"][0]["clips"][0]["effects"][0]["kind"], "blur");
    assert_eq!(tl["layers"][1]["clips"][0]["fit"], "contain");
    // Speed: twice as fast halves the clip, video and audio alike, and
    // is always composited.
    let out = dir.path().join("fast.mp4");
    let doc = run_json(
        &["trim", "--from", "0s", "--to", "2s", "--speed", "2", "-o"],
        &[&out, &media_dir().join("clip.mp4")],
    );
    assert_eq!(doc["mode"], "render", "{doc:#}");
    assert!(
        (doc["duration"].as_f64().unwrap() - 1.0).abs() < 0.05,
        "{doc:#}"
    );
    let info = run_json(&["probe"], &[&out]);
    assert!(
        (info["duration"].as_f64().unwrap() - 1.0).abs() < 0.1,
        "{info:#}"
    );
    assert!(info["audio"].is_object(), "{info:#}");
    // In a timeline, a crop is a clip field; a 0.1 document without one
    // still validates.
    geneva()
        .args(["validate", "--probe", "--assets"])
        .arg(media_dir())
        .arg(media_dir().join("overlay-demo.json"))
        .assert()
        .success();
}

#[test]
#[cfg(feature = "media")]
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
    assert_eq!(doc["content_type"], "video/mp4");
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
    // A smart cut with the system's x264, a plain re-encode without it.
    assert!(
        doc["mode"] == "smart" || doc["mode"] == "direct",
        "{}",
        doc["mode"]
    );
    assert_eq!(doc["frames"], 23);
}

#[cfg(feature = "media")]
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
#[cfg(feature = "media")]
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
    assert!(
        exact["mode"] == "smart" || exact["mode"] == "direct",
        "{}",
        exact["mode"]
    );
    assert_eq!(exact["frames"], 25);
}

#[test]
#[cfg(feature = "media")]
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
#[cfg(feature = "media")]
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
#[cfg(feature = "media")]
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
    // The video goes straight to the encoder; only the logo is drawn.
    assert_eq!(doc["mode"], "direct");
    assert_eq!(doc["frames"], 50);
}

#[test]
#[cfg(feature = "media")]
fn audio_extracts_mutes_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let wav = dir.path().join("sound.wav");
    let source = run_json(&["probe"], &[&clip])["audio"].clone();
    let doc = run_json(&["audio", "--extract", "-o"], &[&wav, &clip]);
    assert_eq!(doc["mode"], "render");
    assert_eq!(doc["frames"], 0);
    let info = run_json(&["probe"], &[&wav]);
    assert!(info["video"].is_null());
    assert_eq!(info["audio"]["codec"], "pcm_s16le");
    // The sound keeps the source's rate and channel count.
    assert_eq!(info["audio"]["sample_rate"], source["sample_rate"]);
    assert_eq!(info["audio"]["channels"], source["channels"]);

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
    assert_eq!(info["audio"]["channels"], source["channels"]);

    geneva()
        .args(["audio", "-o"])
        .arg(dir.path().join("none.mp4"))
        .arg(&clip)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--extract"));
}

#[test]
#[cfg(feature = "media")]
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
    assert_eq!(overlaid["mode"], "direct");
}

#[test]
#[cfg(feature = "media")]
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
#[cfg(feature = "media")]
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
#[cfg(feature = "media")]
fn subtitles_burn_compiles_cues_to_text_clips_and_checks_the_frame() {
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("en.srt");
    std::fs::write(
        &srt,
        "1\n00:00:00,200 --> 00:00:00,900\nHello <i>there</i>\n\n2\n00:00:00,500 --> 00:00:01,200\nOverlap\n\n3\n00:00:01,300 --> 00:00:01,900\nBye\n",
    )
    .unwrap();
    let clip = media_dir().join("clip.mp4");
    let out = dir.path().join("burned.mp4");

    let shown = run_json(
        &[
            "subtitles",
            "--burn",
            srt.to_str().unwrap(),
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    // The video, then one layer per set of non-overlapping cues.
    assert_eq!(shown["layers"].as_array().unwrap().len(), 3);
    let first = &shown["layers"][1]["clips"][0];
    assert_eq!(first["source"]["kind"], "text");
    assert_eq!(first["source"]["text"], "Hello there");
    assert_eq!(first["start"], "0.2s");
    assert_eq!(first["duration"], "0.7s");
    assert_eq!(first["transform"]["anchor"]["y"], "100%");
    assert_eq!(shown["layers"][1]["clips"][1]["source"]["text"], "Bye");
    assert_eq!(shown["layers"][2]["clips"][0]["source"]["text"], "Overlap");

    // A huge size cannot fit a 192×108 frame: warned before rendering,
    // with the clip's path; the render still goes ahead.
    let doc = run_json(
        &[
            "subtitles",
            "--burn",
            srt.to_str().unwrap(),
            "--style",
            r#"{"size": 80}"#,
            "-o",
        ],
        &[&out, &clip],
    );
    assert_eq!(doc["ok"], true);
    // The video itself skips the compositor; only the cues are drawn, and
    // with the system's x264 the frames without a cue are copied.
    assert!(
        doc["mode"] == "smart" || doc["mode"] == "direct",
        "{}",
        doc["mode"]
    );
    let codes: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"W403"), "{codes:?}");
    let w403 = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "W403")
        .unwrap();
    assert!(
        w403["path"]
            .as_str()
            .unwrap()
            .starts_with("/layers/1/clips/")
    );
    assert!(out.exists());

    // With --fit the same style is shrunk until it fits, and says so.
    let timeline = run_json(
        &[
            "subtitles",
            "--burn",
            srt.to_str().unwrap(),
            "--style",
            r#"{"size": 24}"#,
            "--safe",
            "30",
            "--fit",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    // Size 24 wraps "Hello there" onto two lines on this 192×108 frame,
    // too tall for a 40% safe area, so the fit shrinks it. How far it
    // has to go depends on how wide the machine's default font is, so
    // the only bound here is the fit's own: it steps down by 5% and
    // stops at half.
    let size = timeline["layers"][1]["clips"][0]["source"]["size"]
        .as_f64()
        .unwrap();
    assert!((12.0..24.0).contains(&size), "{size}");
    let mut cmd = geneva();
    cmd.args(["--format", "json", "subtitles", "--burn"])
        .arg(&srt)
        .args(["--style", r#"{"size": 24}"#, "--safe", "30", "--fit", "-o"])
        .arg(&out)
        .arg(&clip);
    let result = cmd.output().unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let codes: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(
        codes.contains(&"N405") && !codes.contains(&"W403"),
        "{codes:?}"
    );
    // A cue may only still be outside the safe area when the fit had
    // nothing left to give: it stops at half the asked-for size, and a
    // wide font reaches that floor on a frame this small. Anything
    // flagged above the floor would be a cue the fit gave up on early.
    // Checking it this way rather than forbidding the note outright is
    // what keeps this test from depending on the machine's fonts: this
    // one has Liberation and shrinks to 13.2 px, a bare container has
    // only DejaVu, which is wider, reaches 12 px and is still outside.
    for d in doc["diagnostics"].as_array().unwrap() {
        if d["code"] != "N404" {
            continue;
        }
        let path = d["path"].as_str().unwrap();
        let at: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let (layer, clip_at) = (
            at[1].parse::<usize>().unwrap(),
            at[3].parse::<usize>().unwrap(),
        );
        let flagged = timeline["layers"][layer]["clips"][clip_at]["source"]["size"]
            .as_f64()
            .unwrap();
        assert_eq!(
            flagged, 12.0,
            "{path} was flagged at {flagged} px, above the floor"
        );
    }

    // At the default size the cues fit; a tiny safe area makes them a note.
    let doc = run_json(
        &[
            "subtitles",
            "--burn",
            srt.to_str().unwrap(),
            "--safe",
            "40",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    assert!(doc["layers"].is_array());
    let mut cmd = geneva();
    cmd.args(["--format", "json", "subtitles", "--burn"])
        .arg(&srt)
        .args(["--safe", "40", "-o"])
        .arg(&out)
        .arg(&clip);
    let result = cmd.output().unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "N404"),
        "{doc}"
    );
    let mut cmd = geneva();
    cmd.args(["subtitles", "--burn"])
        .arg(&srt)
        .args(["--style", "{\"nonsense\": 1}", "-o"])
        .arg(&out)
        .arg(&clip);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("nonsense"));
}

#[test]
#[cfg(feature = "media")]
fn subtitles_burn_reads_a_word_file_and_picks_out_the_word_being_said() {
    let dir = tempfile::tempdir().unwrap();
    let words = dir.path().join("words.json");
    // Whisper's verbose_json, keys we do not use and all.
    std::fs::write(
        &words,
        r#"{"text": " Hello there", "language": "en", "segments": [
             {"id": 0, "seek": 0, "start": 0.2, "end": 1.2, "text": " Hello there", "words": [
               {"word": " Hello", "start": 0.2, "end": 0.7, "probability": 0.98},
               {"word": " there", "start": 0.7, "end": 1.2, "probability": 0.91}]}]}"#,
    )
    .unwrap();
    let clip = media_dir().join("clip.mp4");
    let out = dir.path().join("burned.mp4");

    let shown = run_json(
        &[
            "subtitles",
            "--burn",
            words.to_str().unwrap(),
            "--highlight",
            "#ffd233",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    let src = &shown["layers"][1]["clips"][0]["source"];
    assert_eq!(src["text"], "Hello there");
    assert_eq!(src["highlight"]["color"], "#ffd233");
    // Word times are relative to the clip, which starts where the cue does.
    assert_eq!(src["words"][0]["text"], "Hello");
    assert_eq!(src["words"][0]["start"], "0s");
    assert_eq!(src["words"][1]["start"], "0.5s");
    assert_eq!(src["words"][1]["end"], "1s");
    assert_eq!(shown["layers"][1]["clips"][0]["start"], "0.2s");

    // The same flag on a file that times whole cues says so rather than
    // quietly drawing nothing different.
    let srt = dir.path().join("en.srt");
    std::fs::write(&srt, "1\n00:00:00,200 --> 00:00:01,200\nHello there\n").unwrap();
    let mut cmd = geneva();
    cmd.args(["--format", "json", "subtitles", "--burn"])
        .arg(&srt)
        .args(["--highlight", "yellow", "-o"])
        .arg(&out)
        .arg(&clip);
    let result = cmd.output().unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(
        doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "W453"),
        "{doc}"
    );

    // A file we cannot find words in is refused, not rendered empty.
    let junk = dir.path().join("junk.json");
    std::fs::write(&junk, r#"{"ok": true}"#).unwrap();
    let mut cmd = geneva();
    cmd.args(["subtitles", "--burn"])
        .arg(&junk)
        .arg("-o")
        .arg(&out)
        .arg(&clip);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("no words"));
}

#[test]
#[cfg(feature = "media")]
fn css_shorthands_expand_into_the_object_form() {
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("en.srt");
    std::fs::write(&srt, "1\n00:00:00,200 --> 00:00:00,900\nHello\n").unwrap();
    let out = dir.path().join("burned.mp4");
    let shown = run_json(
        &[
            "subtitles",
            "--burn",
            srt.to_str().unwrap(),
            "--style",
            r#"{"font": "italic 600 14px/1.5 Inter", "shadow": "0 1px 3px #000a", "outline": "1px black", "padding": "4px"}"#,
            "--show-timeline",
            "-o",
        ],
        &[&out, &media_dir().join("clip.mp4")],
    );
    let src = &shown["layers"][1]["clips"][0]["source"];
    assert_eq!(src["font"], "Inter");
    assert_eq!(src["size"], 14.0);
    assert_eq!(src["weight"], 600);
    assert_eq!(src["italic"], true);
    assert_eq!(src["line_height"], 1.5);
    assert_eq!(src["shadow"]["y"], 1.0);
    assert_eq!(src["shadow"]["blur"], 3.0);
    assert_eq!(src["shadow"]["color"], "#000000aa");
    assert_eq!(src["outline"]["width"], 1.0);
    assert_eq!(src["padding"], 4.0);

    // A bad shorthand is an error at the field, with the form expected.
    let mut cmd = geneva();
    cmd.args(["subtitles", "--burn"])
        .arg(&srt)
        .args(["--style", r#"{"shadow": "2px"}"#, "-o"])
        .arg(&out)
        .arg(media_dir().join("clip.mp4"));
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("0 2px 8px #0008"));
}

#[test]
#[cfg(feature = "media")]
fn targets_pick_size_codec_quality_and_caps_from_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let out = dir.path().join("phone.mp4");

    // A device class the small H.264 source already fits: used as it is,
    // it is copied, so the printed timeline carries no encode block.
    let shown = run_json(
        &["convert", "--for", "phone", "--show-timeline", "-o"],
        &[&out, &clip],
    );
    assert!(shown["output"]["encode"].is_null(), "{shown}");
    // A quality ask applies the target: the source is kept at its size,
    // the encode block is filled in and explained.
    let shown = run_json(
        &[
            "convert",
            "--for",
            "phone",
            "--quality",
            "good",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    assert_eq!(shown["output"]["width"], 192);
    let enc = &shown["output"]["encode"];
    assert_eq!(enc["video"]["codec"], "h264");
    assert_eq!(enc["video"]["crf"], 23);
    assert_eq!(enc["video"]["keyframe_interval"], 2.0);
    assert_eq!(enc["video"]["level"], "3.1");
    assert_eq!(enc["video"]["max_bitrate_kbps"], 1500);
    assert_eq!(enc["audio"]["bitrate_kbps"], 128);
    assert_eq!(enc["fast_start"], true);

    // A portrait platform: the landscape source sits on a 9:16 canvas of
    // its own width; the eco tier lowers the quality; explicit CRF wins.
    let shown = run_json(
        &[
            "convert",
            "--for",
            "instagram",
            "--quality",
            "eco",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    assert_eq!(shown["output"]["width"], 192);
    assert_eq!(shown["output"]["height"], 342);
    // The blurred copy is the first layer and the picture the second,
    // which is what a portrait canvas does unless bars are asked for.
    assert_eq!(shown["layers"][0]["id"], "fill");
    assert_eq!(shown["layers"][0]["clips"][0]["fit"], "cover");
    assert_eq!(shown["layers"][1]["clips"][0]["fit"], "contain");
    assert_eq!(shown["output"]["encode"]["video"]["crf"], 26);
    let shown = run_json(
        &[
            "convert",
            "--for",
            "instagram",
            "--crf",
            "19",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    assert_eq!(shown["output"]["encode"]["video"]["crf"], 19);

    // A budget caps the bitrate from the length, and the render reports
    // the choices and checks the written size.
    let doc = run_json(
        &[
            "convert",
            "--for",
            "email",
            "--budget",
            "200KB",
            "--preset",
            "ultrafast",
            "-o",
        ],
        &[&out, &clip],
    );
    assert_eq!(doc["ok"], true);
    let notes: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "N410")
        .map(|d| d["message"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(notes.len(), 1, "{doc}");
    assert!(notes[0].contains("fits 200 KB"), "{}", notes[0]);
    assert!(out.exists());
    // The budget's rate also goes into the block as the average to aim
    // for, which hardware encoders need to hold a size.
    let shown = run_json(
        &[
            "convert",
            "--for",
            "email",
            "--budget",
            "200KB",
            "--show-timeline",
            "-o",
        ],
        &[&out, &clip],
    );
    let video = &shown["output"]["encode"]["video"];
    assert_eq!(video["bitrate_kbps"], video["max_bitrate_kbps"], "{video}");

    // Unknown targets and the table.
    let mut cmd = geneva();
    cmd.args(["convert", "--for", "nowhere", "-o"])
        .arg(&out)
        .arg(&clip);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("geneva targets"));
    let table = run_json(&["targets"], &[]);
    assert!(
        table
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "youtube")
    );
}

#[test]
#[cfg(feature = "media")]
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
#[cfg(feature = "media")]
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

#[test]
#[cfg(feature = "media")]
fn verbs_keep_the_source_color_encoding() {
    let dir = tempfile::tempdir().unwrap();
    // A BT.601-tagged SD clip, as a camera or an old encoder would write.
    let sd = dir.path().join("sd.mp4");
    std::fs::write(
        dir.path().join("sd.json"),
        r##"{ "geneva": "0.1",
            "output": { "width": 320, "height": 240, "fps": 25, "duration": "1s",
                        "color": { "primaries": "bt601-625", "matrix": "bt601" } },
            "layers": [ { "clips": [ { "source": { "kind": "solid", "color": "#4080c0" } } ] } ] }"##,
    )
    .unwrap();
    run_json(
        &["render", "--preset", "ultrafast", "-o"],
        &[&sd, &dir.path().join("sd.json")],
    );
    let info = run_json(&["probe"], &[&sd]);
    assert_eq!(info["video"]["color"]["matrix"], "bt601");

    // Resizing it keeps the encoding, tags the output with it, and so
    // stays on the direct path instead of converting to BT.709.
    let out = dir.path().join("small.mp4");
    let doc = run_json(
        &["resize", "--height", "120", "--preset", "ultrafast", "-o"],
        &[&out, &sd],
    );
    assert_eq!(doc["mode"], "direct");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["color"]["matrix"], "bt601");
    assert_eq!(info["video"]["color"]["primaries"], "bt601-625");
}

#[test]
#[cfg(feature = "media")]
fn exact_cuts_copy_the_untouched_stretches() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    // The cut at 0.6 s falls inside a group of pictures: the frames up to
    // the next keyframe are encoded, the rest copied as coded.
    let out = dir.path().join("smart.mp4");
    let doc = run_json(
        &["trim", "--from", "0.6", "--duration", "1", "--exact", "-o"],
        &[&out, &clip],
    );
    assert_eq!(doc["frames"], 25);
    if doc["mode"] == "direct" {
        // No system x264: nothing to stitch with.
        return;
    }
    assert_eq!(doc["mode"], "smart");
    let text = doc.to_string();
    assert!(text.contains("16 of 25 frames copied"), "{text}");
    let info = run_json(&["probe"], &[&out]);
    assert_eq!(info["video"]["codec"], "h264");
    // The audio is the source's own packets, copied.
    assert_eq!(info["audio"]["codec"], "aac");
    assert!(text.contains("audio copied as coded"), "{text}");

    // An overlay shown for part of the time forces only its frames (up to
    // the next keyframe) through the encoder.
    let logo = dir.path().join("logo.json");
    std::fs::write(
        &logo,
        r##"{ "geneva": "0.1", "output": { "width": 40, "height": 20, "fps": 1, "duration": "1s", "background": "#ff0000" }, "layers": [] }"##,
    )
    .unwrap();
    let png = dir.path().join("logo.png");
    geneva()
        .args(["frame", "-o"])
        .arg(&png)
        .arg(&logo)
        .assert()
        .success();
    let out = dir.path().join("overlaid.mp4");
    let doc = run_json(
        &["overlay", "--start", "0.5s", "--duration", "0.4s", "-o"],
        &[&out, &clip, &png],
    );
    assert_eq!(doc["mode"], "smart");
    assert_eq!(doc["frames"], 50);
    assert!(doc.to_string().contains("39 of 50 frames copied"), "{doc}");
}

/// A phone recording reports an average frame rate (frames over the
/// file's duration) that no frame has, since its timestamps jitter; the
/// copy planner must read it at the same rate the probe does, or every
/// trim of it re-encodes.
#[cfg(feature = "media")]
#[test]
fn trim_copies_a_file_whose_timestamps_jitter() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("sync").join("jitter-2997.mov");
    let out = dir.path().join("cut.mp4");
    let doc = run_json(
        &["trim", "--from", "1", "--duration", "1", "-o"],
        &[&out, &clip],
    );
    assert_eq!(doc["mode"], "copy", "{doc:#}");
    let info = run_json(&["probe"], &[&out]);
    let fps = info["video"]["fps"].as_f64().unwrap();
    assert!((fps - 30000.0 / 1001.0).abs() < 1e-3, "{info:#}");
}

/// When the streams could have been copied but the output cannot take
/// them, the render says why.
#[cfg(feature = "media")]
#[test]
fn a_refused_copy_says_why_in_the_notes() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let out = dir.path().join("cut.webm");
    let doc = run_json(
        &["trim", "--from", "0", "--duration", "0.5", "-o"],
        &[&out, &clip],
    );
    assert_ne!(doc["mode"], "copy");
    let notes: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["message"].as_str())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("not copied without re-encoding: clip.mp4 is h264")),
        "{notes:#?}"
    );
}

/// A phone's portrait clip is a landscape stream with a rotation the
/// player applies. The probe reports the picture as displayed, a copy
/// keeps the rotation, and a re-encode writes the picture upright.
#[cfg(feature = "media")]
#[test]
fn a_rotated_clip_is_shown_probed_copied_and_rendered_upright() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("sync").join("rotated-90.mp4");
    let info = run_json(&["probe"], &[&clip]);
    assert_eq!(info["video"]["width"], 90, "{info:#}");
    assert_eq!(info["video"]["height"], 160, "{info:#}");
    assert_eq!(info["video"]["rotation"], 90, "{info:#}");

    // Copied: the stream keeps its matrix, so a player still shows it upright.
    let copied = dir.path().join("copied.mp4");
    let doc = run_json(&["convert", "-o"], &[&copied, &clip]);
    assert_eq!(doc["mode"], "copy", "{doc:#}");
    let info = run_json(&["probe"], &[&copied]);
    assert_eq!(info["video"]["rotation"], 90, "{info:#}");
    assert_eq!(info["video"]["width"], 90, "{info:#}");

    // Re-encoded: the picture is written upright, with no rotation to apply.
    let upright = dir.path().join("upright.mp4");
    let doc = run_json(&["resize", "--height", "320", "-o"], &[&upright, &clip]);
    assert_ne!(doc["mode"], "copy", "{doc:#}");
    let info = run_json(&["probe"], &[&upright]);
    assert_eq!(info["video"]["rotation"], 0, "{info:#}");
    assert_eq!(info["video"]["width"], 180, "{info:#}");
    assert_eq!(info["video"]["height"], 320, "{info:#}");
}

#[test]
#[cfg(feature = "media")]
fn render_writes_every_output_of_a_document_in_one_pass() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("many.json");
    std::fs::write(
        &doc,
        r##"{
          "geneva": "0.3",
          "output": { "width": 160, "height": 90, "fps": 24,
            "color": { "primaries": "bt601-625", "transfer": "bt709", "matrix": "bt601", "range": "limited" } },
          "assets": { "clip": { "src": "sync/cfr.mp4" } },
          "layers": [ { "clips": [ { "source": { "kind": "video", "asset": "clip" } } ] } ],
          "outputs": {
            "full": { "kind": "video" },
            "small": { "kind": "video", "width": 80, "encode": { "video": { "preset": "ultrafast" } } },
            "poster": { "kind": "poster", "at": "1s", "path": "cover.png" },
            "seek": { "kind": "sprites", "every": "1s", "columns": 2 },
            "sound": { "kind": "audio", "audio": { "sample_rate": 16000, "channels": 1 } }
          }
        }"##,
    )
    .unwrap();
    let out = dir.path().join("out");
    let report = run_json(
        &["render"],
        &[
            doc.as_path(),
            std::path::Path::new("--assets"),
            media_dir().as_path(),
            std::path::Path::new("-o"),
            out.as_path(),
        ],
    );
    assert_eq!(report["ok"], true);
    let outputs = report["outputs"].as_array().unwrap();
    let mode = |name: &str| {
        let entry = outputs
            .iter()
            .find(|o| o["name"] == name)
            .unwrap_or_else(|| panic!("no output {name}: {report}"));
        entry["mode"].as_str().unwrap().to_owned()
    };
    // The canvas-size rendition with no encode asks is a stream copy;
    // the smaller one is encoded.
    assert_eq!(mode("full"), "copy");
    assert_eq!(mode("small"), "render");
    for file in [
        "full.mp4",
        "small.mp4",
        "cover.png",
        "seek.jpg",
        "seek.vtt",
        "sound.wav",
    ] {
        assert!(out.join(file).is_file(), "{file} missing");
    }
    let vtt = std::fs::read_to_string(out.join("seek.vtt")).unwrap();
    assert!(vtt.starts_with("WEBVTT"));
    assert!(
        vtt.contains("seek.jpg#xywh=80,0,160,90") || vtt.contains("seek.jpg#xywh=160,0,160,90"),
        "{vtt}"
    );
    let small = run_json(&["probe"], &[out.join("small.mp4").as_path()]);
    assert_eq!(small["video"]["width"], 80);
    assert_eq!(small["video"]["frames"], 72);
    let sound = run_json(&["probe"], &[out.join("sound.wav").as_path()]);
    assert_eq!(sound["audio"]["sample_rate"], 16000);
    assert_eq!(sound["audio"]["channels"], 1);
    let cover = image::open(out.join("cover.png")).unwrap();
    assert_eq!((cover.width(), cover.height()), (160, 90));
}

#[test]
#[cfg(feature = "media")]
fn frame_takes_a_still_from_a_video_file() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("sync/cfr.mp4");
    // An explicit time decodes one frame; the picture takes the video's size.
    let still = dir.path().join("still.jpg");
    let report = run_json(
        &["frame", "--at", "0.5s"],
        &[clip.as_path(), std::path::Path::new("-o"), still.as_path()],
    );
    assert_eq!(report["frames"], 1, "{report}");
    let row = &report["outputs"][0];
    assert_eq!(row["content_type"], "image/jpeg");
    assert_eq!(
        (row["width"].as_u64(), row["height"].as_u64()),
        (Some(160), Some(90))
    );
    assert!(still.is_file());
    // Without a time the frame is chosen; a width keeps the aspect.
    let chosen = dir.path().join("chosen.png");
    let report = run_json(
        &["frame", "--width", "80"],
        &[clip.as_path(), std::path::Path::new("-o"), chosen.as_path()],
    );
    assert!(report["frames"].as_u64().unwrap() >= 1, "{report}");
    let img = image::open(&chosen).unwrap();
    assert_eq!((img.width(), img.height()), (80, 45));
    // A timeline still goes through the renderer, and takes a size too.
    let tl = dir.path().join("tl.jpg");
    let report = run_json(
        &["frame", "--at", "1s", "--height", "90"],
        &[
            examples().join("shapes.json").as_path(),
            std::path::Path::new("-o"),
            tl.as_path(),
        ],
    );
    assert_eq!(report["content_type"], "image/jpeg");
    assert_eq!(report["width"], 160);
    assert_eq!(report["height"], 90);
}

#[test]
#[cfg(feature = "media")]
fn for_target_copies_a_source_that_already_fits() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("sync/cfr.mp4");
    // A small H.264/AAC MP4 is within the web target: copied, with a note.
    let out = dir.path().join("web.mp4");
    let report = run_json(
        &["convert", "--for", "web"],
        &[clip.as_path(), std::path::Path::new("-o"), out.as_path()],
    );
    assert_eq!(report["mode"], "copy", "{report}");
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["message"].as_str().unwrap().contains("already fits web")),
        "{report}"
    );
    // A quality ask re-encodes regardless.
    let out = dir.path().join("web-crf.mp4");
    let report = run_json(
        &["convert", "--for", "web", "--crf", "30"],
        &[clip.as_path(), std::path::Path::new("-o"), out.as_path()],
    );
    assert_ne!(report["mode"], "copy", "{report}");
    // A target the source does not fit (portrait) is encoded onto its canvas.
    let out = dir.path().join("tiktok.mp4");
    let report = run_json(
        &["convert", "--for", "tiktok"],
        &[clip.as_path(), std::path::Path::new("-o"), out.as_path()],
    );
    assert_ne!(report["mode"], "copy", "{report}");
}

#[test]
#[cfg(feature = "media")]
fn a_render_reports_progress_in_the_format_it_was_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let demo = media_dir().join("overlay-demo.json");

    // JSON mode: one object per line on stderr, stdout still one report.
    let out = dir.path().join("demo.mp4");
    let result = geneva()
        .args(["--format", "json", "render"])
        .arg(&demo)
        .args(["-o"])
        .arg(&out)
        .args(["--preset", "ultrafast"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
    let lines: Vec<serde_json::Value> = stderr
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| serde_json::from_str(l).expect("a progress line is one JSON object"))
        .collect();
    assert!(!lines.is_empty(), "no progress lines in {stderr:?}");
    for line in &lines {
        assert_eq!(line["event"], "progress");
        assert_eq!(line["total"], 38);
        assert_eq!(line["duration"], 1.52);
    }
    let last = lines.last().unwrap();
    assert_eq!(last["frames"], 38);
    assert_eq!(last["time"], 1.52);
    assert!(last["remaining"].is_null(), "{last}");
    let doc: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(doc["ok"], true);

    // Human mode: the line a person reads, rewritten in place.
    let out = dir.path().join("human.mp4");
    geneva()
        .args(["render"])
        .arg(&demo)
        .args(["-o"])
        .arg(&out)
        .args(["--preset", "ultrafast"])
        .assert()
        .success()
        .stderr(predicate::str::contains("\rframe 38/38"));

    // A copy renders no frames, so it reports none.
    let copied = dir.path().join("cut.mp4");
    let result = geneva()
        .args(["--format", "json", "trim", "--to", "1s", "-o"])
        .arg(&copied)
        .arg(media_dir().join("clip.mp4"))
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8_lossy(&result.stderr)
            .lines()
            .filter(|l| l.starts_with('{'))
            .count(),
        0
    );
}

/// A document asking for what this binary was not built with is refused
/// at the field, before anything is rendered. With the feature on, the
/// same document is accepted.
#[test]
fn denoise_is_refused_at_the_field_when_it_is_not_built_in() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("denoise.json");
    std::fs::write(
        &path,
        r##"{"geneva":"0.3","output":{"width":320,"height":240,"fps":25,
            "audio":{"denoise":true}},
            "layers":[{"clips":[{"start":"0s","duration":"1s",
            "source":{"kind":"solid","color":"#000000"}}]}]}"##,
    )
    .unwrap();
    let assert = geneva().args(["validate"]).arg(&path).assert();
    if cfg!(feature = "denoise") {
        assert.success();
    } else {
        assert
            .code(1)
            .stderr(predicate::str::contains("E424"))
            .stderr(predicate::str::contains("/output/audio/denoise"));
    }
}

/// A target that only has the sound to change keeps the picture: the
/// video packets are copied and the mix is encoded beside them, which
/// is what `--for podcast` on a long recording depends on. No frame
/// reaching an encoder is what makes it cheap.
#[cfg(feature = "media")]
#[test]
fn a_podcast_target_copies_the_picture_and_encodes_only_the_sound() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("pod.mp4");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/media/clip.mp4");
    let result = geneva()
        .args(["--format", "json", "convert"])
        .arg(&source)
        .arg("-o")
        .arg(&out)
        .args(["--for", "podcast"])
        .assert()
        .success();
    let out_text = String::from_utf8_lossy(&result.get_output().stdout).into_owned();
    assert!(
        out_text.contains("\"mode\": \"copy-picture\""),
        "the picture was not copied: {out_text}"
    );
    assert!(
        out_text.contains("\"frames\": 0"),
        "frames reached an encoder: {out_text}"
    );
}

/// A verb can choose the renderer, not only `render`.
///
/// The flag was on `render` alone, so a job like `convert --for tiktok`,
/// which builds a canvas and composites every frame, had no way to ask
/// for the device. This checks the choice reaches the renderer rather
/// than merely parsing: `gpu` always says something about a device,
/// either that it composited on one or that none could be used, and
/// `cpu` says neither. That holds on a machine with a GPU, on one with
/// only a software Vulkan device, and on one with nothing at all.
#[test]
#[cfg(feature = "media")]
fn a_verb_can_ask_for_the_gpu() {
    let dir = tempfile::tempdir().unwrap();
    let clip = media_dir().join("clip.mp4");
    let about_a_device = |renderer: &str| -> bool {
        let out = dir.path().join(format!("{renderer}.mp4"));
        let result = geneva()
            .args(["convert", "--for", "tiktok", "--renderer", renderer, "-o"])
            .arg(&out)
            .arg(&clip)
            .output()
            .unwrap();
        assert!(result.status.success(), "{renderer}: {result:?}");
        assert!(out.exists(), "{renderer} wrote nothing");
        let said = String::from_utf8_lossy(&result.stderr).to_lowercase();
        said.contains("composited on the device") || said.contains("no usable gpu")
    };
    assert!(
        about_a_device("gpu"),
        "asking for the gpu said nothing about one"
    );
    assert!(
        !about_a_device("cpu"),
        "asking for the cpu mentioned a device"
    );
}

/// Every `.rs` file under a crate's `src`, so a test can read what the
/// engine itself can emit rather than what a list says it emits.
fn crate_sources() -> Vec<PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap().flatten() {
        walk(&entry.path().join("src"), &mut out);
    }
    out.sort();
    assert!(out.len() > 20, "found {} source files", out.len());
    out
}

#[test]
fn every_code_the_engine_emits_is_documented() {
    // The codes as they are written in the source: a letter, three
    // digits, in quotes. Tests are left out, since one of them names a
    // retired code to assert it is not emitted.
    let mut codes: Vec<String> = Vec::new();
    for path in crate_sources() {
        let text = std::fs::read_to_string(&path).unwrap();
        for (i, _) in text.match_indices('"') {
            let rest = &text.as_bytes()[i + 1..];
            if rest.len() < 5 || rest[4] != b'"' {
                continue;
            }
            let (letter, digits) = (rest[0], &rest[1..4]);
            if matches!(letter, b'E' | b'W' | b'N') && digits.iter().all(u8::is_ascii_digit) {
                codes.push(String::from_utf8(rest[..4].to_vec()).unwrap());
            }
        }
    }
    codes.sort();
    codes.dedup();
    assert!(codes.len() > 50, "found {} codes: {codes:?}", codes.len());

    let documented = geneva()
        .args(["--format", "json", "explain", "--list"])
        .assert()
        .success();
    let rows: serde_json::Value = serde_json::from_slice(&documented.get_output().stdout).unwrap();
    let rows = rows.as_array().unwrap();
    let missing: Vec<&String> = codes
        .iter()
        .filter(|c| !rows.iter().any(|r| r["code"] == ***c))
        .collect();
    assert!(missing.is_empty(), "not in docs/errors.md: {missing:?}");
}

#[test]
fn the_guide_prints_the_agent_page_by_default() {
    geneva()
        .args(["guide"])
        .assert()
        .success()
        .stdout(predicate::str::contains("# Geneva for programs and agents"));
}

#[test]
fn every_guide_topic_prints_something() {
    let listed = geneva()
        .args(["--format", "json", "guide", "--list"])
        .assert()
        .success();
    let topics: serde_json::Value = serde_json::from_slice(&listed.get_output().stdout).unwrap();
    let topics = topics.as_array().unwrap();
    assert!(topics.len() >= 5, "{topics:?}");
    for t in topics {
        let name = t["topic"].as_str().unwrap();
        let page = geneva().args(["guide", name]).assert().success();
        let text = String::from_utf8(page.get_output().stdout.clone()).unwrap();
        assert!(
            text.starts_with("# "),
            "{name} does not start with a heading"
        );
        assert!(text.len() > 2000, "{name} is {} bytes", text.len());
    }
}

#[test]
fn an_unknown_topic_or_code_is_a_usage_error() {
    geneva().args(["guide", "nonsense"]).assert().code(2);
    geneva().args(["explain", "E999"]).assert().code(2);
}

#[test]
fn explain_answers_a_code_in_either_case_and_in_json() {
    geneva()
        .args(["explain", "e302"])
        .assert()
        .success()
        .stdout(predicate::str::contains("error[E302]"));
    let out = geneva()
        .args(["--format", "json", "explain", "W405"])
        .assert()
        .success();
    let rows: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(rows[0]["code"], "W405");
    assert_eq!(rows[0]["severity"], "warning");
}

#[test]
fn a_diagnostic_from_a_real_run_can_be_explained() {
    // The loop the guide describes: run, take a code from the JSON
    // report, ask what it means.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(
        &path,
        r#"{"geneva":"0.1","output":{"width":640,"height":360,"fps":30,"duration":"2s"},
            "layers":[{"clips":[{"source":{"kind":"image","asset":"missing"}}]}]}"#,
    )
    .unwrap();
    let run = geneva()
        .args(["--format", "json", "validate"])
        .arg(&path)
        .assert()
        .code(1);
    let doc: serde_json::Value = serde_json::from_slice(&run.get_output().stdout).unwrap();
    let code = doc["diagnostics"][0]["code"].as_str().unwrap();
    geneva()
        .args(["explain", code])
        .assert()
        .success()
        .stdout(predicate::str::contains(code));
}
