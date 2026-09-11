//! The `geneva` command-line tool.

#![forbid(unsafe_code)]

mod media;
mod targets;
mod verbs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use geneva_render::{Frame, RenderError, Renderer};
use geneva_timeline::{Composition, Diagnostic, Loaded, Ratio, Time, summarize};

/// Exit status when the timeline has validation errors.
const EXIT_INVALID: u8 = 1;
/// Exit status when rendering failed.
const EXIT_RENDER: u8 = 3;

#[derive(Parser)]
#[command(name = "geneva", version, about = "Composition and rendering engine for video", long_about = None)]
struct Cli {
    /// Output format for diagnostics and results.
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Readable text.
    Human,
    /// One JSON document, for tools and agents.
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// Check a timeline and report every problem with its location and a fix.
    Validate(ValidateArgs),
    /// Render one frame of a timeline to a PNG file.
    Frame(FrameArgs),
    /// Render a whole timeline to a video file.
    Render(RenderArgs),
    /// Show what a media file contains.
    Probe(ProbeArgs),
    /// Print the JSON Schema of the timeline format.
    Schema,
    /// Re-encode a video, optionally changing its codec, size or frame rate.
    Convert(ConvertArgs),
    /// Change a video's size.
    Resize(ResizeArgs),
    /// Cut a range out of a video.
    Trim(TrimArgs),
    /// Join videos back to back.
    Concat(ConcatArgs),
    /// Place an image or video over a video.
    Overlay(OverlayArgs),
    /// Extract, remove, replace or mix a video's audio.
    Audio(AudioArgs),
    /// Attach subtitle files as streams, or extract a subtitle stream.
    Subtitles(SubtitlesArgs),
    /// List the destinations `--for` knows and what each one implies.
    Targets,
}

#[derive(Args)]
struct TimelineArgs {
    /// Path to the timeline JSON file.
    timeline: PathBuf,
    /// Directory that asset paths are relative to. Defaults to the
    /// timeline's directory.
    #[arg(long, value_name = "DIR")]
    assets: Option<PathBuf>,
}

impl TimelineArgs {
    fn root(&self) -> PathBuf {
        self.assets
            .clone()
            .unwrap_or_else(|| timeline_dir(&self.timeline))
    }
}

#[derive(Args)]
struct ValidateArgs {
    #[command(flatten)]
    timeline: TimelineArgs,
    /// Also open the media assets to check that they exist and to learn
    /// their lengths.
    #[arg(long)]
    probe: bool,
}

#[derive(Args)]
struct FrameArgs {
    #[command(flatten)]
    timeline: TimelineArgs,
    /// Time to render, for example 1.5, "1.5s", "45f" or "00:00:01.5".
    /// Defaults to 0.
    #[arg(long, value_name = "TIME", conflicts_with = "frame")]
    at: Option<String>,
    /// Frame number to render, starting at 0.
    #[arg(long, value_name = "N")]
    frame: Option<u64>,
    /// Where to write the PNG.
    #[arg(short, long, value_name = "FILE", default_value = "frame.png")]
    output: PathBuf,
}

#[derive(Args)]
struct RenderArgs {
    #[command(flatten)]
    timeline: TimelineArgs,
    /// Output file. The extension selects the container unless the
    /// timeline sets one.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Constant-quality level, overriding the timeline.
    #[arg(long)]
    crf: Option<u8>,
    /// Encoder preset, overriding the timeline.
    #[arg(long)]
    preset: Option<String>,
    /// Where the file is going (see `geneva targets`); sets the encode
    /// block for it without changing the timeline's size.
    #[arg(long = "for", value_name = "TARGET")]
    for_: Option<String>,
    /// Quality tier for --for: best, good (default) or eco.
    #[arg(long, value_enum, requires = "for_")]
    quality: Option<targets::Quality>,
    /// Size budget for --for, such as 25MB.
    #[arg(long, value_name = "SIZE", requires = "for_")]
    budget: Option<String>,
    /// Write no audio track.
    #[arg(long)]
    no_audio: bool,
    /// Cut on the exact frame instead of moving cuts to keyframes. With an
    /// H.264 source and the system's x264, only the frames from a cut to
    /// the next keyframe are re-encoded and the rest is copied (smart
    /// cut); otherwise everything is re-encoded.
    #[arg(long)]
    exact: bool,
}

