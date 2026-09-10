//! The `geneva` command-line tool.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use geneva_render::{CpuRenderer, Renderer};
use geneva_timeline::{Diagnostic, Loaded, Ratio, Time, load, summarize};

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
    Validate(TimelineArgs),
    /// Render one frame of a timeline to a PNG file.
    Frame(FrameArgs),
    /// Print the JSON Schema of the timeline format.
    Schema,
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
            let loaded = load_timeline(&args.timeline)?;
            report(&loaded.diagnostics, cli.format, None)?;
            Ok(if loaded.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_INVALID)
            })
        }
        Command::Frame(args) => {
            let loaded = load_timeline(&args.timeline.timeline)?;
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
            let root = args
                .timeline
                .assets
                .clone()
                .unwrap_or_else(|| timeline_dir(&args.timeline.timeline));
            let mut renderer = CpuRenderer::with_asset_root(root);
            let mut diagnostics = loaded.diagnostics.clone();
            match renderer.render_frame(comp, time) {
                Ok(frame) => {
                    let png = frame.to_png().context("encoding PNG")?;
                    std::fs::write(&args.output, png)
                        .with_context(|| format!("writing {}", args.output.display()))?;
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
                    let path = match &err {
                        geneva_render::RenderError::Unsupported { path, .. } => path.clone(),
                        _ => String::new(),
                    };
                    diagnostics.push(Diagnostic::error(err.code(), path, err.to_string()));
                    report(
                        &diagnostics,
                        cli.format,
                        Some(serde_json::json!({ "ok": false })),
                    )?;
                    Ok(ExitCode::from(EXIT_RENDER))
                }
            }
        }
    }
}

fn load_timeline(path: &Path) -> Result<Loaded> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(load(&text))
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
