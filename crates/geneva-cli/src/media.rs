//! Media-dependent commands, with stubs when media support is compiled out.

use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use geneva_render::{RenderError, Renderer};
use geneva_timeline::{AssetInfo, Composition, Diagnostic, Ratio};

/// How a run reports the frames it has finished while it is still
/// running: a rewritten line for a person, one JSON object per line for
/// a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressFormat {
    /// One line on stderr, rewritten in place.
    Human,
    /// One JSON object per line on stderr.
    Json,
}

// Rendering is what reports progress, so in a build without media
// support (which cannot render) nothing below runs; the type still
// exists because the stubs take it.
/// A person's line is rewritten often enough to look live.
#[cfg_attr(not(feature = "media"), allow(dead_code))]
const HUMAN_INTERVAL: Duration = Duration::from_millis(250);
/// A program's lines come often enough to be useful without filling a
/// log; the first frame and the last one are always reported.
#[cfg_attr(not(feature = "media"), allow(dead_code))]
const JSON_INTERVAL: Duration = Duration::from_secs(1);

/// Reports the frames a long run has finished, at a readable pace.
///
/// Progress goes to stderr, so stdout stays the one JSON document the
/// report is. A run that copies its streams finishes without rendering a
/// frame and says nothing.
#[cfg_attr(not(feature = "media"), allow(dead_code))]
pub struct Progress {
    format: ProgressFormat,
    started: Instant,
    /// When the last line was written, if any.
    last: Cell<Option<Instant>>,
}

#[cfg_attr(not(feature = "media"), allow(dead_code))]
impl Progress {
    pub fn new(format: ProgressFormat) -> Self {
        Self {
            format,
            started: Instant::now(),
            last: Cell::new(None),
        }
    }

    /// Reports `done` of `total` frames, unless the last report is too
    /// recent. `fps` is the output's frame rate, which turns frames into
    /// the time reached in the video.
    pub fn frame(&self, done: u64, total: u64, fps: f64) {
        let interval = match self.format {
            ProgressFormat::Human => HUMAN_INTERVAL,
            ProgressFormat::Json => JSON_INTERVAL,
        };
        let now = Instant::now();
        if let Some(last) = self.last.get() {
            if now.duration_since(last) < interval {
                return;
            }
        }
        self.write(done, total, fps, now);
    }

    /// Reports the finished run, closing the line a person reads. A run
    /// that rendered no frames says nothing.
    pub fn finish(&self, done: u64, total: u64, fps: f64) {
        if done == 0 {
            return;
        }
        self.write(done, total, fps, Instant::now());
        if self.format == ProgressFormat::Human {
            eprintln!();
        }
    }

    fn write(&self, done: u64, total: u64, fps: f64, now: Instant) {
        self.last.set(Some(now));
        let elapsed = now.duration_since(self.started).as_secs_f64();
        match self.format {
            ProgressFormat::Human => eprint!("\rframe {done}/{total}"),
            ProgressFormat::Json => eprintln!("{}", progress_line(done, total, fps, elapsed)),
        }
    }
}

/// Rounds to two decimals, so a line carries no more precision than it
/// measured.
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// One progress line: what has been done, how far through the video it
/// is, how fast the work is going and how long is left at that rate.
fn progress_line(done: u64, total: u64, fps: f64, elapsed: f64) -> String {
    let rate = if elapsed > 0.0 {
        done as f64 / elapsed
    } else {
        0.0
    };
    // The estimate is the rate so far applied to the frames left. The
    // first moments measure the startup as much as the work, so there is
    // no estimate until there is half a second of it.
    let remaining = (elapsed >= 0.5 && rate > 0.0 && total > done)
        .then(|| round2((total - done) as f64 / rate));
    let mut doc = serde_json::json!({
        "event": "progress",
        "frames": done,
        "total": total,
        "seconds": round2(elapsed),
        "rate": round2(rate),
    });
    if fps > 0.0 {
        doc["time"] = round2(done as f64 / fps).into();
        doc["duration"] = round2(total as f64 / fps).into();
    }
    doc["remaining"] = remaining.into();
    doc.to_string()
}

#[cfg(test)]
mod progress_tests {
    use super::progress_line;

    #[test]
    fn a_progress_line_carries_the_count_the_time_and_the_estimate() {
        let doc: serde_json::Value =
            serde_json::from_str(&progress_line(540, 1800, 30.0, 8.0)).unwrap();
        assert_eq!(doc["event"], "progress");
        assert_eq!(doc["frames"], 540);
        assert_eq!(doc["total"], 1800);
        assert_eq!(doc["seconds"], 8.0);
        // 540 frames in 8 seconds is 67.5 a second; the 1260 left take 18.67.
        assert_eq!(doc["rate"], 67.5);
        assert_eq!(doc["remaining"], 18.67);
        // At 30 fps those frames are 18 seconds of a 60-second video.
        assert_eq!(doc["time"], 18.0);
        assert_eq!(doc["duration"], 60.0);
    }

    #[test]
    fn the_first_moments_are_too_short_to_estimate_from() {
        // A tenth of a second of work is mostly startup: reporting the
        // frames is honest, dividing by that rate is not.
        let doc: serde_json::Value =
            serde_json::from_str(&progress_line(1, 360, 30.0, 0.1)).unwrap();
        assert_eq!(doc["frames"], 1);
        assert_eq!(doc["total"], 360);
        assert!(doc["remaining"].is_null(), "{doc}");
    }

    #[test]
    fn the_last_line_has_nothing_left_to_estimate() {
        let doc: serde_json::Value =
            serde_json::from_str(&progress_line(1800, 1800, 30.0, 20.0)).unwrap();
        assert_eq!(doc["frames"], 1800);
        assert!(doc["remaining"].is_null());
    }

    #[test]
    fn a_rate_that_is_not_known_leaves_the_video_times_out() {
        let doc: serde_json::Value = serde_json::from_str(&progress_line(0, 10, 0.0, 0.0)).unwrap();
        assert!(doc.get("time").is_none());
        assert!(doc["remaining"].is_null());
        assert_eq!(doc["rate"], 0.0);
    }
}

/// Encoder settings the command line may override.
pub struct RenderOverrides {
    /// Read by the composited paths, which a build without `media` does
    /// not have.
    #[cfg_attr(not(feature = "media"), allow(dead_code))]
    pub renderer: RendererChoice,
    pub crf: Option<u8>,
    pub preset: Option<String>,
    pub no_audio: bool,
    pub exact: bool,
    /// The picture already is what the output asks for, so the quality
    /// settings in the document describe an encode that is not needed.
    /// Set when a `--for` target had only the sound to change. Only the
    /// copy paths read it, which a build without `media` does not have.
    #[cfg_attr(not(feature = "media"), allow(dead_code))]
    pub picture_as_is: bool,
}

/// Which renderer composites the frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum RendererChoice {
    /// The GPU where the machine has a hardware one, the CPU otherwise.
    #[default]
    Auto,
    /// The CPU reference renderer.
    Cpu,
    /// The GPU renderer, on whatever device there is, a software one
    /// included; the CPU when none can be opened, with a note saying so.
    Gpu,
}

/// The renderer `choice` comes to on this machine, reading assets under
/// `root`, and what the report says about it: nothing for the CPU when
/// it was the default; the device when the GPU is used; why not when it
/// was asked for and could not be. `auto` takes a hardware device only,
/// `gpu` a software one too (`GENEVA_GPU=software` forces the software
/// one for either).
#[cfg(feature = "media")]
pub fn choose_renderer(
    choice: RendererChoice,
    root: &std::path::Path,
    keep_hdr: bool,
) -> (Box<dyn geneva_media::PlaneRenderer>, Vec<String>) {
    use geneva_media::{FramePacker, MediaAssets, PlaneRenderer};
    use geneva_render::CpuRenderer;
    let assets = || MediaAssets::new(root).keep_hdr(keep_hdr);
    let cpu = |notes: Vec<String>| -> (Box<dyn PlaneRenderer>, Vec<String>) {
        (
            Box::new(FramePacker::new(CpuRenderer::new(assets()))),
            notes,
        )
    };
    if choice == RendererChoice::Cpu {
        return cpu(Vec::new());
    }
    #[cfg(feature = "gpu")]
    {
        use geneva_gpu::{Gpu, GpuRenderer, Preference};
        let wanted = if choice == RendererChoice::Gpu {
            Preference::Any
        } else {
            Preference::Hardware
        };
        match Gpu::probe(Preference::from_env(wanted)) {
            Ok(gpu) => {
                let line = gpu.report().line();
                (
                    Box::new(GpuRenderer::new(gpu, assets())),
                    vec![format!(
                        "GPU {line}: the frames were composited on the device"
                    )],
                )
            }
            Err(e) if choice == RendererChoice::Gpu => cpu(vec![format!(
                "no usable GPU ({e}); the frames were composited on the CPU"
            )]),
            Err(_) => cpu(Vec::new()),
        }
    }
    #[cfg(not(feature = "gpu"))]
    {
        if choice == RendererChoice::Gpu {
            cpu(vec![
                "this build has no GPU renderer; the frames were composited on the CPU".to_owned(),
            ])
        } else {
            cpu(Vec::new())
        }
    }
}

/// What a render produced.
/// How `render` produced its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Source streams were copied without decoding.
    Copy,
    /// The picture was copied without decoding and the sound was mixed,
    /// treated and encoded beside it.
    CopyPicture,
    /// Decoded frames went straight to the encoder without compositing.
    Direct,
    /// Source packets were copied where nothing changed and the frames
    /// around the cuts and under the overlays were encoded into the same
    /// stream.
    Smart,
    /// Frames were composited by the renderer.
    Render,
}

impl RenderMode {
    /// The name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::CopyPicture => "copy-picture",
            Self::Direct => "direct",
            Self::Smart => "smart",
            Self::Render => "render",
        }
    }
}

pub struct RenderStats {
    /// Frames encoded; zero when streams were copied.
    pub frames: u64,
    /// Output duration in seconds.
    pub duration: Ratio,
    /// How the output was produced.
    pub mode: RenderMode,
    /// Remarks for the user, such as cuts moved to keyframes.
    pub notes: Vec<String>,
    pub seconds: f64,
}

/// One file written by a multi-output render.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OutputStats {
    /// The entry's name in `outputs`.
    pub name: String,
    /// The kind, as its JSON spelling.
    pub kind: &'static str,
    /// Where it was written.
    pub path: std::path::PathBuf,
    /// Size on disk.
    pub bytes: u64,
    /// How the file was made: "copy" or "render".
    pub mode: &'static str,
    /// The media type to serve the file as.
    pub content_type: &'static str,
    /// Picture size, when the file has one (a sprite sheet: one tile).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// The media type a file should be served as, from its extension, so an
/// uploader can set it right (players refuse a `.vtt` served as octets).
pub fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("mxf") => "application/mxf",
        Some("m4a") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("ogg") => "audio/ogg",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("vtt") => "text/vtt",
        Some("srt") => "application/x-subrip",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

