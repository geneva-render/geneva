//! The `geneva` command-line tool.

#![forbid(unsafe_code)]

mod guide;
mod media;
mod targets;
mod verbs;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use geneva_render::{RenderError, Renderer};
use geneva_timeline::{Composition, Diagnostic, Loaded, Ratio, Time, summarize};

/// Exit status when the timeline has validation errors.
const EXIT_INVALID: u8 = 1;
/// Exit status when rendering failed.
const EXIT_RENDER: u8 = 3;

#[derive(Parser)]
#[command(
    name = "geneva",
    version,
    about = "Composition and rendering engine for video",
    long_about = None,
    after_help = "Writing a timeline from a program or an agent: `geneva guide`.\n\
                  What a diagnostic means: `geneva explain E302`."
)]
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

impl Format {
    /// How a long run reports its progress in this format.
    fn progress(self) -> media::ProgressFormat {
        match self {
            Self::Human => media::ProgressFormat::Human,
            Self::Json => media::ProgressFormat::Json,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Check a timeline and report every problem with its location and a fix.
    Validate(ValidateArgs),
    /// Write one frame of a timeline, or one still of a video file, as a
    /// picture.
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
    /// Print the manual: how to drive geneva from a program, or one of
    /// the other pages.
    Guide(GuideArgs),
    /// Say what a diagnostic code means, such as E302.
    Explain(ExplainArgs),
}

#[derive(Args)]
struct GuideArgs {
    /// Which page to print. Defaults to the guide for programs and
    /// agents; `--list` names the rest.
    #[arg(value_name = "TOPIC")]
    topic: Option<String>,
    /// Name the pages instead of printing one.
    #[arg(long)]
    list: bool,
}

#[derive(Args)]
struct ExplainArgs {
    /// The code, such as E302. Case does not matter.
    #[arg(value_name = "CODE", required_unless_present = "list")]
    code: Option<String>,
    /// Print every documented code instead of one.
    #[arg(long)]
    list: bool,
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
    /// A timeline JSON file, or a video file.
    input: PathBuf,
    /// Directory that a timeline's asset paths are relative to. Defaults
    /// to the timeline's directory.
    #[arg(long, value_name = "DIR")]
    assets: Option<PathBuf>,
    /// Time of the frame, for example 1.5, "1.5s", "45f" or "00:00:01.5".
    /// A timeline defaults to 0. A video file defaults to a chosen frame:
    /// the first clear one after the opening, not dark, past the first
    /// motion, so a black leader or a phone clip's opening blur is
    /// skipped.
    #[arg(long, value_name = "TIME", conflicts_with = "frame")]
    at: Option<String>,
    /// Frame number instead of a time, starting at 0.
    #[arg(long, value_name = "N")]
    frame: Option<u64>,
    /// Where to write the picture: .png, or .jpg for JPEG.
    #[arg(short, long, value_name = "FILE", default_value = "frame.png")]
    output: PathBuf,
    /// Picture width in pixels; the frame's by default. The height
    /// follows when not given.
    #[arg(long)]
    width: Option<u32>,
    /// Picture height in pixels. The width follows when not given.
    #[arg(long)]
    height: Option<u32>,
    /// With a video file: print the compiled timeline instead of
    /// rendering.
    #[arg(long)]
    show_timeline: bool,
}

#[derive(Args)]
struct RenderArgs {
    #[command(flatten)]
    timeline: TimelineArgs,
    /// Output file; the extension selects the container unless the
    /// timeline sets one. When the timeline has `outputs`, a directory
    /// that every entry is written into.
    #[arg(short, long, value_name = "FILE|DIR")]
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
    /// Which renderer composites the frames: auto (the GPU where one
    /// can be used, otherwise the CPU), cpu, or gpu (the CPU when no
    /// device opens, with a note).
    #[arg(long, value_enum, default_value = "auto")]
    renderer: media::RendererChoice,
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
    /// Keep one rectangle of the picture: X,Y,WxH, or WxH for the middle
    /// (pixels or percentages, for example 240,0,1440x1080 or 56%x100%).
    /// The output takes its size unless --width or --height is given.
    #[arg(long, value_name = "RECT")]
    crop: Option<verbs::CropArg>,
    /// Play the source this many times faster (2) or slower (0.5); the
    /// pitch follows.
    #[arg(long, value_name = "FACTOR")]
    speed: Option<f64>,
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
    /// Keep one rectangle of the picture first: X,Y,WxH, or WxH for the
    /// middle (pixels or percentages).
    #[arg(long, value_name = "RECT")]
    crop: Option<verbs::CropArg>,
    /// Output width in pixels. The height follows when not given.
    #[arg(long, required_unless_present_any = ["height", "crop"])]
    width: Option<u32>,
    /// Output height in pixels. The width follows when not given.
    #[arg(long, required_unless_present_any = ["width", "crop"])]
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
    /// Keep one rectangle of the picture: X,Y,WxH, or WxH for the middle
    /// (pixels or percentages). The output takes its size.
    #[arg(long, value_name = "RECT")]
    crop: Option<verbs::CropArg>,
    /// Play the kept range this many times faster (2) or slower (0.5);
    /// the pitch follows.
    #[arg(long, value_name = "FACTOR")]
    speed: Option<f64>,
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
    /// Cross-fade between inputs over this long instead of cutting. Both
    /// clips are visible at once and the sound crosses at constant power.
    #[arg(long, value_name = "TIME", conflicts_with = "fade")]
    crossfade: Option<String>,
    /// Dip through a color between inputs over this long instead of
    /// cutting. Only one clip is visible at a time and the sound goes to
    /// silence and back.
    #[arg(long, value_name = "TIME")]
    fade: Option<String>,
    /// The color --fade dips through. Black by default.
    #[arg(long, value_name = "COLOR", requires = "fade")]
    fade_color: Option<String>,
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
    /// With --extract: write 16 kHz mono, what speech recognizers want.
    #[arg(long, requires = "extract")]
    speech: bool,
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
    /// Caption file to draw into the picture: .srt, .vtt, or the .json a
    /// speech recogniser writes, whose word times allow --highlight.
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
    /// Color for the word being said, which needs a --burn file with word
    /// times in it.
    #[arg(long, value_name = "COLOR", requires = "burn")]
    highlight: Option<String>,
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
            let loaded = load_timeline(&args.timeline, args.probe, args.probe)?;
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
            let is_timeline = args
                .input
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("json"));
            if !is_timeline {
                // A video file: the frame is an entry of a one-clip
                // document, chosen when no time is given.
                let at = match (&args.at, args.frame) {
                    (Some(text), _) => parse_time("--at", Some(text))?,
                    (None, Some(n)) => parse_time("--frame", Some(&format!("{n}f")))?,
                    (None, None) => None,
                };
                let compiled = verbs::still(&args.input, at, args.width, args.height)?;
                let encode = verbs::EncodeArgs {
                    show_timeline: args.show_timeline,
                    ..Default::default()
                };
                return run_verb(&compiled, &args.output, &encode, cli.format);
            }
            let timeline = TimelineArgs {
                timeline: args.input.clone(),
                assets: args.assets.clone(),
            };
            let loaded = load_timeline(&timeline, true, false)?;
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
            let mut renderer = media::renderer(timeline.root());
            let mut diagnostics = loaded.diagnostics.clone();
            match renderer.render_frame(comp, time) {
                Ok(frame) => {
                    let (width, height) =
                        picture_size(comp.width, comp.height, args.width, args.height);
                    let rgba = media::scale_picture(
                        &frame.to_rgba8(),
                        comp.width,
                        comp.height,
                        (width, height),
                    );
                    media::write_picture(&rgba, width, height, &args.output)
                        .with_context(|| format!("writing {}", args.output.display()))?;
                    let frame_index = (time * comp.fps).floor();
                    let result = serde_json::json!({
                        "ok": true,
                        "output": args.output,
                        "content_type": media::content_type_for(&args.output),
                        "time": time.to_string(),
                        "frame": frame_index,
                        "width": width,
                        "height": height,
                    });
                    if cli.format == Format::Human {
                        report(&diagnostics, cli.format, None)?;
                        println!(
                            "wrote {} ({}×{}, frame {} at {}s)",
                            args.output.display(),
                            width,
                            height,
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
            let mut loaded = load_timeline(&args.timeline, true, false)?;
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
                        false,
                        false,
                    )?;
                    size_limit = limit;
                    let text = serde_json::to_string_pretty(&tl)?;
                    loaded = load_text(&text, &args.timeline.root(), true, false);
                    loaded.diagnostics.extend(extra);
                }
            }
            let overrides = media::RenderOverrides {
                renderer: args.renderer,
                crf: args.crf,
                preset: args.preset.clone(),
                no_audio: args.no_audio,
                exact: args.exact,
                picture_as_is: false,
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
                args.crop.as_ref(),
                args.speed,
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
                args.crop.as_ref(),
                None,
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
                args.crop.as_ref(),
                args.speed,
                &args.encode,
            )?;
            run_verb(&compiled, &args.output, &args.encode, cli.format)
        }
        Command::Concat(args) => {
            let transition = match (&args.crossfade, &args.fade) {
                (Some(_), _) => parse_time("--crossfade", args.crossfade.as_deref())?
                    .map(|d| (verbs::TransitionKind::Crossfade, d)),
                (_, Some(_)) => parse_time("--fade", args.fade.as_deref())?
                    .map(|d| (verbs::TransitionKind::Fade, d)),
                _ => None,
            };
            let join = verbs::Join {
                transition,
                color: args.fade_color.clone(),
            };
            let compiled = verbs::concat(&args.inputs, &join, &args.encode)?;
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
            let compiled = verbs::audio(&args.input, &op, args.speech, &args.encode)?;
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
        Command::Guide(args) => {
            print_guide(&args, cli.format)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Explain(args) => {
            print_explanation(&args, cli.format)?;
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
                            "content_type": media::content_type_for(&args.output),
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
                    highlight: args.highlight.clone(),
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
    // A verb that writes one file of a multi-output document names the
    // file; the document's directory is the file's, and the entry takes
    // its name.
    let mut output = output.to_path_buf();
    if timeline.outputs.len() == 1 && output.extension().is_some() {
        let file = output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for spec in timeline.outputs.values_mut() {
            spec.path = Some(file.clone());
        }
        output = timeline_dir(&output);
    }
    let output = output.as_path();
    let mut picture_as_is = false;
    if encode.denoise {
        // Into the document, where a render reads it from; never part of
        // a target.
        let audio = timeline
            .output
            .audio
            .get_or_insert(geneva_timeline::schema::AudioOutput {
                sample_rate: None,
                channels: None,
                loudness: None,
                hygiene: None,
                denoise: None,
            });
        audio.denoise = Some(true);
    }
    if let Some(name) = &encode.for_ {
        // A first resolution gives the facts the target needs (the size
        // and length the verb arrived at); the target then rewrites the
        // output block and the timeline is resolved again. A source that
        // already fits the target, used as it is, is copied instead: the
        // target would only force a re-encode to the same thing.
        let text = serde_json::to_string_pretty(&timeline)?;
        let first = load_text(&text, &compiled.root, true, false);
        let verdict = fits_target_as_is(&first, &compiled.root, name, encode, output)?;
        if let Some(note) = verdict.as_is {
            extra.push(Diagnostic::note("N600", "", note));
        } else {
            // A target that only wants the sound changed leaves the
            // picture alone: the quality it writes is what to encode
            // with, not a reason to encode.
            picture_as_is = verdict.picture_as_is;
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
                // The blurred copy is what a reframe wants, so it is
                // what a portrait target does unless bars are asked for.
                encode.fill != Some(verbs::FillArg::Bars),
                picture_as_is,
            )?;
            extra.extend(notes);
            size_limit = limit;
        }
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
    let mut loaded = load_text(&text, &compiled.root, true, false);
    loaded.diagnostics.extend(extra);
    let overrides = media::RenderOverrides {
        renderer: encode.renderer,
        crf: encode.crf,
        preset: encode.preset.clone(),
        no_audio: encode.no_audio,
        exact: encode.exact,
        picture_as_is,
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
#[allow(
    clippy::fn_params_excessive_bools,
    reason = "each is a separate switch of the verb that asked for the target"
)]
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
    fill_blur: bool,
    picture_as_is: bool,
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
        fill_blur,
        picture_as_is,
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
    if !comp.outputs.is_empty() {
        return render_outputs_to(loaded, comp, root, output, overrides, format, size_limit);
    }
    let mut diagnostics = loaded.diagnostics.clone();
    let progress = media::Progress::new(format.progress());
    match media::render(comp, root, output, overrides, &progress) {
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
                "content_type": media::content_type_for(output),
                "mode": stats.mode.as_str(),
                "frames": stats.frames,
                "duration": stats.duration,
                "seconds": stats.seconds,
            });
            if format == Format::Human {
                report(&diagnostics, format, None)?;
                if stats.mode == media::RenderMode::CopyPicture {
                    println!(
                        "wrote {} ({}s, picture copied without re-encoding, sound encoded, {:.1}s elapsed)",
                        output.display(),
                        stats.duration,
                        stats.seconds
                    );
                } else if stats.mode == media::RenderMode::Copy {
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

/// Renders every entry of a timeline's `outputs` into the directory `dir`
/// in one pass and reports each file written.
fn render_outputs_to(
    loaded: &Loaded,
    comp: &Composition,
    root: &Path,
    dir: &Path,
    overrides: &media::RenderOverrides,
    format: Format,
    size_limit: Option<&(String, u64)>,
) -> Result<ExitCode> {
    let mut diagnostics = loaded.diagnostics.clone();
    let progress = media::Progress::new(format.progress());
    match media::render_outputs(comp, root, dir, overrides, &progress) {
        Ok((outputs, stats)) => {
            for note in &stats.notes {
                diagnostics.push(Diagnostic::note("N600", "", note.clone()));
            }
            if let Some((target, max)) = size_limit {
                for o in outputs
                    .iter()
                    .filter(|o| o.kind == "video" && o.bytes > *max)
                {
                    diagnostics.push(
                        Diagnostic::warning(
                            "W412",
                            "",
                            format!(
                                "{} is {}; {target} allows {}",
                                o.path.display(),
                                targets::human_size(o.bytes),
                                targets::human_size(*max)
                            ),
                        )
                        .with_help("a lower --quality, a smaller --budget or a shorter video brings it down"),
                    );
                }
            }
            let result = serde_json::json!({
                "ok": true,
                "directory": dir,
                "outputs": outputs,
                "frames": stats.frames,
                "duration": stats.duration,
                "seconds": stats.seconds,
            });
            if format == Format::Human {
                report(&diagnostics, format, None)?;
                for o in &outputs {
                    let how = match o.mode {
                        "copy" => ", streams copied without re-encoding".to_owned(),
                        "copy-picture" => {
                            ", picture copied without re-encoding, sound encoded".to_owned()
                        }
                        "render" => String::new(),
                        other => format!(", {other}"),
                    };
                    println!(
                        "wrote {} ({}, {}{how})",
                        o.path.display(),
                        o.kind.replace('-', " "),
                        targets::human_size(o.bytes)
                    );
                }
                println!(
                    "{} output{} from {} frame{}, {}s of video, {:.1}s elapsed",
                    outputs.len(),
                    if outputs.len() == 1 { "" } else { "s" },
                    stats.frames,
                    if stats.frames == 1 { "" } else { "s" },
                    stats.duration,
                    stats.seconds
                );
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

/// Whether a `--for` target is already met by the sources as the verb
/// uses them, so a stream copy gives what the target would encode. The
/// composition must be copyable (the planner's judgement) and every
/// source must fit the target's codec, size, rate and bitrate caps;
/// any quality ask on the command line re-encodes regardless. Returns
/// the note to report.
/// What a `--for` target has to do to the sources it was given.
#[derive(Default)]
struct TargetVerdict {
    /// Nothing: every source already fits, so the files are copied.
    /// Carries the note that says so.
    as_is: Option<String>,
    /// Nothing to the picture, which is copied while the sound is
    /// brought to the target's loudness or cleaned and encoded beside
    /// it. False when the picture has to be re-encoded anyway.
    picture_as_is: bool,
}

fn fits_target_as_is(
    first: &Loaded,
    root: &Path,
    target_name: &str,
    encode: &verbs::EncodeArgs,
    output: &Path,
) -> Result<TargetVerdict> {
    let asks_encode = encode.crf.is_some()
        || encode.preset.is_some()
        || encode.codec.is_some()
        || encode.profile.is_some()
        || encode.tune.is_some()
        || encode.keyframe_interval.is_some()
        || encode.quality.is_some()
        || encode.budget.is_some()
        || encode.exact
        || encode.keep_hdr
        || encode.denoise
        || encode.fill.is_some();
    if asks_encode {
        return Ok(TargetVerdict::default());
    }
    let Some(target) = targets::find(target_name) else {
        return Ok(TargetVerdict::default());
    };
    let Some(comp) = &first.composition else {
        return Ok(TargetVerdict::default());
    };
    let Some(sources) = media::copy_sources(comp, root, output)? else {
        return Ok(TargetVerdict::default());
    };
    let mut facts = Vec::new();
    // The picture of every source has to need nothing, since one output
    // carries one picture.
    let mut picture_as_is = true;
    let mut whole = true;
    for path in &sources {
        let fit = verbs::fits_target(path, target)?;
        picture_as_is &= fit.picture;
        match fit.whole {
            Some(f) => facts.push(format!(
                "{} ({f})",
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned()
                )
            )),
            None => whole = false,
        }
    }
    if !whole {
        return Ok(TargetVerdict {
            as_is: None,
            picture_as_is,
        });
    }
    Ok(TargetVerdict {
        as_is: Some(format!(
            "{} already fits {target_name}: copied, not re-encoded",
            facts.join(", ")
        )),
        picture_as_is,
    })
}

/// A picture size from a frame size and an optional width or height,
/// keeping the aspect when one is missing.
fn picture_size(width: u32, height: u32, want_w: Option<u32>, want_h: Option<u32>) -> (u32, u32) {
    match (want_w, want_h) {
        (None, None) => (width, height),
        (Some(w), Some(h)) => (w.max(1), h.max(1)),
        (Some(w), None) => (
            w.max(1),
            ((f64::from(w) * f64::from(height) / f64::from(width.max(1))).round() as u32).max(1),
        ),
        (None, Some(h)) => (
            ((f64::from(h) * f64::from(width) / f64::from(height.max(1))).round() as u32).max(1),
            h.max(1),
        ),
    }
}

fn render_diagnostic(err: &RenderError) -> Diagnostic {
    let path = match err {
        RenderError::Unsupported { path, .. } => path.clone(),
        _ => String::new(),
    };
    Diagnostic::error(err.code(), path, err.to_string())
}

/// Reads and resolves a timeline file, probing media assets for their
/// lengths when `probe` is set and media support is available.
fn load_timeline(args: &TimelineArgs, probe: bool, measure: bool) -> Result<Loaded> {
    let text = std::fs::read_to_string(&args.timeline)
        .with_context(|| format!("reading {}", args.timeline.display()))?;
    Ok(load_text(&text, &args.root(), probe, measure))
}

/// Resolves timeline text whose asset paths are relative to `root`.
/// `measure` also decodes the audio of every asset for its report.
fn load_text(text: &str, root: &Path, probe: bool, measure: bool) -> Loaded {
    let mut loaded = if probe {
        let info = media::probe_assets(text, root, measure);
        let mut loaded = geneva_timeline::load_with(text, &info);
        loaded.diagnostics.extend(info.diagnostics());
        loaded
    } else {
        geneva_timeline::load_with(text, &media::ProbedAssets::fonts_only())
    };
    loaded.diagnostics.extend(unbuilt_features(&loaded));
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

/// Errors for what the document asks for and this binary was not built
/// with, reported before anything is rendered rather than when the mix
/// reaches the missing part.
fn unbuilt_features(loaded: &Loaded) -> Vec<Diagnostic> {
    #[cfg(feature = "denoise")]
    {
        let _ = loaded;
        Vec::new()
    }
    #[cfg(not(feature = "denoise"))]
    {
        const MESSAGE: &str = "speech denoising is not built into this binary; build one with `--features geneva-cli/denoise`";
        let Some(comp) = &loaded.composition else {
            return Vec::new();
        };
        let asks = |a: Option<&geneva_timeline::schema::AudioOutput>| {
            a.and_then(|a| a.denoise) == Some(true)
        };
        // A single-output document leaves `outputs` empty and carries
        // its audio on the composition.
        if comp.outputs.is_empty() {
            return if asks(comp.audio_output.as_ref()) {
                vec![Diagnostic::error("E424", "/output/audio/denoise", MESSAGE)]
            } else {
                Vec::new()
            };
        }
        comp.outputs
            .iter()
            .filter(|o| asks(o.audio.as_ref()))
            .map(|o| {
                Diagnostic::error(
                    "E424",
                    format!("/outputs/{}/audio/denoise", o.name),
                    MESSAGE,
                )
            })
            .collect()
    }
}

/// Prints a page of the manual, or names the pages.
///
/// Under `--format json` the page comes back as one document, as every
/// other command's result does, so a caller reading stdout as JSON is
/// not surprised by Markdown.
fn print_guide(args: &GuideArgs, format: Format) -> Result<()> {
    if args.list {
        let topics: Vec<serde_json::Value> = guide::TOPICS
            .iter()
            .map(|t| serde_json::json!({ "topic": t.name, "about": t.about }))
            .collect();
        match format {
            Format::Json => println!("{}", serde_json::to_string_pretty(&topics)?),
            Format::Human => {
                let width = guide::TOPICS
                    .iter()
                    .map(|t| t.name.len())
                    .max()
                    .unwrap_or(0);
                for t in guide::TOPICS {
                    println!("{:width$}  {}", t.name, t.about);
                }
                println!("\nPrint one with `geneva guide <topic>`.");
            }
        }
        return Ok(());
    }
    let name = args.topic.as_deref().unwrap_or(guide::TOPICS[0].name);
    let topic = guide::topic(name)?;
    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "topic": topic.name,
                "about": topic.about,
                "text": topic.text,
            }))?
        ),
        Format::Human => print!("{}", topic.text),
    }
    Ok(())
}

/// Says what one diagnostic code means, or lists them all.
fn print_explanation(args: &ExplainArgs, format: Format) -> Result<()> {
    let entries = match &args.code {
        Some(code) => {
            let found = guide::explain(code);
            if found.is_empty() {
                anyhow::bail!(
                    "no diagnostic {}; `geneva explain --list` names every code",
                    code.trim().to_ascii_uppercase()
                );
            }
            found
        }
        None => guide::entries(),
    };
    match format {
        Format::Json => {
            let rows: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "code": e.code,
                        "severity": e.severity,
                        "section": e.section,
                        "meaning": e.meaning,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&rows)?);
        }
        Format::Human if args.list => {
            for e in &entries {
                println!("{}  {}", e.code, e.meaning);
            }
        }
        Format::Human => {
            for e in &entries {
                println!("{}[{}]: {}", e.severity, e.code, e.meaning);
                println!("  in {}", e.section);
            }
            println!("\nThe whole page: `geneva guide errors`.");
        }
    }
    Ok(())
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
