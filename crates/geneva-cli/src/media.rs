//! Media-dependent commands, with stubs when media support is compiled out.

use std::path::Path;

use anyhow::Result;
use geneva_render::{RenderError, Renderer};
use geneva_timeline::{AssetInfo, Composition, Diagnostic, Ratio};

/// Encoder settings the command line may override.
pub struct RenderOverrides {
    pub crf: Option<u8>,
    pub preset: Option<String>,
    pub no_audio: bool,
}

/// What a render produced.
pub struct RenderStats {
    pub frames: u64,
    pub seconds: f64,
}

/// Facts learned by opening the timeline's media assets.
#[derive(Default)]
pub struct ProbedAssets {
    durations: std::collections::HashMap<String, Ratio>,
    problems: Vec<Diagnostic>,
}

impl ProbedAssets {
    /// Diagnostics about assets that could not be opened.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.problems.clone()
    }
}

impl AssetInfo for ProbedAssets {
    fn duration(&self, asset_id: &str, _: &str) -> Option<Ratio> {
        self.durations.get(asset_id).copied()
    }
}

/// Opens every video and audio asset declared in `text` under `root`.
pub fn probe_assets(text: &str, root: &Path) -> ProbedAssets {
    let mut out = ProbedAssets::default();
    let Ok(timeline) = geneva_timeline::parse(text) else {
        return out;
    };
    for (id, asset) in &timeline.assets {
        let kind = asset.kind.or_else(|| {
            let ext = Path::new(&asset.src)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            geneva_timeline::schema::AssetKind::from_extension(ext)
        });
        if !matches!(
            kind,
            Some(
                geneva_timeline::schema::AssetKind::Video
                    | geneva_timeline::schema::AssetKind::Audio
            )
        ) {
            continue;
        }
        let path = root.join(&asset.src);
        match imp::probe(&path) {
            Ok(info) => {
                let duration = info
                    .video
                    .as_ref()
                    .and_then(|v| v.duration)
                    .or(info.duration)
                    .or_else(|| info.audio.as_ref().and_then(|a| a.duration));
                if let Some(d) = duration {
                    out.durations.insert(id.clone(), d);
                }
            }
            Err(e) => out.problems.push(
                Diagnostic::error(
                    "E501",
                    format!("/assets/{id}/src"),
                    format!("asset {id:?} could not be opened: {e}"),
                )
                .with_value(asset.src.clone())
                .with_help(format!("expected the file at {}", path.display())),
            ),
        }
    }
    out
}

pub use imp::{describe, probe, render, renderer};

#[cfg(feature = "libav")]
mod imp {
    use std::path::Path;
    use std::time::Instant;

    use anyhow::{Context, Result};
    use geneva_media::{AudioSettings, EncodeSettings, Encoder, MediaAssets, MediaInfo};
    use geneva_render::{CpuRenderer, RenderError, Renderer};
    use geneva_timeline::Composition;

    use super::{RenderOverrides, RenderStats};

    pub fn probe(path: &Path) -> Result<MediaInfo> {
        geneva_media::probe(path).with_context(|| format!("probing {}", path.display()))
    }

    pub fn renderer(root: std::path::PathBuf) -> CpuRenderer<MediaAssets> {
        CpuRenderer::new(MediaAssets::new(root))
    }

    /// Human-readable probe output.
    pub fn describe(path: &Path, info: &MediaInfo) -> String {
        use std::fmt::Write as _;
        let mut s = format!("{}\n  container: {}\n", path.display(), info.container);
        if let Some(d) = info.duration {
            let _ = writeln!(s, "  duration: {d}s");
        }
        if let Some(v) = &info.video {
            let _ = writeln!(
                s,
                "  video: {} {}×{} @ {} fps, {}{}",
                v.codec,
                v.width,
                v.height,
                geneva_timeline::Fps(v.fps),
                v.pixel_format,
                if v.has_alpha { " with alpha" } else { "" }
            );
            let tags = serde_json::to_value(v.color).unwrap_or_default();
            let tag = |name: &str| match tags.get(name).and_then(|t| t.as_str()) {
                Some(t) => format!("{name} {t}"),
                None => format!("{name} untagged"),
            };
            let _ = writeln!(
                s,
                "    color: {}, {}, {}, {}",
                tag("primaries"),
                tag("transfer"),
                tag("matrix"),
                tag("range"),
            );
            let (resolved, notes) = geneva_color::infer(v.color, v.width, v.height);
            if !notes.is_empty() {
                let _ = writeln!(
                    s,
                    "    assumed: {} ({})",
                    notes
                        .iter()
                        .map(|n| format!("{} {}", n.field, n.assumed))
                        .collect::<Vec<_>>()
                        .join(", "),
                    notes[0].reason
                );
            }
            if resolved.is_hdr() {
                s.push_str("    note: HDR material; output will be SDR\n");
            }
        }
        if let Some(a) = &info.audio {
            let _ = writeln!(
                s,
                "  audio: {} {} Hz, {} channel{}",
                a.codec,
                a.sample_rate,
                a.channels,
                if a.channels == 1 { "" } else { "s" }
            );
        }
        s
    }

