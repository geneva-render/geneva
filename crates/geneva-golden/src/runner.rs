use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use geneva_render::{CpuRenderer, Renderer};
use geneva_timeline::{AssetInfo, Ratio, Time, load_with};
use serde::Deserialize;
use thiserror::Error;

use geneva_timeline::Composition;

use crate::audio::{AudioComparison, AudioTolerance, Samples, compare_audio, from_wav, to_wav};
use crate::compare::{Comparison, Rgba8Image, Tolerance, compare, diff_image};

/// Environment variable that, when set, rewrites reference frames instead
/// of comparing against them.
pub const UPDATE_ENV: &str = "GENEVA_UPDATE_GOLDEN";

/// The files under the golden root, for the markup and stylesheets a
/// scene points at. Media durations are not probed: a golden scene says
/// how long its clips are.
struct RootFiles(PathBuf);

impl AssetInfo for RootFiles {
    fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
        None
    }

    fn text(&self, _: &str, src: &str) -> Option<String> {
        fs::read_to_string(self.0.join(src)).ok()
    }

    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(self.0.join(path)).ok()
    }

    fn exists(&self, path: &str) -> Option<bool> {
        Some(self.0.join(path).is_file())
    }
}

/// The `golden.json` file that accompanies each scene.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseSpec {
    /// Times to render, in timeline time syntax.
    #[serde(default)]
    frames: Vec<String>,
    /// Tolerance override.
    #[serde(default)]
    tolerance: Tolerance,
    /// The mixed audio, compared with `expected/audio.wav`.
    #[serde(default)]
    audio: Option<AudioSpec>,
}

/// The audio half of a case.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioSpec {
    /// The integrated loudness the output should measure, in LUFS, where
    /// the scene sets a target: a property of the signal, checked
    /// against the mix as well as the reference.
    #[serde(default)]
    lufs: Option<f64>,
    /// The true peak the output must stay under, in dBTP, where the
    /// scene sets a ceiling; a tenth of a decibel is allowed over it.
    #[serde(default)]
    max_true_peak_dbtp: Option<f64>,
    #[serde(default)]
    tolerance: AudioTolerance,
}

/// Produces a case's mixed audio: what the file would carry, mono or
/// stereo as the scene's output says. The mixer lives in the media crate,
/// so the runner takes it as a callback and a case with audio is skipped
/// where there is none.
pub type Mix = dyn Fn(&Composition, &Path) -> Result<Samples, String>;

/// Result for the audio of a case.
#[derive(Debug)]
pub enum AudioOutcome {
    /// No mixer was given, so the audio was not checked.
    Skipped,
    /// The reference was (re)written.
    Written(PathBuf),
    /// Compared with the reference.
    Compared {
        /// Path of the reference.
        expected: PathBuf,
        /// The measured difference.
        comparison: AudioComparison,
        /// The measured loudness against the case's stated one, when it
        /// states one: (measured, wanted).
        stated: Option<(Option<f64>, f64)>,
        /// The measured true peak against the case's ceiling, when it
        /// states one: (measured, ceiling), in dBTP.
        peak: Option<(f64, f64)>,
        /// Whether the comparison and the stated loudness both held.
        passed: bool,
    },
}

impl AudioOutcome {
    /// Whether the audio passed (a skipped or written one does).
    #[must_use]
    pub fn passed(&self) -> bool {
        match self {
            Self::Skipped | Self::Written(_) => true,
            Self::Compared { passed, .. } => *passed,
        }
    }

    /// One line for a log.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Skipped => "skipped (no mixer)".to_owned(),
            Self::Written(_) => "written".to_owned(),
            Self::Compared {
                comparison,
                stated,
                peak,
                ..
            } => {
                let mut line = comparison.summary();
                if let Some((measured, wanted)) = stated {
                    let _ = write!(
                        line,
                        "; stated {wanted} LUFS, measured {}",
                        measured.map_or("silence".to_owned(), |m| format!("{m:.2}"))
                    );
                }
                if let Some((measured, ceiling)) = peak {
                    let _ = write!(line, "; true peak {measured:.2} dBTP under {ceiling}");
                }
                line
            }
        }
    }
}

/// Why a case could not run at all.
#[derive(Debug, Error)]
pub enum GoldenError {
    /// A file in the case directory could not be read or written.
    #[error("{path}: {reason}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Description.
        reason: String,
    },
    /// `scene.json` did not validate.
    #[error("scene has errors:\n{0}")]
    Scene(String),
    /// A frame failed to render.
    #[error("frame at {time}: {reason}")]
    Render {
        /// Requested time.
        time: String,
        /// Renderer error.
        reason: String,
    },
}

