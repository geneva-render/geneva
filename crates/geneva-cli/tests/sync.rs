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
        ("editlist-73.mov", at(24, 1.0)),
        ("audio-long.mp4", at(24, 1.0)),
        ("audio-short.mp4", at(24, 1.0)),
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

/// The timeline the farm tests render: a scaled video with its sound, so
/// the picture is composited and the output has both tracks.
fn farm_timeline(dir: &Path) -> PathBuf {
    let text = r#"{"geneva":"1.0",
        "output":{"width":160,"height":90,"fps":24,"duration":"3s",
                  "encode":{"video":{"crf":10,"preset":"ultrafast"}}},
        "assets":{"clip":{"src":"cfr.mp4"}},
        "layers":[{"clips":[{"source":{"kind":"video","asset":"clip"},
                             "transform":{"scale":0.75}}]}]}"#;
    // The farm sends the files the timeline reads, so the clip sits
    // beside it rather than under --assets.
    std::fs::copy(corpus().join("cfr.mp4"), dir.join("cfr.mp4")).unwrap();
    let timeline = dir.join("farm.json");
    std::fs::write(&timeline, text).unwrap();
    timeline
}

/// 72 frames with sound, within 40 dB of a single run at the frames
/// around each part's edges.
fn assert_like_single(single: &Path, other: &Path) {
    let info = probe(other).unwrap();
    assert_eq!(info.video.as_ref().unwrap().frames, Some(72));
    assert!(info.audio.is_some());
    let mut a = VideoReader::open(single, ColorTags::default()).unwrap();
    let mut b = VideoReader::open(other, ColorTags::default()).unwrap();
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
        assert!(psnr > 40.0, "frame {n}: {psnr:.1} dB");
    }
}

#[test]
fn a_farm_with_local_and_launched_workers_matches_a_single_run() {
    let dir = tempfile::tempdir().unwrap();
    let timeline = farm_timeline(dir.path());
    let t = timeline.to_str().unwrap();
    let single = dir.path().join("single.mp4");
    run(&["render", t, "-o", single.to_str().unwrap()]);
    let local = dir.path().join("local.mp4");
    run(&[
        "farm",
        t,
        "-o",
        local.to_str().unwrap(),
        "--parts",
        "3",
        "--local",
        "2",
        "--listen",
        "127.0.0.1:0",
    ]);
    assert_like_single(&single, &local);
    // Workers started by a command, as ssh or docker would, and none
    // here: they find the farm through the environment.
    let exe = assert_cmd::cargo::cargo_bin("geneva");
    let launched = dir.path().join("launched.mp4");
    run(&[
        "farm",
        t,
        "-o",
        launched.to_str().unwrap(),
        "--parts",
        "3",
        "--local",
        "0",
        "--listen",
        "127.0.0.1:0",
        "--launch",
        &format!("'{}' worker --name launched", exe.display()),
        "--launch-count",
        "2",
    ]);
    assert_like_single(&single, &launched);
}

#[test]
#[cfg(unix)]
fn a_farm_gives_launched_workers_the_url_it_is_told() {
    // The launch command only writes down the address it was given; the
    // local worker renders everything.
    let dir = tempfile::tempdir().unwrap();
    let timeline = farm_timeline(dir.path());
    let t = timeline.to_str().unwrap();
    let got = dir.path().join("url.txt");
    let out = dir.path().join("out.mp4");
    let farm = geneva()
        .args(["farm", t, "-o", out.to_str().unwrap(), "--parts", "3"])
        .args(["--listen", "127.0.0.1:0", "--url", "http://farm.invalid:9/"])
        .args(["--launch-count", "1", "--launch"])
        .arg(format!("echo \"$GENEVA_FARM_URL\" > '{}'", got.display()))
        .output()
        .unwrap();
    assert!(
        farm.status.success(),
        "{}",
        String::from_utf8_lossy(&farm.stderr)
    );
    // The trailing slash is dropped, and the printed command uses it too.
    assert_eq!(
        std::fs::read_to_string(&got).unwrap().trim(),
        "http://farm.invalid:9"
    );
    let stderr = String::from_utf8_lossy(&farm.stderr);
    assert!(
        stderr.contains("--connect http://farm.invalid:9 "),
        "{stderr}"
    );
    assert_eq!(probe(&out).unwrap().video.unwrap().frames, Some(72));
}

