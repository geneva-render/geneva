//! Everyday tasks as one-line commands.
//!
//! Every verb builds a timeline document and hands it to the same render
//! path a timeline file would take, so the verbs and the format are one
//! engine. `--show-timeline` prints the document instead of rendering it.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use geneva_timeline::schema::{
    Asset, AudioClip, AudioTrack, Clip, Encode, Fit, Layer, Output, Source, Timeline, Transform,
    Transition, TransitionKind, VideoCodec, VideoEncode,
};
use geneva_timeline::{Animated, Fps, Length, Point, Ratio, Scale, Time};

use crate::media;

/// What every verb needs to know about an input file.
#[derive(Debug, Clone)]
pub struct Input {
    pub width: u32,
    pub height: u32,
    pub fps: Ratio,
    pub duration: Option<Ratio>,
    pub has_video: bool,
    pub has_audio: bool,
}

impl Input {
    pub fn probe(path: &Path) -> Result<Self> {
        let info = media::probe(path)?;
        let (width, height, fps) = match &info.video {
            Some(v) => (v.width, v.height, v.fps),
            None => (0, 0, Ratio::from_int(30)),
        };
        let duration = info
            .video
            .as_ref()
            .and_then(|v| v.duration)
            .or(info.duration)
            .or_else(|| info.audio.as_ref().and_then(|a| a.duration));
        Ok(Self {
            width,
            height,
            fps,
            duration,
            has_video: info.video.is_some(),
            has_audio: info.audio.is_some(),
        })
    }
}

/// Encoder options shared by every verb.
#[derive(Args, Debug, Clone, Default)]
pub struct EncodeArgs {
    /// Constant-quality level; lower is better. Setting it forces a
    /// re-encode even when the streams could be copied.
    #[arg(long)]
    pub crf: Option<u8>,
    /// Encoder speed preset, from ultrafast to veryslow.
    #[arg(long)]
    pub preset: Option<String>,
    /// Video codec for the output. Defaults to the container's usual codec.
    #[arg(long, value_enum)]
    pub codec: Option<CodecArg>,
    /// Write no audio track.
    #[arg(long)]
    pub no_audio: bool,
    /// Always decode and re-encode, for frame-accurate cuts.
    #[arg(long)]
    pub exact: bool,
    /// Print the compiled timeline as JSON instead of rendering.
    #[arg(long)]
    pub show_timeline: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CodecArg {
    H264,
    H265,
    Vp9,
    Av1,
}

impl From<CodecArg> for VideoCodec {
    fn from(c: CodecArg) -> Self {
        match c {
            CodecArg::H264 => Self::H264,
            CodecArg::H265 => Self::H265,
            CodecArg::Vp9 => Self::Vp9,
            CodecArg::Av1 => Self::Av1,
        }
    }
}

/// How a source is fitted into a differently shaped output.
#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum FitArg {
    /// Show the whole picture, with bars if the shape differs.
    #[default]
    Contain,
    /// Fill the frame, cropping the edges if the shape differs.
    Cover,
    /// Stretch to the frame.
    Fill,
}

impl From<FitArg> for Fit {
    fn from(f: FitArg) -> Self {
        match f {
            FitArg::Contain => Self::Contain,
            FitArg::Cover => Self::Cover,
            FitArg::Fill => Self::Fill,
        }
    }
}

/// A timeline plus the directory its asset paths are relative to.
pub struct Compiled {
    pub timeline: Timeline,
    pub root: PathBuf,
}

/// Rounds a dimension to the nearest even number, as most codecs need.
pub fn even(v: u32) -> u32 {
    if v % 2 == 1 { v + 1 } else { v }.max(2)
}

/// Picks output dimensions from a source size and optional requests,
/// keeping the aspect ratio when only one side is given.
pub fn output_size(src_w: u32, src_h: u32, width: Option<u32>, height: Option<u32>) -> (u32, u32) {
    match (width, height) {
        (Some(w), Some(h)) => (even(w), even(h)),
        (Some(w), None) => (
            even(w),
            even((f64::from(w) * f64::from(src_h) / f64::from(src_w.max(1))).round() as u32),
        ),
        (None, Some(h)) => (
            even((f64::from(h) * f64::from(src_w) / f64::from(src_h.max(1))).round() as u32),
            even(h),
        ),
        (None, None) => (even(src_w), even(src_h)),
    }
}