/// Result for one rendered frame of a case.
#[derive(Debug)]
pub struct FrameOutcome {
    /// The frame time as written in `golden.json`.
    pub time: String,
    /// Path of the reference image.
    pub expected: PathBuf,
    /// The measured difference, or `None` when the reference was (re)written.
    pub comparison: Option<Comparison>,
    /// Whether the frame passed.
    pub passed: bool,
}

/// Result for a whole case.
#[derive(Debug)]
pub struct CaseOutcome {
    /// Case name (directory name).
    pub name: String,
    /// Per-frame results.
    pub frames: Vec<FrameOutcome>,
    /// The audio, where the case has any.
    pub audio: Option<AudioOutcome>,
}

impl CaseOutcome {
    /// Whether every frame, and the audio, passed.
    pub fn passed(&self) -> bool {
        self.frames.iter().all(|f| f.passed) && self.audio.as_ref().is_none_or(AudioOutcome::passed)
    }
}

/// Lists case directories (those containing `scene.json`) under `root`,
/// sorted by name.
pub fn discover_cases(root: &Path) -> Vec<PathBuf> {
    let mut cases: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("scene.json").is_file())
        .collect();
    cases.sort();
    cases
}

/// Renders every frame listed in a case's `golden.json` with the CPU
/// renderer and compares it with `expected/<time>.png`. Asset paths in
/// `scene.json` are relative to the golden root (the case's parent).
///
/// On failure the rendered frame and a diff image are written under
/// `failure_dir` for inspection. With [`UPDATE_ENV`] set, references are
/// rewritten and every frame passes.
pub fn run_case(case_dir: &Path, failure_dir: &Path) -> Result<CaseOutcome, GoldenError> {
    run_case_with(case_dir, failure_dir, None)
}

/// [`run_case`] with a mixer for the audio of a case that has some,
/// compared with `expected/audio.wav` on numbers ([`AudioTolerance`])
/// rather than bytes. Without a mixer the audio is skipped, and the
/// outcome says so.
pub fn run_case_with(
    case_dir: &Path,
    failure_dir: &Path,
    mix: Option<&Mix>,
) -> Result<CaseOutcome, GoldenError> {
    let mut cpu =
        |root: &Path| -> Box<dyn Renderer> { Box::new(CpuRenderer::with_asset_root(root)) };
    run_case_on(case_dir, failure_dir, &mut cpu, mix)
}

