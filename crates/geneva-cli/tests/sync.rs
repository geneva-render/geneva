//! A/V sync against the corpus in `tests/media/sync`: files whose
//! timestamps carry the traps real files carry, all from one scene with
//! a white flash and a 1 kHz tone at known times (see
//! `scripts/make-sync-corpus.sh`). Whatever path a file takes through
//! geneva, the flash and the tone must come out where they went in.

#![cfg(feature = "media")]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use geneva_color::ColorTags;
use geneva_media::{AudioReader, VideoReader, probe};
use geneva_timeline::Ratio;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/media/sync")
}

fn geneva() -> Command {
    Command::cargo_bin("geneva").unwrap()
}

/// Where the markers are in a file, as the output should show them:
/// the index of the first white frame and the second of the tone's
/// onset.
struct Marks {
    flash_frame: u64,
    tone_secs: f64,
}

/// The corpus, with the marks expected of each file: the flash is the
/// first frame at or after 1.0 s and the tone starts at 1.0 s, in every
/// file, whatever its streams' own timestamps do (in `audio-late` the
/// audio track begins half a second after the video, the tone still at
/// 1.0 s of the file).
fn corpus_files() -> Vec<(&'static str, Marks)> {
    let at = |flash_frame, tone_secs| Marks {
        flash_frame,
        tone_secs,
    };
    vec![
        ("cfr.mp4", at(24, 1.0)),
        ("bframes-editlist.mp4", at(24, 1.0)),
        ("bframes-negative-cts.mp4", at(24, 1.0)),
        ("vfr.mp4", at(24, 1.0)),
        ("start-at-10s.mp4", at(24, 1.0)),
        ("audio-late.mp4", at(24, 1.0)),
        ("audio-late.mkv", at(24, 1.0)),
        ("ntsc-2997.mp4", at(30, 1.0)),
        ("film-2398.mp4", at(24, 1.0)),
        ("aac-44k.mp4", at(24, 1.0)),
        ("opus.webm", at(24, 1.0)),
        ("mpegts.ts", at(24, 1.0)),
    ]
}

/// Finds the marks in a written file: the first frame whose center is
/// bright, and the first sample above the noise.
fn measure(path: &Path) -> Marks {
    let info = probe(path).unwrap();
    let v = info.video.unwrap();
    let fps = v.fps;
    let frames = (info.duration.unwrap() * fps).ceil().max(0) as u64;
    let mut reader = VideoReader::open(path, ColorTags::default()).unwrap();
    let mut flash_frame = None;
    for n in 0..frames {
        let img = reader.frame_at(Ratio::from_int(n as i64) / fps).unwrap();
        let center = img.pixels[(img.height / 2 * img.width + img.width / 2) as usize];
        if center.to_srgb8()[0] > 128 {
            flash_frame = Some(n);
            break;
        }
    }
    let mut audio = AudioReader::open(path).unwrap();
    let samples = audio.read(Ratio::ZERO, Ratio::from_int(3), 48000).unwrap();
    // The onset is where the signal first reaches a third of its peak,
    // whatever level the file carries.
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    let onset = samples
        .iter()
        .position(|s| s.abs() > peak / 3.0)
        .filter(|_| peak > 0.01)
        .map(|i| i as f64 / 2.0 / 48000.0);
    Marks {
        flash_frame: flash_frame.unwrap_or(u64::MAX),
        tone_secs: onset.unwrap_or(f64::NAN),
    }
}

fn check(what: &str, name: &str, out: &Path, want: &Marks) -> Vec<String> {
    let got = measure(out);
    let mut problems = Vec::new();
    if got.flash_frame != want.flash_frame {
        problems.push(format!(
            "{what} {name}: flash at frame {} (wanted {})",
            got.flash_frame, want.flash_frame
        ));
    }
    // Timestamps are sample-exact; 3 ms leaves room for the detector.
    let off = (got.tone_secs - want.tone_secs).abs();
    if off.is_nan() || off > 0.003 {
        problems.push(format!(
            "{what} {name}: tone at {:.4}s (wanted {:.4}s)",
            got.tone_secs, want.tone_secs
        ));
    }
    problems
}