/// PSNR in dB between frame `n` (at 24 fps) of two files.
fn frame_psnr(a: &Path, b: &Path, n: u64) -> f64 {
    let t = Ratio::from_int(n as i64) / Ratio::from_int(24);
    let x = VideoReader::open(a, ColorTags::default())
        .unwrap()
        .frame_at(t)
        .unwrap()
        .pixels
        .clone();
    let y = VideoReader::open(b, ColorTags::default())
        .unwrap()
        .frame_at(t)
        .unwrap()
        .pixels
        .clone();
    let mse: f64 = x
        .iter()
        .zip(&y)
        .map(|(p, q)| {
            let d = |u: f32, v: f32| f64::from(u - v).powi(2);
            (d(p.r, q.r) + d(p.g, q.g) + d(p.b, q.b)) / 3.0
        })
        .sum::<f64>()
        / x.len() as f64;
    10.0 * (1.0 / mse.max(1e-12)).log10()
}

#[test]
#[cfg(unix)]
fn workers_follow_a_farm_that_draws_no_system_fonts() {
    // A worker started without --no-system-fonts still draws the text in
    // the built-in face when the farm was started with it.
    let dir = tempfile::tempdir().unwrap();
    let timeline = dir.path().join("text.json");
    std::fs::write(
        &timeline,
        r#"{"geneva":"1.0",
            "output":{"width":320,"height":180,"fps":24,"duration":"2s",
                      "encode":{"video":{"crf":10,"preset":"ultrafast"}}},
            "layers":[{"clips":[{"source":{"kind":"html",
                "html":"<p style='color:white;font:40px sans-serif'>Farm test</p>"}}]}]}"#,
    )
    .unwrap();
    let t = timeline.to_str().unwrap();
    let path = |name: &str| dir.path().join(name);
    let own = path("own.mp4");
    let builtin = path("builtin.mp4");
    run(&["render", t, "-o", own.to_str().unwrap()]);
    run(&[
        "--no-system-fonts",
        "render",
        t,
        "-o",
        builtin.to_str().unwrap(),
    ]);
    let farmed = path("farmed.mp4");
    let exe = assert_cmd::cargo::cargo_bin("geneva");
    run(&[
        "--no-system-fonts",
        "farm",
        t,
        "-o",
        farmed.to_str().unwrap(),
        "--parts",
        "2",
        "--local",
        "0",
        "--listen",
        "127.0.0.1:0",
        "--launch",
        &format!("'{}' worker", exe.display()),
        "--launch-count",
        "1",
    ]);
    for n in [0, 24, 47] {
        let psnr = frame_psnr(&builtin, &farmed, n);
        assert!(
            psnr > 40.0,
            "frame {n}: {psnr:.1} dB from the built-in face"
        );
    }
    // Where this machine's sans-serif is another face, the farm's frames
    // are not the ones it would draw.
    if frame_psnr(&own, &builtin, 24) < 30.0 {
        let psnr = frame_psnr(&own, &farmed, 24);
        assert!(psnr < 30.0, "{psnr:.1} dB from this machine's face");
    }
}

/// A farm running in the background, stopped when it goes out of scope
/// so that a failed test leaves no process behind.
struct Farm(std::process::Child);