/// [`run_case_with`] on a renderer of the caller's choosing, built for
/// the golden root (the case's parent, which asset paths are relative
/// to). This is how a second renderer is checked against the references
/// the CPU renderer wrote, under the case's tolerance.
pub fn run_case_on(
    case_dir: &Path,
    failure_dir: &Path,
    make: &mut dyn FnMut(&Path) -> Box<dyn Renderer>,
    mix: Option<&Mix>,
) -> Result<CaseOutcome, GoldenError> {
    let name = case_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("case")
        .to_owned();
    let read = |p: &Path| {
        fs::read_to_string(p).map_err(|e| GoldenError::Io {
            path: p.to_owned(),
            reason: e.to_string(),
        })
    };
    let spec: CaseSpec =
        serde_json::from_str(&read(&case_dir.join("golden.json"))?).map_err(|e| {
            GoldenError::Io {
                path: case_dir.join("golden.json"),
                reason: e.to_string(),
            }
        })?;
    // Assets resolve against the golden root so cases can share files such
    // as fonts; markup is read from there too, so its errors are reported
    // and its text reaches the renderer.
    let root = case_dir.parent().unwrap_or(case_dir);
    let loaded = load_with(
        &read(&case_dir.join("scene.json"))?,
        &RootFiles(root.to_path_buf()),
    );
    let Some(comp) = loaded.composition else {
        let text = loaded
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<String>();
        return Err(GoldenError::Scene(text));
    };
    let update = std::env::var_os(UPDATE_ENV).is_some();
    let mut renderer = make(root);
    let expected_dir = case_dir.join("expected");
    let mut frames = Vec::new();
    for time_text in &spec.frames {
        let time = Time::parse(time_text)
            .map_err(|e| GoldenError::Render {
                time: time_text.clone(),
                reason: e,
            })?
            .resolve(comp.fps);
        let frame = renderer
            .render_frame(&comp, time)
            .map_err(|e| GoldenError::Render {
                time: time_text.clone(),
                reason: e.to_string(),
            })?;
        let actual = Rgba8Image::new(frame.width(), frame.height(), frame.to_rgba8());
        let expected_path = expected_dir.join(format!("{}.png", file_stem(time_text)));

        if update || !expected_path.exists() {
            fs::create_dir_all(&expected_dir).map_err(|e| GoldenError::Io {
                path: expected_dir.clone(),
                reason: e.to_string(),
            })?;
            let png = actual.to_png().map_err(|e| GoldenError::Io {
                path: expected_path.clone(),
                reason: e.to_string(),
            })?;
            fs::write(&expected_path, png).map_err(|e| GoldenError::Io {
                path: expected_path.clone(),
                reason: e.to_string(),
            })?;
            frames.push(FrameOutcome {
                time: time_text.clone(),
                expected: expected_path,
                comparison: None,
                passed: true,
            });
            continue;
        }

        let expected_bytes = fs::read(&expected_path).map_err(|e| GoldenError::Io {
            path: expected_path.clone(),
            reason: e.to_string(),
        })?;
        let expected = Rgba8Image::from_png(&expected_bytes).map_err(|e| GoldenError::Io {
            path: expected_path.clone(),
            reason: e.to_string(),
        })?;
        let comparison = compare(&actual, &expected, spec.tolerance.channel_epsilon);
        let passed = comparison.passes(&spec.tolerance);
        if !passed {
            let dir = failure_dir.join(&name);
            let _ = fs::create_dir_all(&dir);
            let stem = file_stem(time_text);
            if let Ok(png) = actual.to_png() {
                let _ = fs::write(dir.join(format!("{stem}.actual.png")), png);
            }
            if comparison.same_size {
                if let Ok(png) = diff_image(&actual, &expected).to_png() {
                    let _ = fs::write(dir.join(format!("{stem}.diff.png")), png);
                }
            }
        }
        frames.push(FrameOutcome {
            time: time_text.clone(),
            expected: expected_path,
            comparison: Some(comparison),
            passed,
        });
    }
    let audio = match (&spec.audio, mix) {
        (None, _) => None,
        (Some(_), None) => Some(AudioOutcome::Skipped),
        (Some(audio_spec), Some(mix)) => {
            let samples = mix(&comp, root).map_err(|reason| GoldenError::Render {
                time: "audio".to_owned(),
                reason,
            })?;
            let expected_path = expected_dir.join("audio.wav");
            if update || !expected_path.exists() {
                fs::create_dir_all(&expected_dir).map_err(|e| GoldenError::Io {
                    path: expected_dir.clone(),
                    reason: e.to_string(),
                })?;
                fs::write(&expected_path, to_wav(&samples)).map_err(|e| GoldenError::Io {
                    path: expected_path.clone(),
                    reason: e.to_string(),
                })?;
                Some(AudioOutcome::Written(expected_path))
            } else {
                let bytes = fs::read(&expected_path).map_err(|e| GoldenError::Io {
                    path: expected_path.clone(),
                    reason: e.to_string(),
                })?;
                let expected = from_wav(&bytes).map_err(|reason| GoldenError::Io {
                    path: expected_path.clone(),
                    reason,
                })?;
                let comparison = compare_audio(&samples, &expected);
                let stated = audio_spec
                    .lufs
                    .map(|wanted| (comparison.lufs_actual, wanted));
                let stated_ok = stated.is_none_or(|(measured, wanted)| {
                    measured.is_some_and(|m| (m - wanted).abs() <= audio_spec.tolerance.lufs)
                });
                let peak = audio_spec.max_true_peak_dbtp.map(|ceiling| {
                    let measured = geneva_audio::true_peak(
                        samples.rate,
                        usize::from(samples.channels.max(1)),
                        &samples.data,
                    );
                    (geneva_audio::to_db(f64::from(measured)), ceiling)
                });
                let peak_ok = peak.is_none_or(|(measured, ceiling)| measured <= ceiling + 0.1);
                let passed = comparison.passes(&audio_spec.tolerance) && stated_ok && peak_ok;
                if !passed {
                    let dir = failure_dir.join(&name);
                    let _ = fs::create_dir_all(&dir);
                    let _ = fs::write(dir.join("audio.actual.wav"), to_wav(&samples));
                }
                Some(AudioOutcome::Compared {
                    expected: expected_path,
                    comparison,
                    stated,
                    peak,
                    passed,
                })
            }
        }
    };
    Ok(CaseOutcome {
        name,
        frames,
        audio,
    })
}

/// Turns a time like `1.5s` or `00:00:01.5` into a safe file stem.
fn file_stem(time: &str) -> String {
    time.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