/// Writes an sRGB picture as PNG or, for any other extension, JPEG.
pub fn write_picture(rgba: &[u8], width: u32, height: u32, path: &Path) -> Result<(), RenderError> {
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| {
        RenderError::Asset {
            id: path.display().to_string(),
            reason: "picture buffer does not match its size".to_owned(),
        }
    })?;
    let io = |e: image::ImageError| RenderError::Asset {
        id: path.display().to_string(),
        reason: e.to_string(),
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if ext == "png" {
        img.save(path).map_err(io)
    } else {
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let file = std::fs::File::create(path).map_err(|e| RenderError::Asset {
            id: path.display().to_string(),
            reason: e.to_string(),
        })?;
        let mut writer = std::io::BufWriter::new(file);
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 88);
        enc.encode_image(&rgb).map_err(io)
    }
}

/// Scales an sRGB picture to `width`×`height`.
pub fn scale_picture(rgba: &[u8], width: u32, height: u32, to: (u32, u32)) -> Vec<u8> {
    if (width, height) == to {
        return rgba.to_vec();
    }
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .expect("picture buffer matches its size");
    image::imageops::resize(&img, to.0, to.1, image::imageops::FilterType::Triangle).into_raw()
}

/// Facts learned by opening the timeline's media assets.
#[derive(Default)]
pub struct ProbedAssets {
    /// The asset root, so that a path written inside markup can be read
    /// and checked the way a browser would resolve it.
    root: std::path::PathBuf,
    durations: std::collections::HashMap<String, Ratio>,
    /// Markup read from html assets, so the resolver checks it before
    /// anything is rendered.
    texts: std::collections::HashMap<String, String>,
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

    fn text(&self, asset_id: &str, _: &str) -> Option<String> {
        self.texts.get(asset_id).cloned()
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(path)).ok()
    }

    fn exists(&self, path: &str) -> Option<bool> {
        Some(self.root.join(path).is_file())
    }
}