/// The deepest directory containing every input, and each input's path
/// relative to it, so a timeline can reference them without `..`.
fn common_root(paths: &[PathBuf]) -> Result<(PathBuf, Vec<String>)> {
    let absolute: Vec<PathBuf> = paths
        .iter()
        .map(|p| {
            p.canonicalize()
                .with_context(|| format!("resolving {}", p.display()))
        })
        .collect::<Result<_>>()?;
    let mut root: Vec<Component> = absolute[0]
        .parent()
        .unwrap_or(Path::new("/"))
        .components()
        .collect();
    for p in &absolute[1..] {
        let parent: Vec<Component> = p.parent().unwrap_or(Path::new("/")).components().collect();
        let shared = root
            .iter()
            .zip(parent.iter())
            .take_while(|(a, b)| a == b)
            .count();
        root.truncate(shared);
    }
    let root: PathBuf = root.iter().collect();
    if root.as_os_str().is_empty() {
        bail!("inputs must be on the same drive");
    }
    let relative = absolute
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .with_context(|| format!("{} is not under {}", p.display(), root.display()))
        })
        .collect::<Result<_>>()?;
    Ok((root, relative))
}

fn encode_block(args: &EncodeArgs) -> Option<Encode> {
    if args.crf.is_none() && args.preset.is_none() && args.codec.is_none() {
        return None;
    }
    Some(Encode {
        container: None,
        video: Some(VideoEncode {
            codec: args.codec.map(Into::into),
            crf: args.crf,
            preset: args.preset.clone(),
            hardware: None,
        }),
        audio: None,
    })
}

fn seconds(r: Ratio) -> Time {
    Time::Seconds(r)
}

fn video_clip(
    asset: &str,
    in_: Option<Time>,
    out: Option<Time>,
    audio: bool,
    fit: Option<Fit>,
) -> Clip {
    Clip {
        id: None,
        source: Source::Video {
            asset: asset.to_owned(),
            in_,
            out,
            audio: if audio { None } else { Some(false) },
        },
        start: None,
        duration: None,
        transition: None,
        fit,
        transform: None,
        opacity: None,
        blend: None,
    }
}

fn base_timeline(width: u32, height: u32, fps: Ratio, encode: Option<Encode>) -> Timeline {
    Timeline {
        geneva: geneva_timeline::FORMAT_VERSION.to_owned(),
        output: Output {
            width,
            height,
            fps: Fps(fps),
            duration: None,
            background: None,
            color: None,
            audio: None,
            encode,
        },
        assets: BTreeMap::new(),
        compositions: BTreeMap::new(),
        layers: Vec::new(),
        audio: Vec::new(),
    }
}

/// `convert` and `resize`: one input, optionally a new size.
pub fn convert(
    input: &Path,
    width: Option<u32>,
    height: Option<u32>,
    fit: FitArg,
    fps: Option<Fps>,
    args: &EncodeArgs,
) -> Result<Compiled> {
    let src = Input::probe(input)?;
    if !src.has_video {
        bail!(
            "{} has no video stream; use `geneva audio` for audio files",
            input.display()
        );
    }
    let (w, h) = output_size(src.width, src.height, width, height);
    let (root, rel) = common_root(&[input.to_owned()])?;
    let mut tl = base_timeline(w, h, fps.map_or(src.fps, |f| f.0), encode_block(args));
    tl.assets.insert(
        "in".to_owned(),
        Asset {
            src: rel[0].clone(),
            kind: None,
            color: None,
        },
    );
    let resized = (w, h) != (src.width, src.height);
    tl.layers.push(Layer {
        id: None,
        enabled: true,
        clips: vec![video_clip(
            "in",
            None,
            None,
            !args.no_audio,
            if resized { Some(fit.into()) } else { None },
        )],
    });
    Ok(Compiled { timeline: tl, root })
}

/// `trim`: one input, a range.
pub fn trim(
    input: &Path,
    from: Option<Time>,
    to: Option<Time>,
    duration: Option<Time>,
    args: &EncodeArgs,
) -> Result<Compiled> {
    let src = Input::probe(input)?;
    let (root, rel) = common_root(&[input.to_owned()])?;
    let from_secs = from.map_or(Ratio::ZERO, |t| t.resolve(src.fps));
    let out = match (to, duration) {
        (Some(t), _) => Some(t),
        (None, Some(d)) => Some(seconds(from_secs + d.resolve(src.fps))),
        (None, None) => None,
    };
    let mut tl = base_timeline(
        even(src.width),
        even(src.height),
        src.fps,
        encode_block(args),
    );
    tl.assets.insert(
        "in".to_owned(),
        Asset {
            src: rel[0].clone(),
            kind: None,
            color: None,
        },
    );
    tl.layers.push(Layer {
        id: None,
        enabled: true,
        clips: vec![video_clip(
            "in",
            from.map(|_| seconds(from_secs)),
            out,
            !args.no_audio,
            None,
        )],
    });
    Ok(Compiled { timeline: tl, root })
}

