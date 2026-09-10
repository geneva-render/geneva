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
    assert_eq!(doc["mode"], "render");
    assert_eq!(doc["frames"], 23);
}