#[derive(Args)]
struct ProbeArgs {
    /// Media file to inspect.
    file: PathBuf,
}

#[derive(Args)]
struct ConvertArgs {
    /// Input video.
    input: PathBuf,
    /// Output file. The extension selects the container.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Output width in pixels. The height follows when not given.
    #[arg(long)]
    width: Option<u32>,
    /// Output height in pixels. The width follows when not given.
    #[arg(long)]
    height: Option<u32>,
    /// How to fit the picture when the shape changes (also the canvas
    /// --for builds): contain by default.
    #[arg(long, value_enum)]
    fit: Option<verbs::FitArg>,
    /// Output frame rate, for example 30 or 30000/1001.
    #[arg(long, value_name = "FPS")]
    fps: Option<String>,
    #[command(flatten)]
    encode: verbs::EncodeArgs,
}

#[derive(Args)]
struct ResizeArgs {
    /// Input video.
    input: PathBuf,
    /// Output file. The extension selects the container.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Output width in pixels. The height follows when not given.
    #[arg(long, required_unless_present = "height")]
    width: Option<u32>,
    /// Output height in pixels. The width follows when not given.
    #[arg(long, required_unless_present = "width")]
    height: Option<u32>,
    /// How to fit the picture when the shape changes (also the canvas
    /// --for builds): contain by default.
    #[arg(long, value_enum)]
    fit: Option<verbs::FitArg>,
    #[command(flatten)]
    encode: verbs::EncodeArgs,
}

#[derive(Args)]
struct TrimArgs {
    /// Input video.
    input: PathBuf,
    /// Output file. The extension selects the container.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Where the kept range starts, for example 1.5, "1.5s", "45f" or
    /// "00:00:01.5". Defaults to the beginning.
    #[arg(long, value_name = "TIME")]
    from: Option<String>,
    /// Where the kept range ends. Defaults to the end.
    #[arg(long, value_name = "TIME", conflicts_with = "duration")]
    to: Option<String>,
    /// Length of the kept range, instead of an end time.
    #[arg(long, value_name = "TIME")]
    duration: Option<String>,
    #[command(flatten)]
    encode: verbs::EncodeArgs,
}

#[derive(Args)]
struct ConcatArgs {
    /// Input videos, in order.
    #[arg(required = true, num_args = 2..)]
    inputs: Vec<PathBuf>,
    /// Output file. The extension selects the container.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Cross-fade between inputs over this long instead of cutting.
    #[arg(long, value_name = "TIME")]
    crossfade: Option<String>,
    #[command(flatten)]
    encode: verbs::EncodeArgs,
}

#[derive(Args)]
struct OverlayArgs {
    /// Input video.
    input: PathBuf,
    /// Image or video to place on top.
    overlay: PathBuf,
    /// Output file. The extension selects the container.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Where to put the overlay.
    #[arg(long, value_enum, default_value_t = verbs::Corner::TopRight)]
    at: verbs::Corner,
    /// Distance from the edges in pixels.
    #[arg(long, default_value_t = 24.0)]
    margin: f64,
    /// Scale factor applied to the overlay.
    #[arg(long, default_value_t = 1.0)]
    scale: f64,
    /// Opacity from 0 to 1.
    #[arg(long, default_value_t = 1.0)]
    opacity: f64,
    /// When the overlay appears. Defaults to the beginning.
    #[arg(long, value_name = "TIME")]
    start: Option<String>,
    /// How long the overlay stays. Defaults to the end of the video.
    #[arg(long, value_name = "TIME")]
    duration: Option<String>,
    #[command(flatten)]
    encode: verbs::EncodeArgs,
}

