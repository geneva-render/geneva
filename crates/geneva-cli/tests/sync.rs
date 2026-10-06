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