/// Opens every video and audio asset declared in `text` under `root`.
/// `measure` decodes every audio track to report what it measures
/// (N310 and the warnings around it), which `validate --probe` asks for
/// and a render does not, since the render reports what it applies.
pub fn probe_assets(text: &str, root: &Path, measure: bool) -> ProbedAssets {
    let mut out = ProbedAssets {
        root: root.to_path_buf(),
        ..ProbedAssets::default()
    };
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
        let path = root.join(&asset.src);
        // Text a document draws rather than muxes: markup, and captions,
        // which a .srt or .vtt infers as a subtitle.
        if matches!(
            kind,
            Some(
                geneva_timeline::schema::AssetKind::Html
                    | geneva_timeline::schema::AssetKind::Captions
                    | geneva_timeline::schema::AssetKind::Subtitle
            )
        ) {
            match std::fs::read_to_string(&path) {
                Ok(markup) => {
                    out.texts.insert(id.clone(), markup);
                }
                Err(e) => out.problems.push(
                    Diagnostic::error(
                        "E501",
                        format!("/assets/{id}/src"),
                        format!("asset {id:?} could not be read: {e}"),
                    )
                    .with_value(asset.src.clone())
                    .with_help(format!("expected the file at {}", path.display())),
                ),
            }
            continue;
        }
        if !matches!(
            kind,
            Some(
                geneva_timeline::schema::AssetKind::Video
                    | geneva_timeline::schema::AssetKind::Audio
            )
        ) {
            continue;
        }
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
                if measure && info.audio.is_some() {
                    match imp::measure_audio(&path) {
                        Ok(Some(report)) => {
                            audio_diagnostics(id, &report, &timeline.output, &mut out.problems);
                        }
                        Ok(None) => {}
                        Err(e) => out.problems.push(
                            Diagnostic::warning(
                                "W316",
                                format!("/assets/{id}/src"),
                                format!("the audio of {id:?} could not be measured: {e}"),
                            )
                            .with_value(asset.src.clone()),
                        ),
                    }
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

/// What a track measures, as a note, with a warning for each thing a
/// render cannot put right or that the document has not asked to.
fn audio_diagnostics(
    id: &str,
    r: &geneva_audio::Report,
    output: &geneva_timeline::schema::Output,
    out: &mut Vec<Diagnostic>,
) {
    use std::fmt::Write as _;
    let path = format!("/assets/{id}/src");
    let audio = output.audio.as_ref();
    let hygiene = audio.and_then(|a| a.hygiene).unwrap_or(false);
    if r.silent {
        out.push(Diagnostic::warning(
            "W314",
            path.clone(),
            format!("the audio of {id:?} is silent: nothing above -60 dBTP"),
        ));
        return;
    }
    let mut line = format!(
        "audio of {id:?}: {} integrated, true peak {:.1} dBTP",
        r.lufs
            .map_or("no loudness".to_owned(), |l| format!("{l:.1} LUFS")),
        r.true_peak_dbtp
    );
    if let (Some(floor), Some(snr)) = (r.noise_floor_dbfs, r.snr_db) {
        let _ = write!(
            line,
            ", noise floor {floor:.0} dBFS ({snr:.0} dB under the signal)"
        );
    }
    let _ = write!(
        line,
        ", {} Hz {}",
        r.rate,
        if r.channels == 1 { "mono" } else { "stereo" }
    );
    if let (Some(measured), Some(target)) = (r.lufs, audio.and_then(|a| a.loudness.as_ref())) {
        let diff = target.target_lufs - measured;
        let _ = write!(
            line,
            "; {:.1} LU {} the {} LUFS target, which the render applies",
            diff.abs(),
            if diff >= 0.0 { "under" } else { "over" },
            target.target_lufs
        );
    }
    out.push(Diagnostic::note("N310", path.clone(), line));
    if r.clipped_runs > 0 {
        out.push(
            Diagnostic::warning(
                "W311",
                path.clone(),
                format!(
                    "the audio of {id:?} is clipped: {} run{} of samples at full scale, the longest {} samples, the first at {:.2} s",
                    r.clipped_runs,
                    if r.clipped_runs == 1 { "" } else { "s" },
                    r.longest_clip,
                    r.first_clip_secs.unwrap_or(0.0)
                ),
            )
            .with_help("the recording is distorted where it clips; a loudness target does not undo that"),
        );
    }
    if r.dc_offset > 0.01 {
        let d = Diagnostic::warning(
            "W312",
            path.clone(),
            format!(
                "the audio of {id:?} carries a DC offset of {:.1}% of full scale",
                r.dc_offset * 100.0
            ),
        );
        out.push(if hygiene {
            d.with_help("output.audio.hygiene is on and takes it out")
        } else {
            d.with_help("set output.audio.hygiene to take it out")
        });
    }
    if let Some(hum) = &r.hum {
        let d = Diagnostic::warning(
            "W313",
            path.clone(),
            format!(
                "the audio of {id:?} hums at {} Hz ({:.0} dBFS) with {} harmonic{}",
                hum.base_hz,
                hum.level_dbfs,
                hum.harmonics.len() - 1,
                if hum.harmonics.len() == 2 { "" } else { "s" }
            ),
        );
        out.push(if hygiene {
            d.with_help("output.audio.hygiene is on and notches it")
        } else {
            d.with_help("set output.audio.hygiene to notch it")
        });
    }
    if r.dual_mono {
        out.push(Diagnostic::note(
            "N315",
            path,
            format!(
                "the audio of {id:?} is two channels of one signal; it reads 3 dB louder than the same recording in one channel"
            ),
        ));
    }
}

pub use imp::{
    copy_sources, describe, measure_audio, probe, read_subtitles, render, render_outputs, renderer,
};

#[cfg(feature = "media")]
mod imp {
    use std::fmt::Write as _;
    use std::path::Path;
    use std::time::Instant;

    use anyhow::{Context, Result};
    use geneva_media::mix::TreatmentReport;
    use geneva_media::{
        AudioSettings, EncodeSettings, Encoder, MediaAssets, MediaInfo, VideoSettings,
    };
    use geneva_render::{CpuRenderer, RenderError, Renderer};
    use geneva_timeline::{Composition, Ratio};

    use super::{RenderMode, RenderOverrides, RenderStats};

    /// The subtitle tracks of a composition as streams to write.
    fn subtitle_settings(
        comp: &Composition,
        root: &Path,
    ) -> Result<Vec<geneva_media::SubtitleSettings>, RenderError> {
        let mut subtitles = Vec::new();
        for track in &comp.subtitles {
            let Some(asset) = comp.assets.get(&track.asset) else {
                continue;
            };
            let path = root.join(&asset.src);
            let text = std::fs::read_to_string(&path).map_err(|e| RenderError::Asset {
                id: track.asset.clone(),
                reason: format!("{}: {e}", path.display()),
            })?;
            let mut cues =
                geneva_media::subtitles::parse(&text).map_err(|e| RenderError::Asset {
                    id: track.asset.clone(),
                    reason: e.to_string(),
                })?;
            geneva_media::subtitles::shift(&mut cues, track.offset);
            subtitles.push(geneva_media::SubtitleSettings {
                language: track.language.clone(),
                title: track.title.clone(),
                cues,
            });
        }
        Ok(subtitles)
    }

    /// A rough luminance of the frame on a coarse grid, and how much it
    /// moved since `previous`: what the poster picker looks at.
    fn frame_gist(frame: &geneva_render::Frame, previous: Option<&[f32]>) -> (Vec<f32>, f32, f32) {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let step = (w.max(h) / 64).max(1);
        let px = frame.pixels();
        let mut grid = Vec::with_capacity((w / step + 1) * (h / step + 1));
        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                let p = px[y * w + x];
                grid.push(0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b);
                x += step;
            }
            y += step;
        }
        let mean = grid.iter().sum::<f32>() / grid.len().max(1) as f32;
        let motion = previous.filter(|p| p.len() == grid.len()).map_or(1.0, |p| {
            grid.iter().zip(p).map(|(a, b)| (a - b).abs()).sum::<f32>() / grid.len() as f32
        });
        (grid, mean, motion)
    }

    /// Writes an sRGB picture as JPEG or PNG by the path's extension.
    /// A WebVTT timestamp.
    fn vtt_time(secs: f64) -> String {
        let ms = (secs * 1000.0).round().max(0.0) as u64;
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            ms / 3_600_000,
            (ms / 60_000) % 60,
            (ms / 1000) % 60,
            ms % 1000
        )
    }

    /// Renders every entry of `comp.outputs` into `dir` from one pass
    /// over the composition: each frame is composited once, scaled to
    /// each video rendition, sampled for the poster and the sprite
    /// sheet; the audio is mixed once per file that takes it. A video
    /// rendition that is a plain copy of a source (the canvas size, no
    /// change, a container that holds the streams) is copied instead.
    /// What each frame has to become for one video entry.
    struct VideoSink {
        name: String,
        path: std::path::PathBuf,
        encoder: Option<Encoder>,
        tags: geneva_color::ResolvedTags,
        format: geneva_media::convert::PlaneFormat,
        scaler: Option<geneva_media::PlaneScaler>,
        has_audio: bool,
        width: u32,
        height: u32,
    }

    /// An audio-only entry.
    struct AudioSink {
        name: String,
        path: std::path::PathBuf,
        encoder: Option<Encoder>,
        sample_rate: u32,
    }

    /// The source files a stream copy of `comp` into `output` would take
    /// its video from, when the copy planner allows one; `None` when the
    /// composition needs a render.
    pub fn copy_sources(
        comp: &Composition,
        root: &Path,
        output: &Path,
    ) -> Result<Option<Vec<std::path::PathBuf>>> {
        let Some(container) = geneva_media::container_for(output, None) else {
            return Ok(None);
        };
        match geneva_media::plan_stream_copy_explained(comp, root, container, None)? {
            Ok(plan) if !plan.segments.is_empty() => {
                let mut paths: Vec<std::path::PathBuf> = Vec::new();
                for seg in &plan.segments {
                    if !paths.contains(&seg.path) {
                        paths.push(seg.path.clone());
                    }
                }
                Ok(Some(paths))
            }
            _ => Ok(None),
        }
    }

    pub fn render_outputs(
        comp: &Composition,
        root: &Path,
        dir: &Path,
        overrides: &RenderOverrides,
        progress: &super::Progress,
    ) -> Result<(Vec<super::OutputStats>, RenderStats), RenderError> {
        use geneva_timeline::schema::OutputKind;
        let started = Instant::now();
        std::fs::create_dir_all(dir).map_err(|e| RenderError::Asset {
            id: dir.display().to_string(),
            reason: e.to_string(),
        })?;
        let err_at = |path: &Path, e: geneva_media::MediaError| RenderError::Asset {
            id: path.display().to_string(),
            reason: e.to_string(),
        };
        let mut notes = Vec::new();
        let mut stats: Vec<super::OutputStats> = Vec::new();
        let subtitles = subtitle_settings(comp, root)?;
        let hdr_metadata = hdr_metadata_for(comp, root);

        let mut video_sinks: Vec<VideoSink> = Vec::new();
        let mut audio_sinks: Vec<AudioSink> = Vec::new();
        let mut poster: Option<(&geneva_timeline::ResolvedOutput, std::path::PathBuf)> = None;
        let mut sprites: Option<(&geneva_timeline::ResolvedOutput, std::path::PathBuf)> = None;

        for o in &comp.outputs {
            let path = dir.join(&o.path);
            match o.kind {
                OutputKind::Poster => poster = Some((o, path)),
                OutputKind::Sprites => sprites = Some((o, path)),
                OutputKind::Audio => {
                    let container = geneva_media::container_for(
                        &path,
                        o.encode.as_ref().and_then(|e| e.container),
                    )
                    .ok_or_else(|| RenderError::Asset {
                        id: path.display().to_string(),
                        reason: "unknown audio container".to_owned(),
                    })?;
                    let (_, default_audio) = geneva_media::default_codecs(container);
                    let enc = o.encode.as_ref().and_then(|e| e.audio.as_ref());
                    let codec = enc.and_then(|a| a.codec).unwrap_or(default_audio);
                    let sample_rate = geneva_media::audio_sample_rate_for(
                        codec,
                        Some(container),
                        o.audio
                            .as_ref()
                            .and_then(|a| a.sample_rate)
                            .unwrap_or(48000),
                    );
                    let settings = EncodeSettings {
                        video: None,
                        container: Some(container),
                        audio: Some(AudioSettings {
                            codec,
                            bitrate_kbps: enc.and_then(|a| a.bitrate_kbps).unwrap_or(160),
                            sample_rate,
                            channels: o
                                .audio
                                .as_ref()
                                .and_then(|a| a.channels)
                                .unwrap_or(2)
                                .clamp(1, 2),
                            loudness: o.audio.as_ref().and_then(|a| a.loudness.clone()),
                            hygiene: o.audio.as_ref().and_then(|a| a.hygiene).unwrap_or(false),
                            denoise: o.audio.as_ref().and_then(|a| a.denoise).unwrap_or(false),
                        }),
                        subtitles: Vec::new(),
                        fast_start: true,
                        copied_audio: None,
                    };
                    let encoder = Encoder::new(&path, settings).map_err(|e| err_at(&path, e))?;
                    audio_sinks.push(AudioSink {
                        name: o.name.clone(),
                        path,
                        encoder: Some(encoder),
                        sample_rate,
                    });
                }
                OutputKind::Video => {
                    let container = geneva_media::container_for(
                        &path,
                        o.encode.as_ref().and_then(|e| e.container),
                    )
                    .ok_or_else(|| RenderError::Asset {
                        id: path.display().to_string(),
                        reason: "unknown container".to_owned(),
                    })?;
                    let video = o.encode.as_ref().and_then(|e| e.video.as_ref());
                    let audio = o.encode.as_ref().and_then(|e| e.audio.as_ref());
                    let fast_start = o.encode.as_ref().and_then(|e| e.fast_start).unwrap_or(true);
                    // A copy, when the entry asks for nothing the source
                    // does not already have.
                    let wants_encode = !overrides.picture_as_is
                        && (overrides.crf.is_some()
                            || overrides.preset.is_some()
                            || video.is_some_and(|v| {
                                v.crf.is_some()
                                    || v.preset.is_some()
                                    || v.tune.is_some()
                                    || v.fixed_keyframes == Some(true)
                            }));
                    let canvas_size = (o.width, o.height) == (comp.width, comp.height);
                    let (default_video, default_audio) = geneva_media::default_codecs(container);
                    let audio_codec = audio.and_then(|a| a.codec).unwrap_or(default_audio);
                    let sample_rate = geneva_media::audio_sample_rate_for(
                        audio_codec,
                        Some(container),
                        o.audio
                            .as_ref()
                            .and_then(|a| a.sample_rate)
                            .unwrap_or(48000),
                    );
                    let audio_settings = if overrides.no_audio || container.is_video_only() {
                        None
                    } else {
                        Some(AudioSettings {
                            codec: audio_codec,
                            bitrate_kbps: audio.and_then(|a| a.bitrate_kbps).unwrap_or(160),
                            sample_rate,
                            channels: o
                                .audio
                                .as_ref()
                                .and_then(|a| a.channels)
                                .unwrap_or(2)
                                .clamp(1, 2),
                            loudness: o.audio.as_ref().and_then(|a| a.loudness.clone()),
                            hygiene: o.audio.as_ref().and_then(|a| a.hygiene).unwrap_or(false),
                            denoise: o.audio.as_ref().and_then(|a| a.denoise).unwrap_or(false),
                        })
                    };
                    // A treated mix is a change no copied audio track can
                    // carry, but it says nothing about the picture: the
                    // copy still stands, with the sound encoded beside it.
                    let mix_audio = audio_treated(o.audio.as_ref())
                        .then(|| audio_settings.clone())
                        .flatten();
                    if canvas_size && !wants_encode && !overrides.exact {
                        let plan = geneva_media::plan_stream_copy_explained(
                            comp,
                            root,
                            container,
                            video.and_then(|v| v.codec),
                        )
                        .map_err(|e| err_at(&path, e))?;
                        match plan {
                            Ok(mut plan) => {
                                if overrides.no_audio {
                                    plan.audio.clear();
                                }
                                let report = match &mix_audio {
                                    Some(a) => {
                                        let mut mixer =
                                            geneva_media::mix::Mixer::for_output(comp, root, a);
                                        let out = geneva_media::stream_copy_mixing_audio(
                                            &plan,
                                            &path,
                                            &subtitles,
                                            fast_start,
                                            a,
                                            &mut |frames| mixer.next_block(frames),
                                        )
                                        .map_err(|e| err_at(&path, e))?;
                                        if let Some(r) = mixer.report() {
                                            notes.extend(
                                                treatment_notes(r)
                                                    .into_iter()
                                                    .map(|n| format!("{}: {n}", o.name)),
                                            );
                                        }
                                        out
                                    }
                                    None => geneva_media::stream_copy(
                                        &plan, &path, &subtitles, fast_start,
                                    )
                                    .map_err(|e| err_at(&path, e))?,
                                };
                                notes.push(format!("{}: {}", o.name, plan.reason));
                                notes.extend(
                                    report
                                        .notes()
                                        .into_iter()
                                        .map(|n| format!("{}: {n}", o.name)),
                                );
                                stats.push(super::OutputStats {
                                    name: o.name.clone(),
                                    kind: "video",
                                    bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                                    content_type: super::content_type_for(&path),
                                    path,
                                    mode: if mix_audio.is_some() {
                                        "copy-picture"
                                    } else {
                                        "copy"
                                    },
                                    width: Some(o.width),
                                    height: Some(o.height),
                                });
                                continue;
                            }
                            Err(geneva_media::CopyRefusal(Some(reason))) => {
                                notes.push(format!(
                                    "{}: not copied without re-encoding: {reason}",
                                    o.name
                                ));
                            }
                            Err(_) => {}
                        }
                    }
                    let settings = EncodeSettings {
                        video: Some(VideoSettings {
                            width: o.width,
                            height: o.height,
                            fps: comp.fps,
                            codec: video.and_then(|v| v.codec).unwrap_or(default_video),
                            crf: overrides.crf.or_else(|| video.and_then(|v| v.crf)),
                            preset: overrides
                                .preset
                                .clone()
                                .or_else(|| video.and_then(|v| v.preset.clone())),
                            hardware: video
                                .and_then(|v| v.hardware)
                                .unwrap_or(geneva_timeline::schema::HardwarePolicy::Auto),
                            color: comp.color,
                            profile: video.and_then(|v| v.profile),
                            keyframe_interval: video.and_then(|v| v.keyframe_interval),
                            max_bitrate_kbps: video.and_then(|v| v.max_bitrate_kbps),
                            bitrate_kbps: video.and_then(|v| v.bitrate_kbps),
                            level: video.and_then(|v| v.level.clone()),
                            tune: video.and_then(|v| v.tune),
                            fixed_keyframes: video.and_then(|v| v.fixed_keyframes).unwrap_or(false),
                            hdr_metadata: hdr_metadata.clone(),
                            threads: None,
                            stitch: None,
                        }),
                        container: Some(container),
                        audio: audio_settings,
                        subtitles: subtitles.clone(),
                        fast_start,
                        copied_audio: None,
                    };
                    let has_audio = settings.audio.is_some();
                    let encoder = Encoder::new(&path, settings).map_err(|e| err_at(&path, e))?;
                    if let Some(note) = encoder.video_encoder_note() {
                        notes.push(format!("{}: {note}", o.name));
                    }
                    let (tags, format) = encoder.video_format().map_err(|e| err_at(&path, e))?;
                    let scaler = if canvas_size {
                        None
                    } else {
                        Some(
                            geneva_media::PlaneScaler::new(
                                format,
                                (comp.width, comp.height),
                                (o.width, o.height),
                            )
                            .map_err(|e| err_at(&path, e))?,
                        )
                    };
                    video_sinks.push(VideoSink {
                        name: o.name.clone(),
                        path,
                        encoder: Some(encoder),
                        tags,
                        format,
                        scaler,
                        has_audio,
                        width: o.width,
                        height: o.height,
                    });
                }
            }
        }

        let total = comp.frame_count();
        let needs_frames = !video_sinks.is_empty() || poster.is_some() || sprites.is_some();
        // The sprite grid.
        let sprite_plan = sprites.as_ref().map(|(o, _)| {
            let duration = comp.duration.to_f64();
            let every = o
                .every
                .map_or_else(|| (duration / 100.0).ceil().max(1.0), Ratio::to_f64);
            let count = ((duration / every).ceil() as u32).max(1);
            let columns = o.columns.unwrap_or(10).max(1).min(count);
            let rows = count.div_ceil(columns);
            (every, count, columns, rows)
        });
        let mut sheet: Option<image::RgbaImage> = sprite_plan.map(|(_, _, cols, rows)| {
            let (o, _) = sprites.as_ref().expect("planned from the entry");
            image::RgbaImage::new(cols * o.width, rows * o.height)
        });
        let mut next_tile = 0u32;
        // The poster: an explicit frame, or the first clear one.
        let poster_at: Option<u64> = poster.as_ref().and_then(|(o, _)| {
            o.at.map(|t| ((t * comp.fps).floor().max(0) as u64).min(total.saturating_sub(1)))
        });
        let poster_earliest = (total / 20).max(20).min(total.saturating_sub(1));
        let mut poster_pick: Option<Vec<u8>> = None;
        let mut poster_fallback: Option<Vec<u8>> = None;
        let mut previous_gist: Option<Vec<f32>> = None;

        let (mut renderer, renderer_notes) =
            super::choose_renderer(overrides.renderer, root, comp.color.is_hdr());
        notes.extend(renderer_notes);
        let mut frame = geneva_render::Frame::new(0, 0, geneva_color::Color::BLACK);
        let mut render_error: Option<RenderError> = None;
        let mut done = 0u64;
        // Frames the pass will composite; a document of pictures alone
        // stops as soon as it has them, so this is the ceiling.
        let frames = if needs_frames { total } else { 0 };
        // With no video to write there is no count to work towards: the
        // pass ends at the frame the last picture wanted.
        let pictures_only = video_sinks.is_empty();

        std::thread::scope(|scope| {
            // One encoder thread per file, fed through a channel; spare
            // plane buffers come back to be filled again.
            let mut video_feeds: Vec<Feed<'_>> = Vec::new();
            for sink in &mut video_sinks {
                let encoder = sink.encoder.take().expect("encoder not yet started");
                video_feeds.push(spawn_feed(
                    scope,
                    &sink.name,
                    encoder,
                    sink.has_audio.then_some(48000),
                    comp,
                    root,
                ));
            }
            let mut audio_feeds: Vec<Feed<'_>> = Vec::new();
            for sink in &mut audio_sinks {
                let encoder = sink.encoder.take().expect("encoder not yet started");
                audio_feeds.push(spawn_feed(
                    scope,
                    &sink.name,
                    encoder,
                    Some(sink.sample_rate),
                    comp,
                    root,
                ));
            }
            // Canvas planes, one set per distinct layout the renditions want.
            let mut canvases: Vec<(
                geneva_color::ResolvedTags,
                geneva_media::convert::PlaneFormat,
                geneva_media::convert::Planes,
            )> = Vec::new();
            for sink in &video_sinks {
                if !canvases
                    .iter()
                    .any(|(t, f, _)| *t == sink.tags && *f == sink.format)
                {
                    canvases.push((
                        sink.tags,
                        sink.format,
                        geneva_media::convert::Planes::new(sink.format, comp.width, comp.height),
                    ));
                }
            }

            // When the canvas is one video filling the frame, its frames
            // come from the decoder already scaled and packed, and the
            // compositor is only needed for the frames a picture wants.
            let mut directs: Vec<Option<geneva_media::DirectSource>> = Vec::new();
            if !pictures_only {
                for (tags, format, _) in &canvases {
                    directs.push(
                        geneva_media::DirectSource::open(comp, root, *format, *tags)
                            .ok()
                            .flatten(),
                    );
                }
            }
            let direct_mode = !directs.is_empty() && directs.iter().all(Option::is_some);
            if direct_mode {
                if let Some(reason) = directs[0].as_ref().map(geneva_media::DirectSource::reason) {
                    notes.push(reason);
                }
            }
            let mut canvas_pool = geneva_media::convert::PlanePool::default();
            let layouts: Vec<(
                geneva_color::ResolvedTags,
                geneva_media::convert::PlaneFormat,
            )> = canvases.iter().map(|(t, f, _)| (*t, *f)).collect();

            // With no video to write, only the frames the pictures need
            // are composited, and the loop ends once they have them.

            'frames: for n in 0..frames {
                let t = comp.frame_time(n);
                // A poster that names its time wants one frame; one that
                // does not is still looking until it has picked, and the
                // sheet wants a frame whenever a tile falls due.
                let poster_done = poster.is_none() || poster_pick.is_some();
                let tiles_done = sprite_plan.is_none_or(|(_, count, _, _)| next_tile >= count);
                let poster_wants = !poster_done
                    && match poster_at {
                        Some(at) => at == n,
                        None => true,
                    };
                let tile_wants = !tiles_done
                    && sprite_plan.is_some_and(|(every, _, _, _)| {
                        t.to_f64() + 1e-9 >= f64::from(next_tile) * every
                    });
                if pictures_only {
                    if poster_done && tiles_done {
                        break;
                    }
                    let looking = poster_wants
                        && poster_at.is_none()
                        && !(n + 1 >= poster_earliest || n == total / 10 || n + 1 == total);
                    if (!poster_wants || looking) && !tile_wants {
                        continue;
                    }
                }
                if direct_mode {
                    for (planes, source) in canvases.iter_mut().zip(directs.iter_mut()) {
                        let source = source.as_mut().expect("direct mode checked every canvas");
                        match source.frame_with(t, &mut canvas_pool) {
                            Ok(fresh) => {
                                let spent = std::mem::replace(&mut planes.2, fresh);
                                canvas_pool.give(spent);
                            }
                            Err(e) => {
                                render_error = Some(err_at(dir, e));
                                break 'frames;
                            }
                        }
                    }
                }
                // The compositor runs for every frame unless the decoder
                // is feeding the encoders, in which case only the frames a
                // picture takes are composited.
                let composited = !direct_mode || poster_wants || tile_wants;
                if composited {
                    // The frame packed into every canvas, and kept whole
                    // when a picture wants it; the next frame is started
                    // first, so a renderer that can draws it meanwhile.
                    let mut targets: Vec<geneva_media::PlaneTarget<'_>> = if direct_mode {
                        Vec::new()
                    } else {
                        canvases
                            .iter_mut()
                            .map(|(tags, format, planes)| geneva_media::PlaneTarget {
                                tags: *tags,
                                format: *format,
                                planes,
                            })
                            .collect()
                    };
                    if !direct_mode && n + 1 < frames {
                        renderer.prepare_planes(comp, comp.frame_time(n + 1), &layouts);
                    }
                    let whole = (poster_wants || tile_wants).then_some(&mut frame);
                    if let Err(e) = renderer.render_planes(comp, t, &mut targets, whole) {
                        render_error = Some(e);
                        break;
                    }
                }
                for (sink, feed) in video_sinks.iter_mut().zip(video_feeds.iter_mut()) {
                    while let Ok(spare) = feed.spare_rx.try_recv() {
                        feed.pool.give(spare);
                    }
                    let canvas = &canvases
                        .iter()
                        .find(|(t, f, _)| *t == sink.tags && *f == sink.format)
                        .expect("canvas laid out for the sink")
                        .2;
                    let planes = match sink.scaler.as_mut() {
                        Some(scaler) => match scaler.scale(canvas, &mut feed.pool) {
                            Ok(p) => p,
                            Err(e) => {
                                render_error = Some(err_at(&sink.path, e));
                                break 'frames;
                            }
                        },
                        None => {
                            let mut p = feed.pool.take(sink.format, comp.width, comp.height);
                            for (dst, src) in p.planes.iter_mut().zip(&canvas.planes) {
                                dst.data.copy_from_slice(&src.data);
                            }
                            p
                        }
                    };
                    if feed.tx.send(Msg::Planes(planes)).is_err() {
                        break 'frames;
                    }
                }
                // Pictures from the composited frame.
                if poster.is_some() && composited {
                    match poster_at {
                        Some(at) if at == n => poster_pick = Some(frame.to_rgba8()),
                        None if poster_pick.is_none() => {
                            let (gist, mean, motion) = frame_gist(&frame, previous_gist.as_deref());
                            if n == total / 10 {
                                poster_fallback = Some(frame.to_rgba8());
                            }
                            if n >= poster_earliest && mean > 0.05 && motion > 0.02 {
                                poster_pick = Some(frame.to_rgba8());
                            } else if n + 1 == total && poster_fallback.is_none() {
                                poster_fallback = Some(frame.to_rgba8());
                            }
                            previous_gist = Some(gist);
                        }
                        Some(_) | None => {}
                    }
                }
                if let (Some((o, _)), Some((every, count, cols, _)), Some(sheet)) =
                    (sprites.as_ref(), sprite_plan, sheet.as_mut())
                    && composited
                {
                    let tile_time = f64::from(next_tile) * every;
                    if next_tile < count && t.to_f64() + 1e-9 >= tile_time {
                        let tile = super::scale_picture(
                            &frame.to_rgba8(),
                            frame.width(),
                            frame.height(),
                            (o.width, o.height),
                        );
                        let tile =
                            image::RgbaImage::from_raw(o.width, o.height, tile).expect("tile size");
                        let (x, y) = ((next_tile % cols) * o.width, (next_tile / cols) * o.height);
                        image::imageops::replace(sheet, &tile, i64::from(x), i64::from(y));
                        next_tile += 1;
                    }
                }
                done += 1;
                if !pictures_only {
                    progress.frame(done, frames, comp.fps.to_f64());
                }
            }
            for feed in video_feeds.into_iter().chain(audio_feeds) {
                drop(feed.tx);
                if let Some(a) = feed.audio {
                    match a.join().unwrap_or_else(|p| std::panic::resume_unwind(p)) {
                        Err(e) => {
                            render_error.get_or_insert(RenderError::Asset {
                                id: dir.display().to_string(),
                                reason: e.to_string(),
                            });
                        }
                        Ok(Some(report)) => {
                            for note in treatment_notes(&report) {
                                notes.push(format!("{}: {note}", feed.name));
                            }
                        }
                        Ok(None) => {}
                    }
                }
                match feed
                    .worker
                    .join()
                    .unwrap_or_else(|p| std::panic::resume_unwind(p))
                {
                    Ok(encoder) => {
                        if let Err(e) = encoder.finish() {
                            render_error.get_or_insert(RenderError::Asset {
                                id: dir.display().to_string(),
                                reason: e.to_string(),
                            });
                        }
                    }
                    Err(e) => {
                        render_error.get_or_insert(RenderError::Asset {
                            id: dir.display().to_string(),
                            reason: e.to_string(),
                        });
                    }
                }
            }
        });
        progress.finish(
            done,
            if pictures_only { done } else { frames },
            comp.fps.to_f64(),
        );
        if let Some(e) = render_error {
            return Err(e);
        }
        for sink in video_sinks {
            stats.push(super::OutputStats {
                name: sink.name,
                kind: "video",
                bytes: std::fs::metadata(&sink.path).map(|m| m.len()).unwrap_or(0),
                content_type: super::content_type_for(&sink.path),
                path: sink.path,
                mode: "render",
                width: Some(sink.width),
                height: Some(sink.height),
            });
        }
        for sink in audio_sinks {
            stats.push(super::OutputStats {
                name: sink.name,
                kind: "audio",
                bytes: std::fs::metadata(&sink.path).map(|m| m.len()).unwrap_or(0),
                content_type: super::content_type_for(&sink.path),
                path: sink.path,
                mode: "render",
                width: None,
                height: None,
            });
        }
        if let Some((o, path)) = poster {
            let rgba = poster_pick
                .or(poster_fallback)
                .unwrap_or_else(|| frame.to_rgba8());
            let scaled = super::scale_picture(&rgba, comp.width, comp.height, (o.width, o.height));
            super::write_picture(&scaled, o.width, o.height, &path)?;
            stats.push(super::OutputStats {
                name: o.name.clone(),
                kind: "poster",
                bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                content_type: super::content_type_for(&path),
                path,
                mode: "render",
                width: Some(o.width),
                height: Some(o.height),
            });
        }
        if let (Some((o, path)), Some(sheet), Some((every, count, cols, _))) =
            (sprites, sheet, sprite_plan)
        {
            super::write_picture(sheet.as_raw(), sheet.width(), sheet.height(), &path)?;
            let mut vtt = String::from("WEBVTT\n\n");
            let sheet_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let duration = comp.duration.to_f64();
            for i in 0..count.min(next_tile.max(1)) {
                let start = f64::from(i) * every;
                let end = (start + every).min(duration);
                let (x, y) = ((i % cols) * o.width, (i / cols) * o.height);
                let _ = write!(
                    vtt,
                    "{} --> {}\n{sheet_name}#xywh={x},{y},{},{}\n\n",
                    vtt_time(start),
                    vtt_time(end),
                    o.width,
                    o.height
                );
            }
            let vtt_path = path.with_extension("vtt");
            std::fs::write(&vtt_path, vtt).map_err(|e| RenderError::Asset {
                id: vtt_path.display().to_string(),
                reason: e.to_string(),
            })?;
            stats.push(super::OutputStats {
                name: o.name.clone(),
                kind: "sprites",
                bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                content_type: super::content_type_for(&path),
                path,
                mode: "render",
                width: Some(o.width),
                height: Some(o.height),
            });
            stats.push(super::OutputStats {
                name: o.name.clone(),
                kind: "sprites-map",
                bytes: std::fs::metadata(&vtt_path).map(|m| m.len()).unwrap_or(0),
                content_type: "text/vtt",
                path: vtt_path,
                mode: "render",
                width: None,
                height: None,
            });
        }
        Ok((
            stats,
            RenderStats {
                frames: done,
                duration: comp.duration,
                mode: RenderMode::Render,
                notes,
                seconds: started.elapsed().as_secs_f64(),
            },
        ))
    }

    /// Whether an output's audio is brought to a loudness, cleaned or
    /// denoised, which only the mix can do, so no copy of it will do.
    fn audio_treated(audio: Option<&geneva_timeline::schema::AudioOutput>) -> bool {
        audio.is_some_and(|a| {
            a.loudness.is_some() || a.hygiene == Some(true) || a.denoise == Some(true)
        })
    }

    /// What the treatment of the mix measured and did, a line each.
    fn treatment_notes(r: &TreatmentReport) -> Vec<String> {
        let mut notes = Vec::new();
        if r.denoised {
            notes.push("denoise: the mix went through the speech model at 48 kHz".to_owned());
        }
        if r.high_pass {
            let hum = match &r.hum {
                Some(hum) => format!(
                    "; mains hum at {} Hz ({:.0} dBFS) notched with {} harmonic{}",
                    hum.base_hz,
                    hum.level_dbfs,
                    hum.harmonics.len() - 1,
                    if hum.harmonics.len() == 2 { "" } else { "s" }
                ),
                None => "; no mains hum found".to_owned(),
            };
            notes.push(format!(
                "hygiene: high-pass at {} Hz{hum}",
                geneva_audio::HIGH_PASS_HZ
            ));
        }
        if let Some(l) = &r.loudness {
            let Some(measured) = l.measured_lufs else {
                notes.push(format!(
                    "loudness: the mix is silent, so it was left as it is (target {} LUFS)",
                    l.target_lufs
                ));
                return notes;
            };
            let mut line = format!(
                "loudness: measured {measured:.1} LUFS, {:+.1} dB to reach {} LUFS, true peak held under {} dBTP",
                l.gain_db, l.target_lufs, l.ceiling_dbtp
            );
            if let Some(result) = l.result_lufs {
                if (result - l.target_lufs).abs() > 0.3 {
                    let _ = write!(
                        line,
                        "; written at {result:.1} LUFS, since the limiter took the peaks that carried the rest"
                    );
                }
            }
            notes.push(line);
        }
        notes
    }

    /// Placeholder kept for the sink loop above.
    /// One encoder thread per file, fed through a channel; spare plane
    /// buffers come back to be filled again.
    struct Feed<'scope> {
        /// The entry's name in `outputs`, for its notes.
        name: String,
        tx: std::sync::mpsc::SyncSender<Msg>,
        spare_rx: std::sync::mpsc::Receiver<geneva_media::convert::Planes>,
        pool: geneva_media::convert::PlanePool,
        worker: std::thread::ScopedJoinHandle<'scope, Result<Encoder, geneva_media::MediaError>>,
        audio: Option<
            std::thread::ScopedJoinHandle<
                'scope,
                Result<Option<TreatmentReport>, geneva_media::MediaError>,
            >,
        >,
    }

    fn spawn_feed<'scope, 'env>(
        scope: &'scope std::thread::Scope<'scope, 'env>,
        name: &str,
        mut encoder: Encoder,
        sample_rate: Option<u32>,
        comp: &'env Composition,
        root: &'env Path,
    ) -> Feed<'scope> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Msg>(8);
        let (spare_tx, spare_rx) = std::sync::mpsc::channel();
        let audio_encoder = sample_rate.and_then(|_| encoder.take_audio_encoder());
        let worker = scope.spawn(move || -> Result<Encoder, geneva_media::MediaError> {
            for msg in rx {
                match msg {
                    Msg::Planes(planes) => {
                        encoder.push_planes(&planes)?;
                        let _ = spare_tx.send(planes);
                    }
                    Msg::Audio(packets, time) => encoder.write_audio_packets(packets, time)?,
                    Msg::Copied(_) | Msg::EndSegment | Msg::CopiedAudio(_) => {}
                }
            }
            Ok(encoder)
        });
        let audio = audio_encoder.map(|mut enc| {
            let tx = tx.clone();
            let rate = sample_rate.expect("audio comes with a rate");
            scope.spawn(
                move || -> Result<Option<TreatmentReport>, geneva_media::MediaError> {
                    let mut mixer =
                        geneva_media::mix::Mixer::for_output(comp, root, enc.settings());
                    while let Some(chunk) = mixer.next_block(rate as usize)? {
                        let packets = enc.push(&chunk)?;
                        if tx.send(Msg::Audio(packets, enc.time())).is_err() {
                            return Ok(None);
                        }
                    }
                    let time = enc.time();
                    let packets = enc.finish()?;
                    let _ = tx.send(Msg::Audio(packets, time));
                    Ok(mixer.report().cloned())
                },
            )
        });
        Feed {
            name: name.to_owned(),
            tx,
            spare_rx,
            pool: geneva_media::convert::PlanePool::default(),
            worker,
            audio,
        }
    }

    /// Encodes the output in the plan's stretches at once, one worker
    /// each with its own decoders, compositor and encoder writing a
    /// stretch file, the first also mixing the audio; then joins the
    /// stretch files by stream copy into the output.
    fn render_chunked(
        comp: &Composition,
        root: &Path,
        output: &Path,
        settings: &EncodeSettings,
        plan: &geneva_media::chunks::ChunkPlan,
        progress: &super::Progress,
        started: Instant,
    ) -> Result<RenderStats, RenderError> {
        let media_err = |e: geneva_media::MediaError| RenderError::Asset {
            id: output.display().to_string(),
            reason: e.to_string(),
        };
        let name = output
            .file_name()
            .map_or_else(|| "output".to_owned(), |n| n.to_string_lossy().into_owned());
        let dir = output
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join(format!(".{name}.chunks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| RenderError::Asset {
            id: dir.display().to_string(),
            reason: e.to_string(),
        })?;
        let ext = output
            .extension()
            .map_or_else(|| "mkv".to_owned(), |e| e.to_string_lossy().into_owned());
        let paths: Vec<std::path::PathBuf> = (0..plan.ranges.len())
            .map(|i| dir.join(format!("chunk-{i:03}.{ext}")))
            .collect();
        let total = comp.frame_count();
        let done = std::sync::atomic::AtomicU64::new(0);
        let subtitles = settings.subtitles.clone();
        let fast_start = settings.fast_start;
        let has_audio = settings.audio.is_some();
        let mut direct_reason = None;
        let encoding_started = Instant::now();
        let results: Vec<Result<Option<String>, geneva_media::MediaError>> =
            std::thread::scope(|scope| {
                let workers: Vec<_> = plan
                    .ranges
                    .iter()
                    .zip(&paths)
                    .enumerate()
                    .map(|(i, (range, path))| {
                        let mut own = settings.clone();
                        own.subtitles.clear();
                        own.copied_audio = None;
                        own.fast_start = false;
                        if i > 0 {
                            own.audio = None;
                        }
                        if let Some(v) = own.video.as_mut() {
                            v.threads = Some(plan.threads_each);
                        }
                        let done = &done;
                        let threads = plan.threads_each;
                        scope.spawn(move || {
                            encode_chunk(comp, root, path, own, range.clone(), threads, done)
                        })
                    })
                    .collect();
                // Progress from the workers' shared count.
                let fps = comp.fps.to_f64();
                while workers.iter().any(|w| !w.is_finished()) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let n = done.load(std::sync::atomic::Ordering::Relaxed);
                    progress.frame(n, total, fps);
                }
                workers
                    .into_iter()
                    .map(|w| {
                        w.join()
                            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                    })
                    .collect()
            });
        let mut first_error = None;
        for r in results {
            match r {
                Ok(reason) => {
                    if direct_reason.is_none() {
                        direct_reason = reason;
                    }
                }
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        if let Some(e) = first_error {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(media_err(e));
        }
        progress.finish(total, total, comp.fps.to_f64());
        // Join: the stretches' video in order, the audio from the first.
        let segment = |path: &std::path::PathBuf| geneva_media::CopySegment {
            path: path.clone(),
            from: Ratio::ZERO,
            to: None,
        };
        let copy_plan = geneva_media::CopyPlan {
            segments: paths.iter().map(segment).collect(),
            audio: if has_audio {
                vec![segment(&paths[0])]
            } else {
                Vec::new()
            },
            reason: String::new(),
        };
        let encoding_secs = encoding_started.elapsed().as_secs_f64();
        let join_started = Instant::now();
        let report = geneva_media::stream_copy(&copy_plan, output, &subtitles, fast_start);
        let join_secs = join_started.elapsed().as_secs_f64();
        let _ = std::fs::remove_dir_all(&dir);
        let report = report.map_err(media_err)?;
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let mut notes = Vec::new();
        if let Some(reason) = direct_reason {
            notes.push(reason);
        }
        notes.push(format!(
            "encoded in {} stretches at once on {cores} cores ({} threads each) in {encoding_secs:.1}s, joined without re-encoding in {join_secs:.1}s",
            plan.ranges.len(),
            plan.threads_each
        ));
        Ok(RenderStats {
            frames: total,
            duration: report.duration,
            mode: if direct_reason_is_direct(&notes) {
                RenderMode::Direct
            } else {
                RenderMode::Render
            },
            notes,
            seconds: started.elapsed().as_secs_f64(),
        })
    }

    /// Whether the notes say the frames came straight from the decoder.
    fn direct_reason_is_direct(notes: &[String]) -> bool {
        notes
            .iter()
            .any(|n| n.contains("used as it is") || n.contains("used as they are"))
    }

    /// Encodes output frames `range` of `comp` into `path` with `settings`
    /// (video-only past the first stretch), by the direct path where the
    /// picture is a source's own and the compositor otherwise, counting
    /// finished frames in `done`. Returns the direct path's reason when it
    /// applied.
    fn encode_chunk(
        comp: &Composition,
        root: &Path,
        path: &Path,
        settings: EncodeSettings,
        range: std::ops::Range<u64>,
        threads: u32,
        done: &std::sync::atomic::AtomicU64,
    ) -> Result<Option<String>, geneva_media::MediaError> {
        // This stretch's decoders take its share of the cores.
        geneva_media::set_decoder_threads_for_this_thread(threads);
        let v = settings.video.as_ref().expect("chunks carry video");
        let format = geneva_media::plane_format_for(v.codec, v.profile, v.color.is_hdr());
        let tags = geneva_media::output_tags_for(v.codec, v.color);
        let output_tags = tags;
        let mut direct = geneva_media::DirectSource::open(comp, root, format, tags)?;
        let mut base = if direct.is_none()
            && geneva_render::CpuRenderer::<MediaAssets>::overlays_are_plain(comp)
        {
            geneva_media::DirectSource::open_base(comp, root, format, tags)?
        } else {
            None
        };
        let reason = direct
            .as_ref()
            .or(base.as_ref())
            .map(geneva_media::DirectSource::reason);
        let sample_rate = settings.audio.as_ref().map(|a| a.sample_rate);
        let mut encoder = Encoder::new(path, settings)?;
        let (color, format) = encoder.video_format()?;
        let audio_encoder = encoder.take_audio_encoder().zip(sample_rate);
        let render_err = |e: RenderError| geneva_media::MediaError::Codec {
            context: "rendering".to_owned(),
            reason: e.to_string(),
        };
        // As in the single run: frames are produced here while the
        // encoder runs on its own thread a few frames behind, and buffers
        // come back to be filled again.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Msg>(8);
        let (spare_tx, spare_rx) = std::sync::mpsc::channel::<geneva_media::convert::Planes>();
        let mut pool = geneva_media::convert::PlanePool::default();
        std::thread::scope(|scope| -> Result<(), geneva_media::MediaError> {
            let worker = scope.spawn(move || -> Result<Encoder, geneva_media::MediaError> {
                let mut encoder = encoder;
                for msg in rx {
                    match msg {
                        Msg::Planes(planes) => {
                            encoder.push_planes(&planes)?;
                            let _ = spare_tx.send(planes);
                        }
                        Msg::Audio(packets, time) => encoder.write_audio_packets(packets, time)?,
                        Msg::Copied(_) | Msg::EndSegment | Msg::CopiedAudio(_) => {}
                    }
                }
                Ok(encoder)
            });
            let mut audio = audio_encoder.map(|(enc, rate)| {
                let mixer = geneva_media::mix::Mixer::for_output(comp, root, enc.settings());
                (enc, rate, mixer)
            });
            let mut renderer =
                CpuRenderer::new(MediaAssets::new(root).keep_hdr(comp.color.is_hdr()));
            let mut frame = geneva_render::Frame::new(0, 0, geneva_color::Color::BLACK);
            let mut failed = None;
            for n in range {
                let t = comp.frame_time(n);
                while let Ok(spare) = spare_rx.try_recv() {
                    pool.give(spare);
                }
                let produced = if let Some(d) = direct.as_mut() {
                    d.frame_with(t, &mut pool)
                } else if let Some(b) = base.as_mut() {
                    b.frame_with(t, &mut pool).and_then(|mut planes| {
                        if let Some((overlay, rect)) =
                            renderer.render_overlays(comp, t).map_err(render_err)?
                        {
                            geneva_media::convert::blend_overlay(
                                &mut planes,
                                &overlay,
                                rect,
                                output_tags,
                            );
                        }
                        Ok(planes)
                    })
                } else {
                    renderer
                        .render_into(comp, t, &mut frame)
                        .map_err(render_err)
                        .map(|()| {
                            let mut planes = pool.take(format, frame.width(), frame.height());
                            geneva_media::convert::frame_to_planes_into(
                                &frame,
                                color,
                                format,
                                &mut planes,
                            );
                            planes
                        })
                };
                let planes = match produced {
                    Ok(p) => p,
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                };
                if tx.send(Msg::Planes(planes)).is_err() {
                    break;
                }
                done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // The audio keeps a second ahead of the picture, so the
                // file interleaves.
                if let Some((enc, rate, mixer)) = audio.as_mut() {
                    let ahead = t + Ratio::from_int(1);
                    while enc.time() < ahead {
                        match mixer.next_block(*rate as usize) {
                            Ok(Some(block)) => {
                                let packets = match enc.push(&block) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        failed = Some(e);
                                        break;
                                    }
                                };
                                if tx.send(Msg::Audio(packets, enc.time())).is_err() {
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                failed = Some(e);
                                break;
                            }
                        }
                    }
                    if failed.is_some() {
                        break;
                    }
                }
            }
            if failed.is_none() {
                if let Some((mut enc, rate, mut mixer)) = audio.take() {
                    let rest = (|| -> Result<(), geneva_media::MediaError> {
                        while let Some(block) = mixer.next_block(rate as usize)? {
                            let packets = enc.push(&block)?;
                            if tx.send(Msg::Audio(packets, enc.time())).is_err() {
                                break;
                            }
                        }
                        let time = enc.time();
                        let packets = enc.finish()?;
                        let _ = tx.send(Msg::Audio(packets, time));
                        Ok(())
                    })();
                    if let Err(e) = rest {
                        failed = Some(e);
                    }
                }
            }
            drop(tx);
            let joined = worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            if let Some(e) = failed {
                return Err(e);
            }
            joined?.finish()
        })?;
        Ok(reason)
    }

    /// The static HDR10 metadata for a PQ output: the first HDR video
    /// asset's, or standard defaults. `None` for SDR and HLG outputs.
    fn hdr_metadata_for(comp: &Composition, root: &Path) -> Option<geneva_media::HdrMetadata> {
        if comp.color.transfer != geneva_color::Transfer::Pq {
            return None;
        }
        let mut ids: Vec<&String> = comp.assets.keys().collect();
        ids.sort();
        for id in ids {
            let asset = &comp.assets[id];
            if asset
                .color
                .transfer
                .is_some_and(|t| t != geneva_color::Transfer::Pq)
            {
                continue;
            }
            if let Ok(Some(meta)) = geneva_media::hdr_metadata_of(&root.join(&asset.src)) {
                return Some(meta);
            }
        }
        Some(geneva_media::HdrMetadata::defaults())
    }

    pub fn probe(path: &Path) -> Result<MediaInfo> {
        geneva_media::probe(path).with_context(|| format!("probing {}", path.display()))
    }

    /// What a file's audio measures; `None` for a file without audio.
    pub fn measure_audio(path: &Path) -> Result<Option<geneva_audio::Report>> {
        geneva_media::measure_audio(path)
            .with_context(|| format!("measuring the audio of {}", path.display()))
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
                "  video: {} {}×{} @ {} fps, {}{}{}",
                v.codec,
                v.width,
                v.height,
                geneva_timeline::Fps(v.fps),
                v.pixel_format,
                if v.has_alpha { " with alpha" } else { "" },
                match v.rotation {
                    0 => String::new(),
                    r if r % 180 == 90 =>
                        format!(", stored as {}×{} with a {r}° rotation", v.height, v.width),
                    r => format!(", with a {r}° rotation"),
                }
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
                s.push_str("    note: HDR material; tone-mapped to SDR (BT.2446 method A) unless the output is HDR\n");
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
        for sub in &info.subtitles {
            let _ = writeln!(
                s,
                "  subtitles: {}{}",
                sub.codec,
                sub.language
                    .as_deref()
                    .map(|l| format!(" ({l})"))
                    .unwrap_or_default()
            );
        }
        s
    }

    /// Reads the cues of the `nth` subtitle stream of a file.
    pub fn read_subtitles(path: &Path, nth: usize) -> Result<Vec<geneva_media::subtitles::Cue>> {
        geneva_media::read_subtitles(path, nth)
            .with_context(|| format!("reading subtitles from {}", path.display()))
    }

    /// Renders every frame of `comp` to `output`, mixing audio unless disabled.
    /// What the render loop and the audio thread hand to the encoder
    /// thread.
    enum Msg {
        Planes(geneva_media::convert::Planes),
        Copied(Vec<geneva_media::CopiedPacket>),
        /// The end of an encoded run of a smart cut.
        EndSegment,
        CopiedAudio(Vec<geneva_media::Packet>),
        Audio(Vec<geneva_media::Packet>, geneva_timeline::Ratio),
    }

    /// Constant rate factor of the runs a smart cut encodes: near the
    /// source's quality, since they sit between its own pictures.
    const STITCH_CRF: u8 = 18;

    pub fn render(
        comp: &Composition,
        root: &Path,
        output: &Path,
        overrides: &RenderOverrides,
        progress: &super::Progress,
    ) -> Result<RenderStats, RenderError> {
        let started = Instant::now();
        let container =
            geneva_media::container_for(output, comp.encode.as_ref().and_then(|e| e.container))
                .ok_or_else(|| {
                    RenderError::Asset {
            id: output.display().to_string(),
            reason:
                "unknown container; use .mp4, .mov, .mkv, .webm, .mxf, .m4a, .ogg, .flac, .wav, .mp3, an image pattern such as frames/%04d.png, or set output.encode.container"
                    .to_owned(),
        }
                })?;
        let (mut default_video, default_audio) = geneva_media::default_codecs(container);
        if container == geneva_timeline::schema::Container::ImageSequence {
            default_video = geneva_media::default_image_codec(output);
            if !output.to_string_lossy().contains('%') {
                return Err(RenderError::Asset {
                    id: output.display().to_string(),
                    reason: "an image sequence needs a numbered pattern in the output path, for example frames/%04d.png".to_owned(),
                });
            }
        }
        let video = comp.encode.as_ref().and_then(|e| e.video.as_ref());
        let audio = comp.encode.as_ref().and_then(|e| e.audio.as_ref());
        let audio_codec = audio.and_then(|a| a.codec).unwrap_or(default_audio);
        // The source's rate when the codec and container take it.
        let sample_rate = geneva_media::audio_sample_rate_for(
            audio_codec,
            Some(container),
            comp.audio_output
                .as_ref()
                .and_then(|a| a.sample_rate)
                .unwrap_or(48000),
        );
        let channels = comp
            .audio_output
            .as_ref()
            .and_then(|a| a.channels)
            .unwrap_or(2)
            .clamp(1, 2);
        let audio_settings = if overrides.no_audio || container.is_video_only() {
            None
        } else {
            Some(AudioSettings {
                codec: audio_codec,
                bitrate_kbps: audio.and_then(|a| a.bitrate_kbps).unwrap_or(160),
                sample_rate,
                channels,
                loudness: comp.audio_output.as_ref().and_then(|a| a.loudness.clone()),
                hygiene: comp
                    .audio_output
                    .as_ref()
                    .and_then(|a| a.hygiene)
                    .unwrap_or(false),
                denoise: comp
                    .audio_output
                    .as_ref()
                    .and_then(|a| a.denoise)
                    .unwrap_or(false),
            })
        };
        let video_settings = if container.is_audio_only() {
            None
        } else {
            Some(VideoSettings {
                width: comp.width,
                height: comp.height,
                fps: comp.fps,
                codec: video.and_then(|v| v.codec).unwrap_or(default_video),
                crf: overrides.crf.or_else(|| video.and_then(|v| v.crf)),
                preset: overrides
                    .preset
                    .clone()
                    .or_else(|| video.and_then(|v| v.preset.clone())),
                hardware: video
                    .and_then(|v| v.hardware)
                    .unwrap_or(geneva_timeline::schema::HardwarePolicy::Auto),
                color: comp.color,
                profile: video.and_then(|v| v.profile),
                keyframe_interval: video.and_then(|v| v.keyframe_interval),
                max_bitrate_kbps: video.and_then(|v| v.max_bitrate_kbps),
                bitrate_kbps: video.and_then(|v| v.bitrate_kbps),
                level: video.and_then(|v| v.level.clone()),
                tune: video.and_then(|v| v.tune),
                fixed_keyframes: video.and_then(|v| v.fixed_keyframes).unwrap_or(false),
                hdr_metadata: hdr_metadata_for(comp, root),
                threads: None,
                stitch: None,
            })
        };
        if video_settings.is_none() && audio_settings.is_none() {
            return Err(RenderError::Asset {
                id: output.display().to_string(),
                reason: "an audio-only output with audio disabled has nothing to write".to_owned(),
            });
        }
        let mut subtitles = Vec::new();
        for track in &comp.subtitles {
            let Some(asset) = comp.assets.get(&track.asset) else {
                continue;
            };
            let path = root.join(&asset.src);
            let text = std::fs::read_to_string(&path).map_err(|e| RenderError::Asset {
                id: track.asset.clone(),
                reason: format!("{}: {e}", path.display()),
            })?;
            let mut cues =
                geneva_media::subtitles::parse(&text).map_err(|e| RenderError::Asset {
                    id: track.asset.clone(),
                    reason: e.to_string(),
                })?;
            geneva_media::subtitles::shift(&mut cues, track.offset);
            subtitles.push(geneva_media::SubtitleSettings {
                language: track.language.clone(),
                title: track.title.clone(),
                cues,
            });
        }
        let mut settings = EncodeSettings {
            video: video_settings,
            container: Some(container),
            audio: audio_settings,
            subtitles,
            fast_start: comp
                .encode
                .as_ref()
                .and_then(|e| e.fast_start)
                .unwrap_or(true),
            copied_audio: None,
        };
        let has_audio = settings.audio.is_some();
        let media_err = |e: geneva_media::MediaError| RenderError::Asset {
            id: output.display().to_string(),
            reason: e.to_string(),
        };
        // Copy source streams when nothing would change the picture, no
        // quality setting asks for a re-encode, and the caller did not ask
        // for exact cuts.
        let wants_encode = !overrides.picture_as_is
            && (overrides.crf.is_some()
                || overrides.preset.is_some()
                || video.is_some_and(|v| {
                    v.crf.is_some()
                        || v.preset.is_some()
                        || v.tune.is_some()
                        || v.fixed_keyframes == Some(true)
                }));
        let copyable = !overrides.exact && !wants_encode;
        // Why the streams were not copied although nothing in the
        // composition changes them: said in the notes of the render.
        let mut copy_refusal: Option<String> = None;
        if copyable {
            let requested = video.and_then(|v| v.codec);
            let plan = geneva_media::plan_stream_copy_explained(comp, root, container, requested)
                .map_err(media_err)?;
            let plan = match plan {
                Ok(mut p) => {
                    if overrides.no_audio {
                        p.audio.clear();
                    }
                    Some(p)
                }
                Err(geneva_media::CopyRefusal(reason)) => {
                    copy_refusal = reason;
                    None
                }
            };
            if let Some(plan) = plan {
                // A treated mix is encoded beside the copied picture;
                // anything else copies both tracks as they are.
                let mix_audio = settings
                    .audio
                    .clone()
                    .filter(|_| audio_treated(comp.audio_output.as_ref()));
                let mut notes = Vec::new();
                let report = match &mix_audio {
                    Some(a) => {
                        let mut mixer = geneva_media::mix::Mixer::for_output(comp, root, a);
                        let out = geneva_media::stream_copy_mixing_audio(
                            &plan,
                            output,
                            &settings.subtitles,
                            settings.fast_start,
                            a,
                            &mut |frames| mixer.next_block(frames),
                        )
                        .map_err(media_err)?;
                        if let Some(r) = mixer.report() {
                            notes.extend(treatment_notes(r));
                        }
                        out
                    }
                    None => geneva_media::stream_copy(
                        &plan,
                        output,
                        &settings.subtitles,
                        settings.fast_start,
                    )
                    .map_err(media_err)?,
                };
                notes.insert(0, plan.reason.clone());
                notes.extend(report.notes());
                return Ok(RenderStats {
                    frames: 0,
                    duration: report.duration,
                    mode: if mix_audio.is_some() {
                        RenderMode::CopyPicture
                    } else {
                        RenderMode::Copy
                    },
                    notes,
                    seconds: started.elapsed().as_secs_f64(),
                });
            }
        }
        // Smart cut: copy the sources' packets wherever nothing changes
        // and encode only the frames around the cuts and under the
        // overlays, into the same stream.
        let smart = match &settings.video {
            Some(v)
                if !wants_encode
                    && v.codec == geneva_timeline::schema::VideoCodec::H264
                    && v.hardware != geneva_timeline::schema::HardwarePolicy::Require
                    && geneva_media::system_x264().is_some() =>
            {
                geneva_media::plan_smart_cut(
                    comp,
                    root,
                    container,
                    video.and_then(|v| v.codec),
                    geneva_media::output_tags_for(v.codec, v.color),
                )
                .map_err(media_err)?
            }
            _ => None,
        };
        if let (Some(plan), Some(v)) = (&smart, settings.video.as_mut()) {
            v.stitch = Some(geneva_media::StitchSettings {
                extradata: plan.extradata.clone(),
                sps_id: plan.sps_id,
                reorder: plan.reorder,
                crf: STITCH_CRF,
            });
        }
        // The clips' own audio goes with the video: copied as coded,
        // unless the output treats its audio, which only the mix does.
        // Dropped from the plan as well as from the settings, so that
        // the plan's own note does not claim a copy that did not happen.
        let mut smart = smart;
        if audio_treated(comp.audio_output.as_ref()) {
            if let Some(plan) = smart.as_mut() {
                plan.audio = None;
            }
        }
        let smart = smart;
        let copied_audio = match &smart {
            Some(plan) if settings.audio.is_some() => plan.audio.clone(),
            _ => None,
        };
        if let Some(copy) = &copied_audio {
            settings.audio = None;
            settings.copied_audio = Some(copy.template.clone());
        }
        // Chunked encoding: the output cut into stretches encoded at the
        // same time and joined afterwards, when the encoder would leave
        // cores idle. Not with a bitrate ceiling (its buffer cannot
        // restart at a boundary), a smart cut, or an image sequence.
        let chunk_plan = match (&settings.video, &smart) {
            (Some(v), None)
                if v.max_bitrate_kbps.is_none()
                    && v.bitrate_kbps.is_none()
                    && container != geneva_timeline::schema::Container::ImageSequence =>
            {
                let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
                geneva_media::chunks::plan_chunks(
                    comp,
                    v.codec,
                    v.hardware,
                    v.keyframe_interval,
                    video.and_then(|e| e.chunks),
                    cores,
                )
            }
            _ => geneva_media::chunks::ChunkPlan::single(comp.frame_count()),
        };
        if chunk_plan.is_chunked() {
            return render_chunked(
                comp,
                root,
                output,
                &settings,
                &chunk_plan,
                progress,
                started,
            );
        }
        let has_video = settings.video.is_some();
        // When the picture is the source's own, decoded frames skip the
        // compositing pipeline.
        let mut direct = match &settings.video {
            Some(v) => {
                let format = geneva_media::plane_format_for(v.codec, v.profile, v.color.is_hdr());
                let tags = geneva_media::output_tags_for(v.codec, v.color);
                geneva_media::DirectSource::open(comp, root, format, tags).map_err(media_err)?
            }
            None => None,
        };
        // With overlays above an untouched video, the decoded frames still
        // skip the compositor: only the overlays are drawn, and laid onto
        // the frames that show them.
        let mut base = match (&direct, &settings.video) {
            (None, Some(v))
                if geneva_render::CpuRenderer::<MediaAssets>::overlays_are_plain(comp) =>
            {
                let format = geneva_media::plane_format_for(v.codec, v.profile, v.color.is_hdr());
                let tags = geneva_media::output_tags_for(v.codec, v.color);
                geneva_media::DirectSource::open_base(comp, root, format, tags)
                    .map_err(media_err)?
            }
            _ => None,
        };
        let output_tags = settings
            .video
            .as_ref()
            .map(|v| geneva_media::output_tags_for(v.codec, v.color));
        let mut notes = Vec::new();
        if let Some(reason) = copy_refusal {
            notes.push(format!("not copied without re-encoding: {reason}"));
        }
        match (&smart, direct.as_ref().or(base.as_ref())) {
            (Some(plan), _) => notes.push(plan.reason()),
            (None, Some(d)) => notes.push(d.reason()),
            (None, None) => {}
        }
        let encoder = Encoder::new(output, settings).map_err(media_err)?;
        if let Some(note) = encoder.video_encoder_note() {
            notes.push(note);
        }
        notes.extend(encoder.video_setting_notes());
        let mut renderer = if has_video {
            let (renderer, renderer_notes) =
                super::choose_renderer(overrides.renderer, root, comp.color.is_hdr());
            notes.extend(renderer_notes);
            Some(renderer)
        } else {
            None
        };
        // The overlays over a copied picture are drawn on the CPU: they
        // are small, and their blend onto the packed planes is the
        // CPU's.
        let mut overlays = base
            .is_some()
            .then(|| CpuRenderer::new(MediaAssets::new(root).keep_hdr(comp.color.is_hdr())));
        let total = if has_video { comp.frame_count() } else { 0 };
        let video_format = if has_video {
            Some(encoder.video_format().map_err(media_err)?)
        } else {
            None
        };
        // Frames are rendered and converted here while the encoder runs on
        // its own thread, a few frames behind; the audio is mixed and
        // encoded on a third thread and its packets are interleaved by the
        // encoder thread as they arrive.
        let (tx, rx) = std::sync::mpsc::sync_channel::<Msg>(8);
        // Encoded frames come back here to be filled again, so the run
        // allocates as many frame buffers as are in flight, not one per
        // picture.
        let (spare_tx, spare_rx) = std::sync::mpsc::channel::<geneva_media::convert::Planes>();
        let mut pool = geneva_media::convert::PlanePool::default();
        let mut encoder = encoder;
        let audio_encoder = if has_audio {
            encoder.take_audio_encoder()
        } else {
            None
        };
        let mut render_error = None;
        let mut composited = 0u64;
        let (joined, audio_joined) = std::thread::scope(|scope| {
            let worker = scope.spawn(move || -> Result<Encoder, geneva_media::MediaError> {
                let mut encoder = encoder;
                for msg in rx {
                    match msg {
                        Msg::Planes(planes) => {
                            encoder.push_planes(&planes)?;
                            let _ = spare_tx.send(planes);
                        }
                        Msg::Copied(batch) => {
                            for p in &batch {
                                encoder.push_copied(&p.data, p.frame, p.keyframe)?;
                            }
                        }
                        Msg::EndSegment => encoder.end_segment()?,
                        Msg::CopiedAudio(packets) => encoder.write_copied_audio(packets)?,
                        Msg::Audio(packets, time) => encoder.write_audio_packets(packets, time)?,
                    }
                }
                Ok(encoder)
            });
            let copy_worker = copied_audio.as_ref().map(|copy| {
                let tx = tx.clone();
                scope.spawn(
                    move || -> Result<Option<TreatmentReport>, geneva_media::MediaError> {
                        let mut grid = geneva_media::AudioGrid::default();
                        for segment in &copy.segments {
                            let mut batch = Vec::with_capacity(64);
                            let mut stopped = false;
                            geneva_media::read_copied_audio(segment, &mut grid, &mut |p| {
                                batch.push(p);
                                if batch.len() < 64 {
                                    return true;
                                }
                                stopped = tx
                                    .send(Msg::CopiedAudio(std::mem::take(&mut batch)))
                                    .is_err();
                                !stopped
                            })?;
                            if stopped
                                || (!batch.is_empty() && tx.send(Msg::CopiedAudio(batch)).is_err())
                            {
                                return Ok(None);
                            }
                        }
                        Ok(None)
                    },
                )
            });
            let audio_worker = audio_encoder.map(|mut enc| {
                let tx = tx.clone();
                scope.spawn(
                    move || -> Result<Option<TreatmentReport>, geneva_media::MediaError> {
                        // A second of audio per message keeps the video frames
                        // flowing between them; the mix is made a second at a
                        // time too, so a long timeline never holds it whole.
                        let mut mixer =
                            geneva_media::mix::Mixer::for_output(comp, root, enc.settings());
                        while let Some(chunk) = mixer.next_block(sample_rate as usize)? {
                            let packets = enc.push(&chunk)?;
                            if tx.send(Msg::Audio(packets, enc.time())).is_err() {
                                return Ok(None);
                            }
                        }
                        let time = enc.time();
                        let packets = enc.finish()?;
                        let _ = tx.send(Msg::Audio(packets, time));
                        Ok(mixer.report().cloned())
                    },
                )
            });
            // One output frame, by whichever path applies.
            let mut produce = |n: u64| -> Result<geneva_media::convert::Planes, RenderError> {
                let t = comp.frame_time(n);
                while let Ok(spare) = spare_rx.try_recv() {
                    pool.give(spare);
                }
                if let Some(d) = direct.as_mut() {
                    return d.frame_with(t, &mut pool).map_err(media_err);
                }
                if let Some(b) = base.as_mut() {
                    let mut planes = b.frame_with(t, &mut pool).map_err(media_err)?;
                    let overlays = overlays.as_mut().expect("made with the base");
                    if let Some((overlay, rect)) = overlays.render_overlays(comp, t)? {
                        let tags = output_tags.expect("video output has tags");
                        geneva_media::convert::blend_overlay(&mut planes, &overlay, rect, tags);
                        composited += 1;
                    }
                    return Ok(planes);
                }
                let renderer = renderer.as_mut().expect("a video output has a renderer");
                let (color, format) = video_format.expect("video output has a format");
                let mut planes = pool.take(format, comp.width, comp.height);
                // The next frame is started first, so a renderer that
                // can draws it while this one is read back.
                if n + 1 < total {
                    renderer.prepare_planes(comp, comp.frame_time(n + 1), &[(color, format)]);
                }
                renderer.render_planes(
                    comp,
                    t,
                    &mut [geneva_media::PlaneTarget {
                        tags: color,
                        format,
                        planes: &mut planes,
                    }],
                    None,
                )?;
                Ok(planes)
            };
            let mut done = 0u64;
            let fps = comp.fps.to_f64();
            let report = |done: u64| progress.frame(done, total, fps);
            // Sends the frames of one run; false once the encoder stopped
            // (its error is reported below) or a frame failed.
            let mut encode_run = |range: std::ops::Range<u64>,
                                  render_error: &mut Option<RenderError>,
                                  done: &mut u64|
             -> bool {
                for n in range {
                    let planes = match produce(n) {
                        Ok(planes) => planes,
                        Err(e) => {
                            *render_error = Some(e);
                            return false;
                        }
                    };
                    if tx.send(Msg::Planes(planes)).is_err() {
                        return false;
                    }
                    *done += 1;
                    report(*done);
                }
                true
            };
            match &smart {
                None => {
                    encode_run(0..total, &mut render_error, &mut done);
                }
                Some(plan) => {
                    for segment in &plan.segments {
                        let ok = match segment {
                            geneva_media::Segment::Encode { frames } => {
                                encode_run(frames.clone(), &mut render_error, &mut done)
                                    && tx.send(Msg::EndSegment).is_ok()
                            }
                            geneva_media::Segment::Copy {
                                source,
                                packets,
                                offset,
                                frames,
                            } => {
                                let mut batch = Vec::with_capacity(64);
                                let mut stopped = false;
                                let result = geneva_media::read_copied(
                                    &plan.sources[*source],
                                    packets.clone(),
                                    *offset,
                                    &mut |p| {
                                        batch.push(p);
                                        if batch.len() < 64 {
                                            return true;
                                        }
                                        stopped = tx
                                            .send(Msg::Copied(std::mem::take(&mut batch)))
                                            .is_err();
                                        !stopped
                                    },
                                );
                                if !stopped && !batch.is_empty() {
                                    stopped = tx.send(Msg::Copied(batch)).is_err();
                                }
                                if let Err(e) = result {
                                    render_error = Some(media_err(e));
                                }
                                done += frames.end - frames.start;
                                report(done);
                                !stopped && render_error.is_none()
                            }
                        };
                        if !ok {
                            break;
                        }
                    }
                }
            }
            drop(tx);
            let audio_joined = audio_worker.or(copy_worker).map(|w| {
                w.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            });
            let joined = worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            (joined, audio_joined)
        });
        if let Some(e) = render_error {
            return Err(e);
        }
        let encoder = joined.map_err(media_err)?;
        match audio_joined {
            Some(Err(e)) => return Err(media_err(e)),
            Some(Ok(Some(report))) => notes.extend(treatment_notes(&report)),
            _ => {}
        }
        progress.finish(total, total, comp.fps.to_f64());
        if base.is_some() && smart.is_none() {
            notes.push(format!(
                "overlays were drawn onto {composited} of {total} frames; the others went from the decoder to the encoder untouched"
            ));
        }
        encoder.finish().map_err(media_err)?;
        Ok(RenderStats {
            frames: total,
            duration: comp.duration,
            mode: if smart.is_some() {
                RenderMode::Smart
            } else if direct.is_some() || base.is_some() {
                RenderMode::Direct
            } else {
                RenderMode::Render
            },
            notes,
            seconds: started.elapsed().as_secs_f64(),
        })
    }
}

