//! Renders every case under `tests/golden/` and compares with references.

use std::path::Path;

use geneva_golden::{discover_cases, run_case};

#[test]
fn golden_scenes_match_their_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let failures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/golden-failures");
    let cases = discover_cases(&root);
    assert!(
        !cases.is_empty(),
        "no golden cases under {}",
        root.display()
    );
    let mut failed = Vec::new();
    for case in cases {
        match run_case(&case, &failures) {
            Ok(outcome) => {
                for f in &outcome.frames {
                    let summary = f
                        .comparison
                        .as_ref()
                        .map_or("written".to_owned(), geneva_golden::Comparison::summary);
                    eprintln!(
                        "{} @ {}: {} ({summary})",
                        outcome.name,
                        f.time,
                        if f.passed { "ok" } else { "FAIL" }
                    );
                }
                if let Some(a) = &outcome.audio {
                    eprintln!(
                        "{} audio: {} ({})",
                        outcome.name,
                        if a.passed() { "ok" } else { "FAIL" },
                        a.summary()
                    );
                }
                if !outcome.passed() {
                    failed.push(outcome.name);
                }
            }
            Err(e) => {
                failed.push(format!("{}: {e}", case.display()));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "golden failures: {failed:#?}\nartifacts under {}",
        failures.display()
    );
}