#[derive(Args)]
#[command(group = clap::ArgGroup::new("operation")
    .required(true)
    .args(["extract", "mute", "replace", "mix"]))]
struct AudioArgs {
    /// Input file.
    input: PathBuf,
    /// Output file. For --extract, an audio extension such as .m4a, .ogg,
    /// .flac or .wav.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Write only the audio, to an audio file.
    #[arg(long)]
    extract: bool,
    /// Drop the audio track.
    #[arg(long)]
    mute: bool,
    /// Replace the audio with this file's.
    #[arg(long, value_name = "FILE")]
    replace: Option<PathBuf>,
    /// Mix this file's audio in with the original.
    #[arg(long, value_name = "FILE")]
    mix: Option<PathBuf>,
    /// Gain in decibels applied to the mixed-in audio.
    #[arg(
        long,
        default_value_t = 0.0,
        requires = "mix",
        allow_negative_numbers = true
    )]
    gain: f64,
    #[command(flatten)]
    encode: verbs::EncodeArgs,
}

#[derive(Args)]
#[command(group = clap::ArgGroup::new("operation")
    .required(true)
    .args(["extract", "add", "burn"]))]
struct SubtitlesArgs {
    /// Input file.
    input: PathBuf,
    /// Output file: a video file for --add, or a .srt or .vtt file for
    /// --extract.
    #[arg(short, long, value_name = "FILE")]
    output: PathBuf,
    /// Write one subtitle stream of the input to a subtitle file.
    #[arg(long)]
    extract: bool,
    /// Which subtitle stream to extract, counting from 0.
    #[arg(long, default_value_t = 0, requires = "extract")]
    track: usize,
    /// Subtitle file (.srt or .vtt) to attach as a stream; repeatable.
    #[arg(long, value_name = "FILE")]
    add: Vec<PathBuf>,
    /// Language code for each --add, in the same order; repeatable.
    #[arg(long, value_name = "CODE", requires = "add")]
    language: Vec<String>,
    /// Subtitle file (.srt or .vtt) to draw into the picture.
    #[arg(long, value_name = "FILE", conflicts_with_all = ["add", "extract"])]
    burn: Option<PathBuf>,
    /// Where burned-in subtitles sit.
    #[arg(long, default_value = "bottom", requires = "burn")]
    position: verbs::SubtitlePosition,
    /// Distance from the top or bottom edge in pixels; by default the
    /// title-safe inset (--safe), at least 5% of the height.
    #[arg(long, value_name = "PX", requires = "burn")]
    margin: Option<f64>,
    /// JSON object of text fields merged over the default look, for
    /// example '{"size": 36, "color": "#ffdd00", "background": "#00000080"}'.
    #[arg(long, value_name = "JSON", requires = "burn")]
    style: Option<String>,
    /// Title-safe inset as a percentage of each dimension; cues outside it
    /// get a note. 0 turns the note off.
    #[arg(long, default_value_t = 5.0, value_name = "PERCENT", requires = "burn")]
    safe: f64,
    /// Shrink cues that do not fit the title-safe area until they do,
    /// down to half their size; each one shrunk is reported.
    #[arg(long, requires = "burn")]
    fit: bool,
    #[command(flatten)]
    encode: verbs::EncodeArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Schema => {
            println!(
                "{}",
                serde_json::to_string_pretty(&geneva_timeline::json_schema())?
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Validate(args) => {
            let loaded = load_timeline(&args.timeline, args.probe)?;
            report(&loaded.diagnostics, cli.format, None)?;
            Ok(if loaded.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_INVALID)
            })
        }
        Command::Probe(args) => {
            let info = media::probe(&args.file)?;
            match cli.format {
                Format::Json => println!("{}", serde_json::to_string_pretty(&info)?),
                Format::Human => print!("{}", media::describe(&args.file, &info)),
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Frame(args) => {
            let loaded = load_timeline(&args.timeline, true)?;
            let Some(comp) = &loaded.composition else {
                report(&loaded.diagnostics, cli.format, None)?;
                return Ok(ExitCode::from(EXIT_INVALID));
            };
            let time = match (&args.at, args.frame) {
                (Some(text), _) => Time::parse(text)
                    .map_err(|e| anyhow::anyhow!("--at: {e}"))?
                    .resolve(comp.fps),
                (None, Some(n)) => comp.frame_time(n),
                (None, None) => Ratio::ZERO,
            };
            let mut renderer = media::renderer(args.timeline.root());
            let mut diagnostics = loaded.diagnostics.clone();
            match renderer.render_frame(comp, time) {
                Ok(frame) => {
                    write_png(&frame, &args.output)?;
                    let frame_index = (time * comp.fps).floor();
                    let result = serde_json::json!({
                        "ok": true,
                        "output": args.output,
                        "time": time.to_string(),
                        "frame": frame_index,
                        "width": comp.width,
                        "height": comp.height,
                    });
                    if cli.format == Format::Human {
                        report(&diagnostics, cli.format, None)?;
                        println!(
                            "wrote {} ({}×{}, frame {} at {}s)",
                            args.output.display(),
                            comp.width,
                            comp.height,
                            frame_index,
                            time
                        );
                    } else {
                        report(&diagnostics, cli.format, Some(result))?;
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Err(err) => {
                    diagnostics.push(render_diagnostic(&err));
                    report(
                        &diagnostics,
                        cli.format,
                        Some(serde_json::json!({ "ok": false })),
                    )?;
                    Ok(ExitCode::from(EXIT_RENDER))
                }
            }
        }
        Command::Render(args) => {
            let mut loaded = load_timeline(&args.timeline, true)?;
            let mut size_limit = None;
            if let Some(name) = &args.for_ {
                let text = std::fs::read_to_string(&args.timeline.timeline)?;
                if let Ok(mut tl) = serde_json::from_str::<geneva_timeline::Timeline>(&text) {
                    let (extra, limit) = apply_target(
                        &mut tl,
                        &loaded,
                        name,
                        args.quality,
                        args.budget.as_deref(),
                        args.crf,
                        None,
                        false,
                        &args.output,
                        !args.no_audio,
                    )?;
                    size_limit = limit;
                    let text = serde_json::to_string_pretty(&tl)?;
                    loaded = load_text(&text, &args.timeline.root(), true);
                    loaded.diagnostics.extend(extra);
                }
            }
            let overrides = media::RenderOverrides {
                crf: args.crf,
                preset: args.preset.clone(),
                no_audio: args.no_audio,
                exact: args.exact,
            };
            render_to(
                &loaded,
                &args.timeline.root(),
                &args.output,
                &overrides,
                cli.format,
                size_limit.as_ref(),
            )
        }
        Command::Convert(args) => {
            let fps = args
                .fps
                .as_deref()
                .map(|f| geneva_timeline::Fps::parse(f).map_err(|e| anyhow::anyhow!("--fps: {e}")))
                .transpose()?;
            let compiled = verbs::convert(
                &args.input,
                args.width,
                args.height,
                args.fit,
                fps,
                &args.encode,
            )?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
        Command::Resize(args) => {
            let compiled = verbs::convert(
                &args.input,
                args.width,
                args.height,
                args.fit,
                None,
                &args.encode,
            )?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
        Command::Trim(args) => {
            let compiled = verbs::trim(
                &args.input,
                parse_time("--from", args.from.as_deref())?,
                parse_time("--to", args.to.as_deref())?,
                parse_time("--duration", args.duration.as_deref())?,
                &args.encode,
            )?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
        Command::Concat(args) => {
            let compiled = verbs::concat(
                &args.inputs,
                parse_time("--crossfade", args.crossfade.as_deref())?,
                &args.encode,
            )?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
        Command::Overlay(args) => {
            let placement = verbs::Placement {
                corner: args.at,
                margin: args.margin,
                scale: args.scale,
                opacity: args.opacity,
                start: parse_time("--start", args.start.as_deref())?,
                duration: parse_time("--duration", args.duration.as_deref())?,
            };
            let compiled = verbs::overlay(&args.input, &args.overlay, &placement, &args.encode)?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
        Command::Audio(args) => {
            let op = if args.extract {
                verbs::AudioOp::Extract
            } else if args.mute {
                verbs::AudioOp::Mute
            } else if let Some(p) = &args.replace {
                verbs::AudioOp::Replace(p.clone())
            } else if let Some(p) = &args.mix {
                verbs::AudioOp::Mix(p.clone(), args.gain)
            } else {
                anyhow::bail!("choose one of --extract, --mute, --replace or --mix");
            };
            let compiled = verbs::audio(&args.input, &op, &args.encode)?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
        Command::Targets => {
            if cli.format == Format::Json {
                println!("{}", serde_json::to_string_pretty(&targets::table())?);
            } else {
                targets::print_table();
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Subtitles(args) => {
            if args.extract {
                let cues = media::read_subtitles(&args.input, args.track)?;
                let ext = args
                    .output
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                let text = match geneva_media::subtitles::SubtitleFormat::from_extension(ext) {
                    Some(geneva_media::subtitles::SubtitleFormat::Srt) => {
                        geneva_media::subtitles::write_srt(&cues)
                    }
                    Some(geneva_media::subtitles::SubtitleFormat::WebVtt) => {
                        geneva_media::subtitles::write_webvtt(&cues)
                    }
                    None => anyhow::bail!("the output must end in .srt or .vtt"),
                };
                std::fs::write(&args.output, text)
                    .with_context(|| format!("writing {}", args.output.display()))?;
                if cli.format == Format::Json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "output": args.output,
                            "cues": cues.len(),
                        }))?
                    );
                } else {
                    println!("wrote {} ({} cues)", args.output.display(), cues.len());
                }
                return Ok(ExitCode::SUCCESS);
            }
            if let Some(file) = &args.burn {
                let opts = verbs::BurnOptions {
                    file: file.clone(),
                    position: args.position,
                    margin: args.margin,
                    style: args.style.clone(),
                    safe: args.safe,
                    fit: args.fit,
                };
                let compiled = verbs::burn_subtitles(&args.input, &opts, &args.encode)?;
                return run_verb(&compiled, &args.output, &args.encode, cli.format);
            }
            if args.add.is_empty() {
                anyhow::bail!("give --add FILE, --burn FILE or --extract");
            }
            if args.language.len() > args.add.len() {
                anyhow::bail!("more --language values than --add files");
            }
            let files: Vec<verbs::SubtitleFile> = args
                .add
                .iter()
                .enumerate()
                .map(|(i, p)| verbs::SubtitleFile {
                    path: p.clone(),
                    language: args.language.get(i).cloned(),
                })
                .collect();
            let compiled = verbs::add_subtitles(&args.input, &files, &args.encode)?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
    }
}

fn parse_time(flag: &str, text: Option<&str>) -> Result<Option<Time>> {
    text.map(|t| Time::parse(t).map_err(|e| anyhow::anyhow!("{flag}: {e}")))
        .transpose()
}

/// Renders a compiled verb, or prints its timeline when asked to.
fn run_verb(
    compiled: &verbs::Compiled,
    output: &Path,
    encode: &verbs::EncodeArgs,
    format: Format,
) -> Result<ExitCode> {
    let mut timeline = compiled.timeline.clone();
    let mut extra = compiled.diagnostics.clone();
    let mut size_limit = None;
    if let Some(name) = &encode.for_ {
        // A first resolution gives the facts the target needs (the size
        // and length the verb arrived at); the target then rewrites the
        // output block and the timeline is resolved again.
        let text = serde_json::to_string_pretty(&timeline)?;
        let first = load_text(&text, &compiled.root, true);
        let (notes, limit) = apply_target(
            &mut timeline,
            &first,
            name,
            encode.quality,
            encode.budget.as_deref(),
            encode.crf,
            encode.codec.map(Into::into),
            true,
            output,
            !encode.no_audio,
        )?;
        extra.extend(notes);
        size_limit = limit;
    }
    let text = serde_json::to_string_pretty(&timeline)?;
    if encode.show_timeline {
        println!("{text}");
        if format == Format::Human {
            for d in &extra {
                eprint!("{d}");
            }
            eprintln!("asset paths are relative to {}", compiled.root.display());
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut loaded = load_text(&text, &compiled.root, true);
    loaded.diagnostics.extend(extra);
    let overrides = media::RenderOverrides {
        crf: encode.crf,
        preset: encode.preset.clone(),
        no_audio: encode.no_audio,
        exact: encode.exact,
    };
    render_to(
        &loaded,
        &compiled.root,
        output,
        &overrides,
        format,
        size_limit.as_ref(),
    )
}

/// A target's size limit in bytes, with the target's name for the report.
type SizeLimit = Option<(String, u64)>;

/// Applies a `--for` target to a timeline: the facts come from `resolved`
/// (a first resolution of the same document). Returns the target's
/// diagnostics and the size limit to check the written file against.
#[allow(clippy::too_many_arguments)]
fn apply_target(
    timeline: &mut geneva_timeline::Timeline,
    resolved: &Loaded,
    name: &str,
    quality: Option<targets::Quality>,
    budget: Option<&str>,
    crf: Option<u8>,
    codec: Option<geneva_timeline::schema::VideoCodec>,
    resize: bool,
    output: &Path,
    audio: bool,
) -> Result<(Vec<Diagnostic>, SizeLimit)> {
    let target = targets::find(name).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown target {name:?}; the targets are {} (see `geneva targets`)",
            targets::names()
        )
    })?;
    let budget = budget
        .map(targets::parse_budget)
        .transpose()
        .map_err(|e| anyhow::anyhow!("--budget: {e}"))?;
    let Some(comp) = &resolved.composition else {
        // The document does not resolve; its own errors are reported by
        // the caller, and the target has nothing to decide from.
        return Ok((Vec::new(), None));
    };
    let facts = targets::Facts {
        width: comp.width,
        height: comp.height,
        fps: comp.fps,
        duration: Some(comp.duration),
        audio,
    };
    let extension = output.extension().and_then(|e| e.to_str());
    let opts = targets::Options {
        target,
        quality,
        budget,
        crf,
        codec,
        resize,
        extension,
    };
    let notes = targets::apply(timeline, &facts, &opts);
    let limit = match (budget, target.max_bytes) {
        (Some(b), Some(m)) => Some(b.min(m)),
        (b, m) => b.or(m),
    }
    .map(|bytes| (target.name.to_owned(), bytes));
    Ok((notes, limit))
}

/// Renders a loaded timeline to `output` and reports the outcome.
fn render_to(
    loaded: &Loaded,
    root: &Path,
    output: &Path,
    overrides: &media::RenderOverrides,
    format: Format,
    size_limit: Option<&(String, u64)>,
) -> Result<ExitCode> {
    let Some(comp) = &loaded.composition else {
        report(&loaded.diagnostics, format, None)?;
        return Ok(ExitCode::from(EXIT_INVALID));
    };
    let mut diagnostics = loaded.diagnostics.clone();
    match media::render(comp, root, output, overrides, format == Format::Human) {
        Ok(stats) => {
            for note in &stats.notes {
                diagnostics.push(Diagnostic::note("N600", "", note.clone()));
            }
            if let Some((target, max)) = size_limit {
                let written = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
                if written > *max {
                    diagnostics.push(
                        Diagnostic::warning(
                            "W412",
                            "",
                            format!(
                                "the output is {}; {target} allows {}",
                                targets::human_size(written),
                                targets::human_size(*max)
                            ),
                        )
                        .with_help("a lower --quality, a smaller --budget or a shorter video brings it down"),
                    );
                }
            }
            let result = serde_json::json!({
                "ok": true,
                "output": output,
                "mode": stats.mode.as_str(),
                "frames": stats.frames,
                "duration": stats.duration,
                "seconds": stats.seconds,
            });
            if format == Format::Human {
                report(&diagnostics, format, None)?;
                if stats.mode == media::RenderMode::Copy {
                    println!(
                        "wrote {} ({}s, streams copied without re-encoding, {:.1}s elapsed)",
                        output.display(),
                        stats.duration,
                        stats.seconds
                    );
                } else if stats.frames == 0 {
                    println!(
                        "wrote {} ({}s of audio, {:.1}s elapsed)",
                        output.display(),
                        stats.duration,
                        stats.seconds
                    );
                } else {
                    println!(
                        "wrote {} ({} frames, {}s of video, {:.1}s elapsed)",
                        output.display(),
                        stats.frames,
                        stats.duration,
                        stats.seconds
                    );
                }
            } else {
                report(&diagnostics, format, Some(result))?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(err) => {
            diagnostics.push(render_diagnostic(&err));
            report(
                &diagnostics,
                format,
                Some(serde_json::json!({ "ok": false })),
            )?;
            Ok(ExitCode::from(EXIT_RENDER))
        }
    }
}

fn render_diagnostic(err: &RenderError) -> Diagnostic {
    let path = match err {
        RenderError::Unsupported { path, .. } => path.clone(),
        _ => String::new(),
    };
    Diagnostic::error(err.code(), path, err.to_string())
}

fn write_png(frame: &Frame, path: &Path) -> Result<()> {
    let png = frame.to_png().context("encoding PNG")?;
    std::fs::write(path, png).with_context(|| format!("writing {}", path.display()))
}

/// Reads and resolves a timeline file, probing media assets for their
/// lengths when `probe` is set and media support is available.
fn load_timeline(args: &TimelineArgs, probe: bool) -> Result<Loaded> {
    let text = std::fs::read_to_string(&args.timeline)
        .with_context(|| format!("reading {}", args.timeline.display()))?;
    Ok(load_text(&text, &args.root(), probe))
}

/// Resolves timeline text whose asset paths are relative to `root`.
fn load_text(text: &str, root: &Path, probe: bool) -> Loaded {
    if !probe {
        return geneva_timeline::load(text);
    }
    let info = media::probe_assets(text, root);
    let mut loaded = geneva_timeline::load_with(text, &info);
    loaded.diagnostics.extend(info.diagnostics());
    loaded.diagnostics.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.path.cmp(&b.path))
    });
    if loaded.diagnostics.iter().any(Diagnostic::is_error) {
        loaded.composition = None;
    }
    loaded
}

fn timeline_dir(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Prints diagnostics. In JSON mode the whole result is one document on
/// stdout; in human mode diagnostics go to stderr so stdout stays clean.
fn report(
    diagnostics: &[Diagnostic],
    format: Format,
    result: Option<serde_json::Value>,
) -> Result<()> {
    let summary = summarize(diagnostics);
    match format {
        Format::Json => {
            let mut doc = result.unwrap_or_else(|| serde_json::json!({ "ok": summary.is_ok() }));
            doc["diagnostics"] = serde_json::to_value(diagnostics)?;
            doc["summary"] = serde_json::to_value(summary)?;
            println!("{}", serde_json::to_string_pretty(&doc)?);
        }
        Format::Human => {
            for d in diagnostics {
                eprint!("{d}");
            }
            if diagnostics.is_empty() {
                eprintln!("ok: no problems found");
            } else {
                let mut parts = Vec::new();
                for (n, label) in [
                    (summary.errors, "error"),
                    (summary.warnings, "warning"),
                    (summary.notes, "note"),
                ] {
                    if n > 0 {
                        parts.push(format!("{n} {label}{}", if n == 1 { "" } else { "s" }));
                    }
                }
                eprintln!(
                    "{}: {}",
                    if summary.is_ok() { "ok" } else { "invalid" },
                    parts.join(", ")
                );
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn _assert_composition_is_used(_: &Composition) {}
