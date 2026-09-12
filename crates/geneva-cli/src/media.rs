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
    pub exact: bool,
}

/// What a render produced.
/// How `render` produced its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Source streams were copied without decoding.
    Copy,
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

pub use imp::{describe, probe, read_subtitles, render, renderer};

#[cfg(feature = "media")]
mod imp {
    use std::path::Path;
    use std::time::Instant;

    use anyhow::{Context, Result};
    use geneva_media::{
        AudioSettings, EncodeSettings, Encoder, MediaAssets, MediaInfo, VideoSettings,
    };
    use geneva_render::{CpuRenderer, RenderError, Renderer};
    use geneva_timeline::{Composition, Ratio};

    use super::{RenderMode, RenderOverrides, RenderStats};

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
        progress: bool,
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
                if progress {
                    let mut last = 0;
                    while workers.iter().any(|w| !w.is_finished()) {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        let n = done.load(std::sync::atomic::Ordering::Relaxed);
                        if n != last {
                            eprint!("\rframe {n}/{total}");
                            last = n;
                        }
                    }
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
        if progress {
            eprintln!("\rframe {total}/{total}");
        }
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
            let mut audio = audio_encoder
                .map(|(enc, rate)| (enc, rate, geneva_media::mix::Mixer::new(comp, root, rate)));
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
                s.push_str("    note: HDR material; tone-mapped to SDR (BT.2390) unless the output is HDR\n");
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
        progress: bool,
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
        let wants_encode = overrides.crf.is_some()
            || overrides.preset.is_some()
            || video.is_some_and(|v| {
                v.crf.is_some()
                    || v.preset.is_some()
                    || v.tune.is_some()
                    || v.fixed_keyframes == Some(true)
            });
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
                let report = geneva_media::stream_copy(
                    &plan,
                    output,
                    &settings.subtitles,
                    settings.fast_start,
                )
                .map_err(media_err)?;
                let mut notes = vec![plan.reason.clone()];
                notes.extend(report.notes());
                return Ok(RenderStats {
                    frames: 0,
                    duration: report.duration,
                    mode: RenderMode::Copy,
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
        // The clips' own audio goes with the video: copied as coded.
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
        let mut renderer = CpuRenderer::new(MediaAssets::new(root).keep_hdr(comp.color.is_hdr()));
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
                scope.spawn(move || -> Result<(), geneva_media::MediaError> {
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
                            return Ok(());
                        }
                    }
                    Ok(())
                })
            });
            let audio_worker = audio_encoder.map(|mut enc| {
                let tx = tx.clone();
                scope.spawn(move || -> Result<(), geneva_media::MediaError> {
                    // A second of audio per message keeps the video frames
                    // flowing between them; the mix is made a second at a
                    // time too, so a long timeline never holds it whole.
                    let mut mixer = geneva_media::mix::Mixer::new(comp, root, sample_rate);
                    while let Some(chunk) = mixer.next_block(sample_rate as usize)? {
                        let packets = enc.push(&chunk)?;
                        if tx.send(Msg::Audio(packets, enc.time())).is_err() {
                            return Ok(());
                        }
                    }
                    let time = enc.time();
                    let packets = enc.finish()?;
                    let _ = tx.send(Msg::Audio(packets, time));
                    Ok(())
                })
            });
            let mut frame = geneva_render::Frame::new(0, 0, geneva_color::Color::BLACK);
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
                    if let Some((overlay, rect)) = renderer.render_overlays(comp, t)? {
                        let tags = output_tags.expect("video output has tags");
                        geneva_media::convert::blend_overlay(&mut planes, &overlay, rect, tags);
                        composited += 1;
                    }
                    return Ok(planes);
                }
                renderer.render_into(comp, t, &mut frame)?;
                let (color, format) = video_format.expect("video output has a format");
                let mut planes = pool.take(format, frame.width(), frame.height());
                geneva_media::convert::frame_to_planes_into(&frame, color, format, &mut planes);
                Ok(planes)
            };
            let mut done = 0u64;
            let report = |done: u64| {
                if progress && (done % 30 == 0 || done == total) {
                    eprint!("\rframe {done}/{total}");
                }
            };
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
        if let Some(Err(e)) = audio_joined {
            return Err(media_err(e));
        }
        if progress && total > 0 {
            eprintln!();
        }
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

    pub fn renderer(root: std::path::PathBuf) -> CpuRenderer<FileAssets> {
        CpuRenderer::with_asset_root(root)
    }

    pub fn describe(_: &Path, _: &geneva_media::MediaInfo) -> String {
        String::new()
    }

    pub fn read_subtitles(_: &Path, _: usize) -> Result<Vec<geneva_media::subtitles::Cue>> {
        Err(unavailable())
    }

    pub fn render(
        _: &Composition,
        _: &Path,
        output: &Path,
        overrides: &RenderOverrides,
        _: bool,
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