#[cfg(not(feature = "media"))]
mod imp {
    use std::path::Path;

    use anyhow::Result;
    use geneva_render::{CpuRenderer, FileAssets, RenderError};
    use geneva_timeline::Composition;

    use super::{RenderMode, RenderOverrides, RenderStats};

    fn unavailable() -> anyhow::Error {
        anyhow::anyhow!("{}", geneva_media::MediaError::Unavailable)
    }

    pub fn probe(_: &Path) -> Result<geneva_media::MediaInfo> {
        Err(unavailable())
    }

    pub fn measure_audio(_: &Path) -> Result<Option<geneva_audio::Report>> {
        Err(unavailable())
    }

    pub fn renderer(root: std::path::PathBuf) -> CpuRenderer<FileAssets> {
        CpuRenderer::with_asset_root(root)
    }

    pub fn describe(_: &Path, _: &geneva_media::MediaInfo) -> String {
        String::new()
    }

    pub fn read_subtitles(_: &Path, _: usize) -> Result<Vec<geneva_media::subtitles::Cue>> {
        Err(unavailable())
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn copy_sources(
        _: &Composition,
        _: &Path,
        _: &Path,
    ) -> Result<Option<Vec<std::path::PathBuf>>> {
        Ok(None)
    }

    pub fn render_outputs(
        _: &Composition,
        _: &Path,
        dir: &Path,
        _: &RenderOverrides,
        _: &super::Progress,
    ) -> Result<(Vec<super::OutputStats>, RenderStats), RenderError> {
        Err(RenderError::Asset {
            id: dir.display().to_string(),
            reason: geneva_media::MediaError::Unavailable.to_string(),
        })
    }

    pub fn render(
        _: &Composition,
        _: &Path,
        output: &Path,
        overrides: &RenderOverrides,
        _: &super::Progress,
    ) -> Result<RenderStats, RenderError> {
        let _ = (
            overrides.crf,
            overrides.preset.as_deref(),
            overrides.no_audio,
            overrides.exact,
            [RenderMode::Direct, RenderMode::Smart, RenderMode::Render],
        );
        Err(RenderError::Asset {
            id: output.display().to_string(),
            reason: geneva_media::MediaError::Unavailable.to_string(),
        })
    }
}

#[allow(dead_code)]
fn _unused(_: &dyn Renderer, _: &Composition, _: RenderError, _: Result<()>) {}
