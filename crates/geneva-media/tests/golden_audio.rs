//! The audio of every golden case that has some, mixed and compared with
//! its reference WAV on numbers rather than bytes.

#![cfg(feature = "media")]

use std::path::Path;

use geneva_golden::{Samples, discover_cases, run_case_with};
use geneva_media::AudioSettings;
use geneva_media::mix::Mixer;
use geneva_timeline::Composition;
use geneva_timeline::schema::AudioCodec;

/// The mix as the encoder would write it: at the output's rate, mono
/// where the output is mono.
fn mix(comp: &Composition, root: &Path) -> Result<Samples, String> {
    let audio = comp.audio_output.as_ref();
    let rate = audio.and_then(|a| a.sample_rate).unwrap_or(48_000);
    let channels = audio.and_then(|a| a.channels).unwrap_or(2).clamp(1, 2);
    let settings = AudioSettings {
        codec: AudioCodec::Pcm,
        bitrate_kbps: 0,
        sample_rate: rate,
        channels,
        loudness: audio.and_then(|a| a.loudness.clone()),
    };
    let mut mixer = Mixer::for_output(comp, root, &settings);
    let mut data = Vec::new();
    while let Some(block) = mixer.next_block(rate as usize).map_err(|e| e.to_string())? {
        if channels == 1 {
            data.extend(block.chunks_exact(2).map(|lr| (lr[0] + lr[1]) * 0.5));
        } else {
            data.extend(block);
        }
    }
    if let Some(report) = mixer.loudness_report() {
        eprintln!("loudness: {report:?}");
    }
    Ok(Samples {
        rate,
        channels,
        data,
    })
}

#[test]
fn golden_audio_matches_its_references() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    let failures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/golden-failures");
    let mut failed = Vec::new();
    let mut checked = 0;
    for case in discover_cases(&root) {
        match run_case_with(&case, &failures, Some(&mix)) {
            Ok(outcome) => {
                if let Some(a) = &outcome.audio {
                    checked += 1;
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
            Err(e) => failed.push(format!("{}: {e}", case.display())),
        }
    }
    assert!(checked > 0, "no golden case has audio");
    assert!(
        failed.is_empty(),
        "golden failures: {failed:#?}\nartifacts under {}",
        failures.display()
    );
}