fn run(args: &[&str]) {
    let out = geneva().args(args).output().unwrap();
    assert!(
        out.status.success(),
        "geneva {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn direct_path_keeps_the_marks_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, want) in corpus_files() {
        let out = dir.path().join(format!("direct-{name}.mp4"));
        run(&[
            "convert",
            corpus().join(name).to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--crf",
            "16",
            "--preset",
            "ultrafast",
        ]);
        problems.extend(check("direct", name, &out, &want));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn stream_copy_keeps_the_marks_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, want) in corpus_files() {
        if name == "vfr.mp4" {
            // A copied variable-rate stream keeps its own timing; the
            // marks are checked on the re-encoding paths.
            continue;
        }
        let ext = if Path::new(name).extension().is_some_and(|e| e == "webm") {
            "webm"
        } else {
            "mp4"
        };
        let out = dir.path().join(format!("copy-{name}.{ext}"));
        run(&[
            "convert",
            corpus().join(name).to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ]);
        problems.extend(check("copy", name, &out, &want));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn compositor_keeps_the_marks_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, want) in corpus_files() {
        let info = probe(&corpus().join(name)).unwrap();
        let fps = info.video.unwrap().fps;
        // Scaled down, the clip goes through the compositor rather than
        // straight from the decoder to the encoder; its audio goes
        // through the mixer as the clip's own.
        let text = format!(
            r#"{{"geneva":"1.0",
                "output":{{"width":160,"height":90,"fps":"{}/{}","duration":"3s",
                           "encode":{{"video":{{"crf":16,"preset":"ultrafast"}}}}}},
                "assets":{{"clip":{{"src":"{name}"}}}},
                "layers":[{{"clips":[{{"source":{{"kind":"video","asset":"clip"}},
                                       "transform":{{"scale":0.5}}}}]}}]}}"#,
            fps.numer(),
            fps.denom(),
        );
        let timeline = dir.path().join(format!("{name}.json"));
        std::fs::write(&timeline, text).unwrap();
        let out = dir.path().join(format!("render-{name}.mp4"));
        run(&[
            "render",
            timeline.to_str().unwrap(),
            "--assets",
            corpus().to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ]);
        problems.extend(check("render", name, &out, &want));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// The marks after a cut asked at 0.51 s. Copied, the cut moves back
/// to the keyframe half a second in (every corpus file has one there:
/// at 0.5 s, or 0.5005 s at the NTSC rates), so the flash comes half a
/// second in and the tone with it. Exact, the output starts with the
/// frame shown at 0.51 s and the tone at 0.49 s.
fn marks_after_cut(want: &Marks, fps: f64, exact: bool) -> Marks {
    if exact {
        Marks {
            flash_frame: want.flash_frame - (0.51 * fps).floor() as u64,
            tone_secs: want.tone_secs - 0.51,
        }
    } else {
        let keyframe = (0.5 * fps).round();
        Marks {
            flash_frame: want.flash_frame - keyframe as u64,
            tone_secs: want.tone_secs - keyframe / fps,
        }
    }
}

