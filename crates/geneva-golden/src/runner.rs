use std::fs;
use std::path::{Path, PathBuf};

use geneva_render::{CpuRenderer, Renderer};
use geneva_timeline::{Time, load};
use serde::Deserialize;
use thiserror::Error;

use crate::compare::{Comparison, Rgba8Image, Tolerance, compare, diff_image};

/// Environment variable that, when set, rewrites reference frames instead
/// of comparing against them.
pub const UPDATE_ENV: &str = "GENEVA_UPDATE_GOLDEN";

/// The `golden.json` file that accompanies each scene.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseSpec {
    /// Times to render, in timeline time syntax.
    frames: Vec<String>,
    /// Tolerance override.
    #[serde(default)]
    tolerance: Tolerance,
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
}

impl CaseOutcome {
    /// Whether every frame passed.
    pub fn passed(&self) -> bool {
        self.frames.iter().all(|f| f.passed)
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
    let loaded = load(&read(&case_dir.join("scene.json"))?);
    let Some(comp) = loaded.composition else {
        let text = loaded
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<String>();
        return Err(GoldenError::Scene(text));
    };
    let update = std::env::var_os(UPDATE_ENV).is_some();
    // Assets resolve against the golden root so cases can share files such
    // as fonts.
    let root = case_dir.parent().unwrap_or(case_dir);
    let mut renderer = CpuRenderer::with_asset_root(root);
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
    Ok(CaseOutcome { name, frames })
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
