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
fn frame_of_unsupported_source_exits_three_with_e500() {
    let dir = tempfile::tempdir().unwrap();
    geneva()
        .args(["frame"])
        .arg(examples().join("overlay.json"))
        .args(["--at", "5s", "-o"])
        .arg(dir.path().join("f.png"))
        .assert()
        .code(3)
        .stderr(predicate::str::contains("error[E500]"));
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