#[test]
fn trims_keep_the_marks_in_place_copied_and_exact() {
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, want) in corpus_files() {
        let fps = probe(&corpus().join(name))
            .unwrap()
            .video
            .unwrap()
            .fps
            .to_f64();
        let ext = if Path::new(name).extension().is_some_and(|e| e == "webm") {
            "webm"
        } else {
            "mp4"
        };
        if name != "vfr.mp4" {
            let out = dir.path().join(format!("trim-copy-{name}.{ext}"));
            run(&[
                "trim",
                corpus().join(name).to_str().unwrap(),
                "--from",
                "0.51s",
                "-o",
                out.to_str().unwrap(),
            ]);
            problems.extend(check(
                "trim (copy)",
                name,
                &out,
                &marks_after_cut(&want, fps, false),
            ));
        }
        let out = dir.path().join(format!("trim-exact-{name}.mp4"));
        run(&[
            "trim",
            corpus().join(name).to_str().unwrap(),
            "--from",
            "0.51s",
            "--exact",
            "-o",
            out.to_str().unwrap(),
        ]);
        problems.extend(check(
            "trim (exact)",
            name,
            &out,
            &marks_after_cut(&want, fps, true),
        ));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn chunked_encoding_keeps_the_marks_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, want) in corpus_files() {
        let info = probe(&corpus().join(name)).unwrap();
        let fps = info.video.unwrap().fps;
        // Three stretches of a 3 s output, encoded at once and joined.
        let text = format!(
            r#"{{"geneva":"1.0",
                "output":{{"width":160,"height":90,"fps":"{}/{}","duration":"3s",
                           "encode":{{"video":{{"crf":16,"preset":"ultrafast","chunks":3}}}}}},
                "assets":{{"clip":{{"src":"{name}"}}}},
                "layers":[{{"clips":[{{"source":{{"kind":"video","asset":"clip"}}}}]}}]}}"#,
            fps.numer(),
            fps.denom(),
        );
        let timeline = dir.path().join(format!("{name}.chunked.json"));
        std::fs::write(&timeline, text).unwrap();
        let out = dir.path().join(format!("chunked-{name}.mp4"));
        run(&[
            "render",
            timeline.to_str().unwrap(),
            "--assets",
            corpus().to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ]);
        problems.extend(check("chunked", name, &out, &want));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn chunked_and_single_runs_give_the_same_frames() {
    let dir = tempfile::tempdir().unwrap();
    let name = "cfr.mp4";
    let render = |chunks: u32| {
        let text = format!(
            r#"{{"geneva":"1.0",
                "output":{{"width":160,"height":90,"fps":24,"duration":"3s",
                           "encode":{{"video":{{"crf":10,"preset":"ultrafast","chunks":{chunks}}}}}}},
                "assets":{{"clip":{{"src":"{name}"}}}},
                "layers":[{{"clips":[{{"source":{{"kind":"video","asset":"clip"}},
                                       "transform":{{"scale":0.75}}}}]}}]}}"#
        );
        let timeline = dir.path().join(format!("chunks-{chunks}.json"));
        std::fs::write(&timeline, text).unwrap();
        let out = dir.path().join(format!("chunks-{chunks}.mp4"));
        run(&[
            "render",
            timeline.to_str().unwrap(),
            "--assets",
            corpus().to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ]);
        out
    };
    let single = render(1);
    let chunked = render(3);
    for path in [&single, &chunked] {
        let info = probe(path).unwrap();
        assert_eq!(
            info.video.as_ref().unwrap().frames,
            Some(72),
            "{}",
            path.display()
        );
        assert!(info.audio.is_some(), "{}", path.display());
    }
    // The same picture, within the encoders' rounding: the stretches
    // restart rate control, so bits differ, pixels barely.
    let mut a = VideoReader::open(&single, ColorTags::default()).unwrap();
    let mut b = VideoReader::open(&chunked, ColorTags::default()).unwrap();
    for n in [0u64, 23, 24, 25, 47, 48, 49, 71] {
        let t = Ratio::from_int(n as i64) / Ratio::from_int(24);
        let x = a.frame_at(t).unwrap().pixels.clone();
        let y = b.frame_at(t).unwrap().pixels.clone();
        let mse: f64 = x
            .iter()
            .zip(&y)
            .map(|(p, q)| {
                let d = |u: f32, v: f32| f64::from(u - v).powi(2);
                (d(p.r, q.r) + d(p.g, q.g) + d(p.b, q.b)) / 3.0
            })
            .sum::<f64>()
            / x.len() as f64;
        let psnr = 10.0 * (1.0 / mse.max(1e-12)).log10();
        assert!(
            psnr > 40.0,
            "frame {n}: {psnr:.1} dB between single and chunked"
        );
    }
}