impl Drop for Farm {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `geneva farm` with `args` and returns it with the port it
/// listens on, read from the line it prints once it is listening. Asking
/// the system for a free port first and passing it would leave a moment
/// in which another test's farm can take that port.
fn start_farm(args: &[&str]) -> (Farm, u16) {
    use std::io::BufRead;
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("geneva"))
        .arg("farm")
        .args(args)
        .args(["--listen", "127.0.0.1:0"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = String::new();
    let port = loop {
        let Some(Ok(line)) = lines.next() else {
            panic!("the farm stopped before listening:\n{seen}");
        };
        if let Some(rest) = line.split(" at http://127.0.0.1:").nth(1) {
            break rest.split(';').next().unwrap().parse().unwrap();
        }
        seen.push_str(&line);
        seen.push('\n');
    };
    std::thread::spawn(move || lines.for_each(drop));
    (Farm(child), port)
}

/// One HTTP request to the farm, as a worker would send it.
fn farm_call(port: u16, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nAuthorization: Bearer secret\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    s.read_to_string(&mut answer).unwrap();
    let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
    let json = answer
        .split_once("\r\n\r\n")
        .and_then(|(_, b)| serde_json::from_str(b).ok())
        .unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[test]
fn a_farm_gives_a_lost_part_to_another_worker_and_refuses_another_build() {
    let dir = tempfile::tempdir().unwrap();
    let timeline = farm_timeline(dir.path());
    let out = dir.path().join("out.mp4");
    let (mut farm, port) = start_farm(&[
        timeline.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--parts",
        "3",
        "--local",
        "0",
        "--token",
        "secret",
        "--part-timeout",
        "2",
    ]);
    let (_, status) = farm_call(port, "GET", "/status", "");
    let fingerprint = status["fingerprint"].as_str().unwrap().to_owned();
    // Another build is turned away before it is given anything.
    let (code, answer) = farm_call(
        port,
        "POST",
        "/hello",
        r#"{"fingerprint":"geneva 0.0.1; x264 none"}"#,
    );
    assert_eq!(code, 409);
    assert!(
        answer["reason"]
            .as_str()
            .unwrap()
            .contains("would not join")
    );
    // A worker that takes a part and is never heard from again.
    let hello = serde_json::json!({ "fingerprint": fingerprint, "name": "lost" }).to_string();
    let (code, job) = farm_call(port, "POST", "/hello", &hello);
    assert_eq!(code, 200);
    let claim = serde_json::json!({ "worker": job["worker"] }).to_string();
    let (code, part) = farm_call(port, "POST", "/claim", &claim);
    assert_eq!(code, 200);
    assert_eq!(part["part"], 0);
    // A worker whose --stop-after time has passed takes nothing.
    let url = format!("http://127.0.0.1:{port}");
    let stopped = geneva()
        .args(["--format", "json", "worker", "--connect", &url])
        .args(["--token", "secret", "--stop-after", "0"])
        .output()
        .unwrap();
    assert!(stopped.status.success());
    let report: serde_json::Value = serde_json::from_slice(&stopped.stdout).unwrap();
    assert_eq!(report["stopped"], true, "{report}");
    assert_eq!(report["parts"].as_array().unwrap().len(), 0, "{report}");
    // A real worker finishes everything, the lost part included once its
    // two seconds are up.
    run(&["worker", "--connect", &url, "--token", "secret"]);
    assert!(farm.0.wait().unwrap().success());
    let info = probe(&out).unwrap();
    assert_eq!(info.video.unwrap().frames, Some(72));
    assert!(info.audio.is_some());
}

#[test]
fn a_farm_keeps_the_marks_in_place_with_sources_fetched_in_ranges() {
    // MP4 and QuickTime sources reach the workers as the byte ranges
    // their parts read, with holes elsewhere; the rest go whole. Either
    // way the flash and the tone land where a single run puts them.
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, want) in corpus_files() {
        let info = probe(&corpus().join(name)).unwrap();
        let fps = info.video.unwrap().fps;
        let text = format!(
            r#"{{"geneva":"1.0",
                "output":{{"width":160,"height":90,"fps":"{}/{}","duration":"3s",
                           "encode":{{"video":{{"crf":16,"preset":"ultrafast"}}}}}},
                "assets":{{"clip":{{"src":"{name}"}}}},
                "layers":[{{"clips":[{{"source":{{"kind":"video","asset":"clip"}}}}]}}]}}"#,
            fps.numer(),
            fps.denom(),
        );
        std::fs::copy(corpus().join(name), dir.path().join(name)).unwrap();
        let timeline = dir.path().join(format!("{name}.farm.json"));
        std::fs::write(&timeline, text).unwrap();
        let out = dir.path().join(format!("farm-{name}.mp4"));
        run(&[
            "farm",
            timeline.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--parts",
            "3",
            "--local",
            "2",
            "--listen",
            "127.0.0.1:0",
        ]);
        problems.extend(check("farm", name, &out, &want));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

#[test]
fn a_farm_fetches_part_of_a_long_source_and_renders_it_as_a_single_run_does() {
    // 48 s of the corpus clip, so each of twelve parts leaves most of
    // the file as a hole on its worker.
    let dir = tempfile::tempdir().unwrap();
    let clip = corpus().join("cfr.mp4");
    let c = clip.to_str().unwrap();
    let long = dir.path().join("long.mp4");
    let mut args = vec!["concat"];
    args.extend([c; 16]);
    args.extend(["-o", long.to_str().unwrap()]);
    run(&args);
    let text = r#"{"geneva":"1.0",
        "output":{"width":160,"height":90,"fps":24,
                  "encode":{"video":{"crf":10,"preset":"ultrafast"}}},
        "assets":{"clip":{"src":"long.mp4"}},
        "layers":[{"clips":[{"source":{"kind":"video","asset":"clip"},
                             "transform":{"scale":0.75}}]}]}"#;
    let timeline = dir.path().join("long.json");
    std::fs::write(&timeline, text).unwrap();
    let t = timeline.to_str().unwrap();
    let single = dir.path().join("single.mp4");
    run(&["render", t, "-o", single.to_str().unwrap()]);
    let farmed = dir.path().join("farmed.mp4");
    let out = geneva()
        .args([
            "--format",
            "json",
            "farm",
            t,
            "-o",
            farmed.to_str().unwrap(),
        ])
        .args(["--parts", "12", "--local", "2", "--listen", "127.0.0.1:0"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let fetched = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["message"].as_str())
        .find(|m| m.starts_with("workers fetched"))
        .unwrap()
        .to_owned();
    // "workers fetched A of files; each whole copy is B, so N whole
    // copies would have been C": A is under C.
    let size = |s: &str| -> f64 {
        let (n, unit) = s.trim().split_once(' ').unwrap();
        let n: f64 = n.parse().unwrap();
        n * match unit.trim_end_matches(|c: char| !c.is_ascii_alphabetic()) {
            "kB" => 1e3,
            "MB" => 1e6,
            _ => 1e9,
        }
    };
    let a = fetched["workers fetched ".len()..]
        .split(" of files")
        .next()
        .unwrap();
    let c = fetched.rsplit("would have been ").next().unwrap();
    assert!(size(a) < 0.8 * size(c), "{fetched}");
    let frames = probe(&single).unwrap().video.unwrap().frames.unwrap();
    assert_eq!(probe(&farmed).unwrap().video.unwrap().frames, Some(frames));
    let mut x = VideoReader::open(&single, ColorTags::default()).unwrap();
    let mut y = VideoReader::open(&farmed, ColorTags::default()).unwrap();
    for n in (0..frames).step_by(23) {
        let t = Ratio::from_int(n as i64) / Ratio::from_int(24);
        let p = x.frame_at(t).unwrap().pixels.clone();
        let q = y.frame_at(t).unwrap().pixels.clone();
        let mse: f64 = p
            .iter()
            .zip(&q)
            .map(|(p, q)| {
                let d = |u: f32, v: f32| f64::from(u - v).powi(2);
                (d(p.r, q.r) + d(p.g, q.g) + d(p.b, q.b)) / 3.0
            })
            .sum::<f64>()
            / p.len() as f64;
        let psnr = 10.0 * (1.0 / mse.max(1e-12)).log10();
        assert!(psnr > 40.0, "frame {n}: {psnr:.1} dB");
    }
}

/// Files whose length is not a whole number of frames in six decimals,
/// or whose audio runs past or stops short of the picture: every path
/// writes the frames the source shows and no more. A source of 73
/// frames at 24 fps lasts 3.0416666... s, which a verb writes into its
/// document as 3.041667 s, a third of a microsecond past the last frame;
/// that used to ask the direct path for a 74th frame no clip covered.
#[test]
fn a_length_that_ends_mid_frame_adds_no_frame() {
    let dir = tempfile::tempdir().unwrap();
    let srt = dir.path().join("a.srt");
    std::fs::write(&srt, "1\n00:00:00,500 --> 00:00:02,500\nHello\n").unwrap();
    // The count the muxer wrote: MP4 keeps one entry per sample.
    let frames_of = |path: &Path| probe(path).unwrap().video.unwrap().frames.unwrap_or(0);
    let corpus_dir = corpus();
    let corpus_dir = corpus_dir.to_str().unwrap();
    let mut problems = Vec::new();
    for (name, want) in [
        ("editlist-73.mov", 73u32),
        ("audio-long.mp4", 72),
        ("audio-short.mp4", 72),
    ] {
        let src = corpus().join(name);
        let src = src.to_str().unwrap();
        // Burned captions with the defaults (copied where it can be),
        // re-encoded throughout, and as a document with a markup clip
        // and the duration written out to six decimals.
        let doc = dir.path().join("doc.json");
        std::fs::write(
            &doc,
            format!(
                r#"{{"geneva":"1.1","output":{{"width":160,"height":90,"fps":24,"duration":"{:.6}s"}},
                   "assets":{{"v":{{"src":"{name}"}}}},
                   "layers":[{{"clips":[{{"source":{{"kind":"video","asset":"v"}}}}]}},
                             {{"clips":[{{"source":{{"kind":"html","html":"<p style='color:white'>Hi</p>"}},
                                         "start":"0.5s","duration":"1s"}}]}}]}}"#,
                f64::from(want) / 24.0
            ),
        )
        .unwrap();
        let runs: [(&str, Vec<&str>); 3] = [
            (
                "burn",
                vec!["subtitles", src, "--burn", srt.to_str().unwrap()],
            ),
            (
                "burn --crf",
                vec![
                    "subtitles",
                    src,
                    "--burn",
                    srt.to_str().unwrap(),
                    "--crf",
                    "20",
                ],
            ),
            (
                "document",
                vec![
                    "render",
                    doc.to_str().unwrap(),
                    "--crf",
                    "20",
                    "--assets",
                    corpus_dir,
                ],
            ),
        ];
        for (what, mut args) in runs {
            let out = dir
                .path()
                .join(format!("{name}-{}.mp4", what.replace(' ', "")));
            args.extend(["-o", out.to_str().unwrap()]);
            let run = geneva().args(&args).output().unwrap();
            if !run.status.success() {
                problems.push(format!(
                    "{name}, {what}: {}",
                    String::from_utf8_lossy(&run.stderr)
                ));
                continue;
            }
            let got = frames_of(&out);
            if got != u64::from(want) {
                problems.push(format!("{name}, {what}: {got} frames, want {want}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The legacy corpus (`tests/media/legacy`, `scripts/make-legacy-corpus.sh`):
/// DV, MPEG-2 in a program and a transport stream, MS-MPEG4 v3, Motion
/// JPEG and QuickTime RLE, each with a white frame at 0.5 s and a tone
/// from 0.5 s. Converted, each keeps every frame (as ffmpeg counts them)
/// and both marks.
#[test]
fn legacy_sources_keep_their_frames_and_marks() {
    let legacy = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/media/legacy");
    let dir = tempfile::tempdir().unwrap();
    let mut problems = Vec::new();
    for (name, frames, flash_frame) in [
        ("dv-ntsc.dv", 20, 15),
        ("dv-pal.dv", 17, 13),
        ("mpeg2.mpg", 25, 13),
        ("mpeg2.ts", 30, 15),
        ("msmpeg4v3.avi", 25, 13),
        ("mjpeg.avi", 25, 13),
        ("qtrle.mov", 25, 13),
    ] {
        let out = dir.path().join(format!("{name}.mp4"));
        run(&[
            "convert",
            legacy.join(name).to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--crf",
            "16",
            "--preset",
            "ultrafast",
        ]);
        let written = probe(&out).unwrap().video.unwrap().frames;
        if written != Some(frames) {
            problems.push(format!("{name}: {written:?} frames (wanted {frames})"));
        }
        let want = Marks {
            flash_frame,
            tone_secs: 0.5,
        };
        problems.extend(check("convert", name, &out, &want));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