    /// Renders every frame of `comp` to `output`, mixing audio unless disabled.
    pub fn render(
        comp: &Composition,
        root: &Path,
        output: &Path,
        overrides: &RenderOverrides,
        progress: bool,
    ) -> Result<RenderStats, RenderError> {
        let started = Instant::now();
        let container =
            geneva_media::container_for(output, comp.encode.as_ref().and_then(|e| e.container))
                .ok_or_else(|| {
                    RenderError::Asset {
            id: output.display().to_string(),
            reason:
                "unknown container; use .mp4, .mov, .mkv or .webm or set output.encode.container"
                    .to_owned(),
        }
                })?;
        let (default_video, default_audio) = geneva_media::default_codecs(container);
        let video = comp.encode.as_ref().and_then(|e| e.video.as_ref());
        let audio = comp.encode.as_ref().and_then(|e| e.audio.as_ref());
        let sample_rate = comp
            .audio_output
            .as_ref()
            .and_then(|a| a.sample_rate)
            .unwrap_or(48000);
        let settings = EncodeSettings {
            width: comp.width,
            height: comp.height,
            fps: comp.fps,
            container: Some(container),
            video_codec: video.and_then(|v| v.codec).unwrap_or(default_video),
            crf: overrides.crf.or_else(|| video.and_then(|v| v.crf)),
            preset: overrides
                .preset
                .clone()
                .or_else(|| video.and_then(|v| v.preset.clone())),
            color: comp.color,
            audio: if overrides.no_audio {
                None
            } else {
                Some(AudioSettings {
                    codec: audio.and_then(|a| a.codec).unwrap_or(default_audio),
                    bitrate_kbps: audio.and_then(|a| a.bitrate_kbps).unwrap_or(160),
                    sample_rate,
                })
            },
        };
        let has_audio = settings.audio.is_some();
        let media_err = |e: geneva_media::MediaError| RenderError::Asset {
            id: output.display().to_string(),
            reason: e.to_string(),
        };
        let mut encoder = Encoder::new(output, settings).map_err(media_err)?;
        let mut renderer = CpuRenderer::new(MediaAssets::new(root));
        let total = comp.frame_count();
        for n in 0..total {
            let frame = renderer.render_frame(comp, comp.frame_time(n))?;
            encoder.push_frame(&frame).map_err(media_err)?;
            if progress && (n % 30 == 29 || n + 1 == total) {
                eprint!("\rframe {}/{total}", n + 1);
            }
        }
        if progress {
            eprintln!();
        }
        if has_audio {
            let samples = geneva_media::mix::mix(comp, root, sample_rate).map_err(media_err)?;
            encoder.push_audio(&samples).map_err(media_err)?;
        }
        encoder.finish().map_err(media_err)?;
        Ok(RenderStats {
            frames: total,
            seconds: started.elapsed().as_secs_f64(),
        })
    }
}

#[cfg(not(feature = "libav"))]
mod imp {
    use std::path::Path;

    use anyhow::Result;
    use geneva_render::{CpuRenderer, FileAssets, RenderError};
    use geneva_timeline::Composition;

    use super::{RenderOverrides, RenderStats};

    fn unavailable() -> anyhow::Error {
        anyhow::anyhow!("{}", geneva_media::MediaError::Unavailable)
    }

    pub fn probe(_: &Path) -> Result<geneva_media::MediaInfo> {
        Err(unavailable())
    }

    pub fn renderer(root: std::path::PathBuf) -> CpuRenderer<FileAssets> {
        CpuRenderer::with_asset_root(root)
    }

    pub fn describe(_: &Path, _: &geneva_media::MediaInfo) -> String {
        String::new()
    }

    pub fn render(
        _: &Composition,
        _: &Path,
        output: &Path,
        _: &RenderOverrides,
        _: bool,
    ) -> Result<RenderStats, RenderError> {
        Err(RenderError::Asset {
            id: output.display().to_string(),
            reason: geneva_media::MediaError::Unavailable.to_string(),
        })
    }
}

#[allow(dead_code)]
fn _unused(_: &dyn Renderer, _: &Composition, _: RenderError, _: Result<()>) {}