/// Renders `timeline` the way separate machines would: `plan` into
/// `parts` parts, each part and the sound as their own run, then
/// `join`. Returns the joined file.
fn render_in_parts(timeline: &Path, assets: &Path, output: &Path, parts: u32) -> PathBuf {
    let planned = geneva()
        .args([
            "--format",
            "json",
            "plan",
            timeline.to_str().unwrap(),
            "--assets",
            assets.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--parts",
            &parts.to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        planned.status.success(),
        "plan: {}",
        String::from_utf8_lossy(&planned.stdout)
    );
    let plan: serde_json::Value = serde_json::from_slice(&planned.stdout).unwrap();
    let mut files = Vec::new();
    for part in plan["parts"].as_array().unwrap() {
        let range = format!("{}..{}", part["frames"][0], part["frames"][1]);
        let file = part["output"].as_str().unwrap().to_owned();
        run(&[
            "render",
            timeline.to_str().unwrap(),
            "--assets",
            assets.to_str().unwrap(),
            "--frames",
            &range,
            "-o",
            &file,
        ]);
        files.push(file);
    }
    let sound = plan["audio"]["output"].as_str().unwrap().to_owned();
    run(&[
        "render",
        timeline.to_str().unwrap(),
        "--assets",
        assets.to_str().unwrap(),
        "--audio-only",
        "-o",
        &sound,
    ]);
    let mut join = vec![
        "join",
        timeline.to_str().unwrap(),
        "--assets",
        assets.to_str().unwrap(),
    ];
    join.extend(files.iter().map(String::as_str));
    join.extend(["--audio", &sound, "-o", output.to_str().unwrap()]);
    run(&join);
    output.to_path_buf()
}

#[test]
fn parts_keep_the_marks_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, want) in corpus_files() {
        let info = probe(&corpus().join(name)).unwrap();
        let fps = info.video.unwrap().fps;
        // Three parts of a 3 s output, each rendered by its own process,
        // so the flash at 1 s is the first frame of the second part.
        let text = format!(
            r#"{{"geneva":"1.0",
                "output":{{"width":160,"height":90,"fps":"{}/{}","duration":"3s",
                           "encode":{{"video":{{"crf":16,"preset":"ultrafast"}}}}}},
                "assets":{{"clip":{{"src":"{name}"}}}},
                "layers":[{{"clips":[{{"source":{{"kind":"video","asset":"clip"}}}}]}}]}}"#,
            fps.numer(),
            fps.denom(),
        );
        let timeline = dir.path().join(format!("{name}.parts.json"));
        std::fs::write(&timeline, text).unwrap();
        let out = dir.path().join(format!("parts-{name}.mp4"));
        let out = render_in_parts(&timeline, &corpus(), &out, 3);
        problems.extend(check("parts", name, &out, &want));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn parts_and_single_runs_give_the_same_frames() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"geneva":"1.0",
        "output":{"width":160,"height":90,"fps":24,"duration":"3s",
                  "encode":{"video":{"crf":10,"preset":"ultrafast"}}},
        "assets":{"clip":{"src":"cfr.mp4"}},
        "layers":[{"clips":[{"source":{"kind":"video","asset":"clip"},
                             "transform":{"scale":0.75}}]}]}"#;
    let timeline = dir.path().join("parts.json");
    std::fs::write(&timeline, text).unwrap();
    let single = dir.path().join("single.mp4");
    run(&[
        "render",
        timeline.to_str().unwrap(),
        "--assets",
        corpus().to_str().unwrap(),
        "-o",
        single.to_str().unwrap(),
    ]);
    let joined = render_in_parts(&timeline, &corpus(), &dir.path().join("joined.mp4"), 3);
    for path in [&single, &joined] {
        let info = probe(path).unwrap();
        assert_eq!(
            info.video.as_ref().unwrap().frames,
            Some(72),
            "{}",
            path.display()
        );
        assert!(info.audio.is_some(), "{}", path.display());
    }
    // As with chunked encoding: each part restarts rate control, so the
    // bits differ and the pixels barely.
    let mut a = VideoReader::open(&single, ColorTags::default()).unwrap();
    let mut b = VideoReader::open(&joined, ColorTags::default()).unwrap();
    for n in [0u64, 23, 24, 25, 47, 48, 49, 71] {
        let t = Ratio::from_int(n as i64) / Ratio::from_int(24);
        let x = a.frame_at(t).unwrap().pixels.clone();
        let y = b.frame_at(t).unwrap().pixels.clone();
        let mse: f64 = x
            .iter()
            .zip(&y)
            .map(|(p, q)| {
                let d = |u: f32, v: f32| f64::from(u - v).powi(2);
                (d(p.r, q.r) + d(p.g, q.g) + d(p.b, q.b)) / 3.0
            })
            .sum::<f64>()
            / x.len() as f64;
        let psnr = 10.0 * (1.0 / mse.max(1e-12)).log10();
        assert!(
            psnr > 40.0,
            "frame {n}: {psnr:.1} dB between single and parts"
        );
    }
}

#[test]
fn join_refuses_a_missing_part_and_plan_refuses_what_cannot_split() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"geneva":"1.0",
        "output":{"width":160,"height":90,"fps":24,"duration":"6s"},
        "layers":[{"clips":[{"source":{"kind":"solid","color":"blue"}}]}]}"#;
    let timeline = dir.path().join("missing.json");
    std::fs::write(&timeline, text).unwrap();
    let t = timeline.to_str().unwrap();
    let part = |range: &str, name: &str| {
        let file = dir.path().join(name);
        run(&["render", t, "--frames", range, "-o", file.to_str().unwrap()]);
        file
    };
    let a = part("0..72", "a.mp4");
    let c = part("96..144", "c.mp4");
    let out = dir.path().join("out.mp4");
    let joined = geneva()
        .args(["join", t, a.to_str().unwrap(), c.to_str().unwrap()])
        .args(["-o", out.to_str().unwrap()])
        .output()
        .unwrap();
    let said = |o: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };
    assert!(!joined.status.success());
    assert!(
        said(&joined).contains("a part is missing"),
        "{}",
        said(&joined)
    );
    // A bitrate ceiling holds over the whole file: no plan.
    let capped = dir.path().join("capped.json");
    std::fs::write(
        &capped,
        text.replace(
            r#""duration":"6s"}"#,
            r#""duration":"6s","encode":{"video":{"max_bitrate_kbps":2000}}}"#,
        ),
    )
    .unwrap();
    let planned = geneva()
        .args([
            "plan",
            capped.to_str().unwrap(),
            "-o",
            "out.mp4",
            "--parts",
            "3",
        ])
        .output()
        .unwrap();
    assert!(!planned.status.success());
    assert!(said(&planned).contains("E504"), "{}", said(&planned));
}