/// `concat`: inputs back to back, optionally cross-faded.
pub fn concat(inputs: &[PathBuf], crossfade: Option<Time>, args: &EncodeArgs) -> Result<Compiled> {
    if inputs.len() < 2 {
        bail!("concat needs at least two inputs");
    }
    let probed: Vec<Input> = inputs
        .iter()
        .map(|p| Input::probe(p))
        .collect::<Result<_>>()?;
    let first = &probed[0];
    let (root, rel) = common_root(inputs)?;
    let mut tl = base_timeline(
        even(first.width),
        even(first.height),
        first.fps,
        encode_block(args),
    );
    let mut clips = Vec::new();
    for (i, (src, path)) in probed.iter().zip(rel.iter()).enumerate() {
        let id = format!("in{}", i + 1);
        tl.assets.insert(
            id.clone(),
            Asset {
                src: path.clone(),
                kind: None,
                color: None,
            },
        );
        let same_shape = (src.width, src.height) == (first.width, first.height);
        let mut clip = video_clip(
            &id,
            None,
            None,
            !args.no_audio,
            if same_shape { None } else { Some(Fit::Contain) },
        );
        if i > 0 {
            if let Some(d) = crossfade {
                clip.transition = Some(Transition {
                    kind: TransitionKind::Crossfade,
                    duration: d,
                });
            }
        }
        clips.push(clip);
    }
    tl.layers.push(Layer {
        id: None,
        enabled: true,
        clips,
    });
    Ok(Compiled { timeline: tl, root })
}

/// Named positions for overlays.
#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum Corner {
    #[default]
    TopRight,
    TopLeft,
    BottomLeft,
    BottomRight,
    Center,
}

/// Where and when an overlay is shown.
#[derive(Debug, Clone)]
pub struct Placement {
    pub corner: Corner,
    /// Distance from the edges in pixels.
    pub margin: f64,
    pub scale: f64,
    pub opacity: f64,
    pub start: Option<Time>,
    pub duration: Option<Time>,
}

/// `overlay`: an image or video placed over the input.
pub fn overlay(
    input: &Path,
    overlay: &Path,
    placement: &Placement,
    args: &EncodeArgs,
) -> Result<Compiled> {
    let Placement {
        corner,
        margin,
        scale,
        opacity,
        start,
        duration,
    } = placement.clone();
    let src = Input::probe(input)?;
    let (root, rel) = common_root(&[input.to_owned(), overlay.to_owned()])?;
    let mut tl = base_timeline(
        even(src.width),
        even(src.height),
        src.fps,
        encode_block(args),
    );
    // The overlay is open-ended by default, so the output length comes
    // from the input rather than from the clips.
    tl.output.duration = src.duration.map(seconds);
    tl.assets.insert(
        "in".to_owned(),
        Asset {
            src: rel[0].clone(),
            kind: None,
            color: None,
        },
    );
    tl.assets.insert(
        "overlay".to_owned(),
        Asset {
            src: rel[1].clone(),
            kind: None,
            color: None,
        },
    );
    tl.layers.push(Layer {
        id: None,
        enabled: true,
        clips: vec![video_clip("in", None, None, !args.no_audio, None)],
    });
    let is_video = overlay
        .extension()
        .and_then(|e| e.to_str())
        .and_then(geneva_timeline::schema::AssetKind::from_extension)
        == Some(geneva_timeline::schema::AssetKind::Video);
    let source = if is_video {
        Source::Video {
            asset: "overlay".to_owned(),
            in_: None,
            out: None,
            audio: Some(false),
        }
    } else {
        Source::Image {
            asset: "overlay".to_owned(),
        }
    };
    let pct = |v: f64| Length::Percent(v);
    let px = |v: f64| Length::Px(v);
    let (anchor, position) = match corner {
        Corner::TopLeft => (
            Point {
                x: pct(0.0),
                y: pct(0.0),
            },
            Point {
                x: px(margin),
                y: px(margin),
            },
        ),
        Corner::TopRight => (
            Point {
                x: pct(100.0),
                y: pct(0.0),
            },
            Point {
                x: px(f64::from(even(src.width)) - margin),
                y: px(margin),
            },
        ),
        Corner::BottomLeft => (
            Point {
                x: pct(0.0),
                y: pct(100.0),
            },
            Point {
                x: px(margin),
                y: px(f64::from(even(src.height)) - margin),
            },
        ),
        Corner::BottomRight => (
            Point {
                x: pct(100.0),
                y: pct(100.0),
            },
            Point {
                x: px(f64::from(even(src.width)) - margin),
                y: px(f64::from(even(src.height)) - margin),
            },
        ),
        Corner::Center => (
            Point {
                x: pct(50.0),
                y: pct(50.0),
            },
            Point {
                x: pct(50.0),
                y: pct(50.0),
            },
        ),
    };
    tl.layers.push(Layer {
        id: None,
        enabled: true,
        clips: vec![Clip {
            id: None,
            source,
            start,
            duration,
            transition: None,
            fit: None,
            transform: Some(Transform {
                position: Some(Animated::Constant(position)),
                anchor: Some(anchor),
                scale: if (scale - 1.0).abs() < f64::EPSILON {
                    None
                } else {
                    Some(Animated::Constant(Scale { x: scale, y: scale }))
                },
                rotation: None,
            }),
            opacity: if (opacity - 1.0).abs() < f64::EPSILON {
                None
            } else {
                Some(Animated::Constant(opacity))
            },
            blend: None,
        }],
    });
    Ok(Compiled { timeline: tl, root })
}

/// What `audio` does with the input.
#[derive(Debug, Clone)]
pub enum AudioOp {
    /// Write the audio alone to an audio file.
    Extract,
    /// Drop the audio.
    Mute,
    /// Replace the audio with another file's.
    Replace(PathBuf),
    /// Mix another file's audio in at a gain in decibels.
    Mix(PathBuf, f64),
}

/// `audio`: extract, mute, replace or mix.
pub fn audio(input: &Path, op: &AudioOp, args: &EncodeArgs) -> Result<Compiled> {
    let src = Input::probe(input)?;
    let mut inputs = vec![input.to_owned()];
    if let AudioOp::Replace(p) | AudioOp::Mix(p, _) = op {
        inputs.push(p.clone());
    }
    let (root, rel) = common_root(&inputs)?;
    let (w, h) = if src.has_video {
        (even(src.width), even(src.height))
    } else {
        (2, 2)
    };
    let mut tl = base_timeline(w, h, src.fps, encode_block(args));
    tl.assets.insert(
        "in".to_owned(),
        Asset {
            src: rel[0].clone(),
            kind: None,
            color: None,
        },
    );
    let track = |asset: &str, gain: Option<f64>| AudioTrack {
        id: None,
        enabled: true,
        clips: vec![AudioClip {
            id: None,
            asset: asset.to_owned(),
            in_: None,
            out: None,
            start: None,
            duration: None,
            gain_db: gain.map(Animated::Constant),
            fade_in: None,
            fade_out: None,
        }],
    };
    // The picture sets the length; a longer replacement or mix-in is cut
    // to it rather than padding the video.
    if src.has_video && !matches!(op, AudioOp::Extract) {
        tl.output.duration = src.duration.map(seconds);
    }
    match op {
        AudioOp::Extract => {
            if !src.has_audio {
                bail!("{} has no audio stream", input.display());
            }
            tl.output.duration = src.duration.map(seconds);
            tl.audio.push(track("in", None));
        }
        AudioOp::Mute => {
            tl.layers.push(Layer {
                id: None,
                enabled: true,
                clips: vec![video_clip("in", None, None, false, None)],
            });
        }
        AudioOp::Replace(_) => {
            tl.assets.insert(
                "audio".to_owned(),
                Asset {
                    src: rel[1].clone(),
                    kind: None,
                    color: None,
                },
            );
            tl.layers.push(Layer {
                id: None,
                enabled: true,
                clips: vec![video_clip("in", None, None, false, None)],
            });
            tl.audio.push(track("audio", None));
        }
        AudioOp::Mix(_, gain) => {
            tl.assets.insert(
                "audio".to_owned(),
                Asset {
                    src: rel[1].clone(),
                    kind: None,
                    color: None,
                },
            );
            tl.layers.push(Layer {
                id: None,
                enabled: true,
                clips: vec![video_clip("in", None, None, true, None)],
            });
            tl.audio.push(track("audio", Some(*gain)));
        }
    }
    Ok(Compiled { timeline: tl, root })
}
