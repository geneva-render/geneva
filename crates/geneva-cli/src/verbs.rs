//! Everyday tasks as one-line commands.
//!
//! Every verb builds a timeline document and hands it to the same render
//! path a timeline file would take, so the verbs and the format are one
//! engine. `--show-timeline` prints the document instead of rendering it.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use geneva_color::{ColorTags, ResolvedTags};
use geneva_timeline::schema::{
    Asset, AudioClip, AudioTrack, Clip, Encode, Fit, Layer, Output, Source, SubtitleTrack,
    TextSource, Timeline, Transform, Transition, TransitionKind, VideoCodec, VideoEncode,
    VideoProfile,
};
use geneva_timeline::{Animated, Diagnostic, Fps, Length, Point, Ratio, Scale, Time};

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
    /// The video's color encoding, as tagged or as inferred from its size.
    pub color: Option<ResolvedTags>,
}

impl Input {
    pub fn probe(path: &Path) -> Result<Self> {
        let info = media::probe(path)?;
        let (width, height, fps) = match &info.video {
            Some(v) => (v.width, v.height, v.fps),
            None => (0, 0, Ratio::from_int(30)),
        };
        let color = info
            .video
            .as_ref()
            .map(|v| geneva_color::infer(v.color, v.width, v.height).0);
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
            color,
        })
    }
}

/// The color encoding to write when every video input shares one: the
/// picture then keeps its encoding (SD material stays BT.601), the tags
/// travel into the file, and nothing is converted on the way. HDR sources
/// fall back to the SDR default, which the renderer maps down to.
fn shared_color(inputs: &[&Input]) -> Option<ColorTags> {
    let mut found: Option<ResolvedTags> = None;
    for input in inputs {
        let Some(color) = input.color else {
            continue;
        };
        match found {
            None => found = Some(color),
            Some(f) if f != color => return None,
            _ => {}
        }
    }
    found.filter(|c| !c.is_hdr()).map(ColorTags::from)
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
    /// Codec profile, for prores and dnxhd.
    #[arg(long, value_enum)]
    pub profile: Option<ProfileArg>,
    /// Write no audio track.
    #[arg(long)]
    pub no_audio: bool,
    /// Always decode and re-encode, for frame-accurate cuts.
    #[arg(long)]
    pub exact: bool,
    /// Print the compiled timeline as JSON instead of rendering.
    #[arg(long)]
    pub show_timeline: bool,
    /// Where the file is going: a device class (phone, tablet, desktop,
    /// tv, web) or a platform (youtube, instagram, tiktok, x, linkedin,
    /// email). Picks size ceiling, codec, quality, bitrate cap, keyframes,
    /// fast start and audio from a table; `geneva targets` prints it.
    #[arg(long = "for", value_name = "TARGET")]
    pub for_: Option<String>,
    /// Quality tier for --for: best, good (default) or eco.
    #[arg(long, value_enum, requires = "for_")]
    pub quality: Option<crate::targets::Quality>,
    /// Size budget for --for, such as 25MB; caps the bitrate so the file
    /// fits, and warns when the result is still larger.
    #[arg(long, value_name = "SIZE", requires = "for_")]
    pub budget: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CodecArg {
    H264,
    H265,
    Vp9,
    Av1,
    Prores,
    Dnxhd,
    Png,
    Mjpeg,
}

impl From<CodecArg> for VideoCodec {
    fn from(c: CodecArg) -> Self {
        match c {
            CodecArg::H264 => Self::H264,
            CodecArg::H265 => Self::H265,
            CodecArg::Vp9 => Self::Vp9,
            CodecArg::Av1 => Self::Av1,
            CodecArg::Prores => Self::Prores,
            CodecArg::Dnxhd => Self::Dnxhd,
            CodecArg::Png => Self::Png,
            CodecArg::Mjpeg => Self::Mjpeg,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ProfileArg {
    Proxy,
    Lt,
    Standard,
    Hq,
    #[value(name = "4444")]
    P4444,
    #[value(name = "4444-xq")]
    P4444Xq,
    DnxhrLb,
    DnxhrSq,
    DnxhrHq,
    DnxhrHqx,
    Dnxhr444,
}

impl From<ProfileArg> for VideoProfile {
    fn from(p: ProfileArg) -> Self {
        match p {
            ProfileArg::Proxy => Self::Proxy,
            ProfileArg::Lt => Self::Lt,
            ProfileArg::Standard => Self::Standard,
            ProfileArg::Hq => Self::Hq,
            ProfileArg::P4444 => Self::P4444,
            ProfileArg::P4444Xq => Self::P4444Xq,
            ProfileArg::DnxhrLb => Self::DnxhrLb,
            ProfileArg::DnxhrSq => Self::DnxhrSq,
            ProfileArg::DnxhrHq => Self::DnxhrHq,
            ProfileArg::DnxhrHqx => Self::DnxhrHqx,
            ProfileArg::Dnxhr444 => Self::Dnxhr444,
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
    /// Findings from compiling the verb (for example subtitle cues that
    /// leave the frame), reported with the timeline's own diagnostics.
    pub diagnostics: Vec<Diagnostic>,
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
    if args.crf.is_none() && args.preset.is_none() && args.codec.is_none() && args.profile.is_none()
    {
        return None;
    }
    // A profile names its codec; spelling the codec out keeps the
    // document valid on its own.
    let profile: Option<VideoProfile> = args.profile.map(Into::into);
    let codec = args
        .codec
        .map(Into::into)
        .or_else(|| profile.map(VideoProfile::codec));
    Some(Encode {
        container: None,
        video: Some(VideoEncode {
            codec,
            crf: args.crf,
            preset: args.preset.clone(),
            hardware: None,
            profile,
            keyframe_interval: None,
            max_bitrate_kbps: None,
            level: None,
        }),
        audio: None,
        fast_start: None,
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
        subtitles: Vec::new(),
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
    tl.output.color = shared_color(&[&src]);
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
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
    })
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
    tl.output.color = shared_color(&[&src]);
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
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
    })
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
    tl.output.color = shared_color(&probed.iter().collect::<Vec<_>>());
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
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
    })
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
    tl.output.color = shared_color(&[&src]);
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
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
    })
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
    tl.output.color = shared_color(&[&src]);
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
            // The clip's own length sets the output length exactly; a
            // printed duration would round it at fractional frame rates.
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
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
    })
}

/// A subtitle file to attach, with its language.
#[derive(Debug, Clone)]
pub struct SubtitleFile {
    pub path: PathBuf,
    pub language: Option<String>,
}

/// `subtitles --add`: the input with subtitle files attached as streams.
pub fn add_subtitles(input: &Path, files: &[SubtitleFile], args: &EncodeArgs) -> Result<Compiled> {
    let src = Input::probe(input)?;
    if !src.has_video && !src.has_audio {
        bail!("{} has no video or audio stream", input.display());
    }
    let mut inputs = vec![input.to_owned()];
    inputs.extend(files.iter().map(|f| f.path.clone()));
    let (root, rel) = common_root(&inputs)?;
    let (w, h) = if src.has_video {
        (even(src.width), even(src.height))
    } else {
        (2, 2)
    };
    let mut tl = base_timeline(w, h, src.fps, encode_block(args));
    tl.output.color = shared_color(&[&src]);
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
        clips: vec![video_clip("in", None, None, !args.no_audio, None)],
    });
    for (i, (file, path)) in files.iter().zip(&rel[1..]).enumerate() {
        let id = format!("subs{}", i + 1);
        tl.assets.insert(
            id.clone(),
            Asset {
                src: path.clone(),
                kind: None,
                color: None,
            },
        );
        tl.subtitles.push(SubtitleTrack {
            id: None,
            enabled: true,
            asset: id,
            language: file.language.clone(),
            title: None,
            offset: None,
        });
    }
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
    })
}

/// Where burned-in subtitles sit in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum SubtitlePosition {
    Bottom,
    Top,
    Center,
}

/// Options of `subtitles --burn`.
#[derive(Debug, Clone)]
pub struct BurnOptions {
    /// The subtitle file.
    pub file: PathBuf,
    pub position: SubtitlePosition,
    /// Distance from the top or bottom edge in pixels; 5% of the height
    /// by default.
    pub margin: Option<f64>,
    /// A JSON object with text-source fields merged over the defaults.
    pub style: Option<String>,
    /// Title-safe inset as a percentage of each dimension; cues outside
    /// it get a note. Zero turns the note off.
    pub safe: f64,
    /// Shrink a cue that does not fit the title-safe area (or the frame
    /// when the safe inset is zero) until it does, down to half its size.
    pub fit: bool,
}

/// Lays a cue out the way the renderer will and returns its box size.
fn measure_cue(
    engine: &mut geneva_render::TextEngine,
    spec: &TextSource,
    max_width: f64,
) -> (f64, f64) {
    let resolved = geneva_timeline::ResolvedText {
        text: spec.text.clone().unwrap_or_default(),
        words: Vec::new(),
        max_width,
        spec: spec.clone(),
    };
    let image = engine.render(&resolved, 0.0);
    (f64::from(image.width), f64::from(image.height))
}

/// Scales the size-dependent parts of a cue's style by `ratio`.
fn scale_style(spec: &mut TextSource, template: &TextSource, ratio: f64) {
    spec.style.size = Some(template.style.size.unwrap_or(48.0) * ratio);
    if let (Some(o), Some(t)) = (spec.outline.as_mut(), template.outline.as_ref()) {
        o.width = t.width * ratio;
    }
    if let (Some(sh), Some(t)) = (spec.shadow.as_mut(), template.shadow.as_ref()) {
        sh.x = t.x * ratio;
        sh.y = t.y * ratio;
        sh.blur = t.blur * ratio;
    }
}

/// The default look of burned-in subtitles for a frame `height` pixels
/// tall: white, semi-bold, a black outline and a soft shadow, sized so
/// that two lines take a tenth of the picture.
fn subtitle_style(height: u32, width: u32) -> serde_json::Value {
    let size = (f64::from(height) * 0.045).round().max(12.0);
    serde_json::json!({
        "text": "",
        "size": size,
        "weight": 600,
        "color": "white",
        "align": "center",
        "line_height": 1.15,
        "max_width": (f64::from(width) * 0.9).round(),
        "outline": { "color": "black", "width": (size * 0.06).round().max(1.0) },
        "shadow": { "color": "#000000a0", "x": 0, "y": (size * 0.04).round(), "blur": (size * 0.08).round() }
    })
}

/// How many offending cues are listed one by one before a summary line.
const LISTED: usize = 10;

/// `subtitles --burn`: the input with the cues of a subtitle file drawn
/// into the picture. Every cue becomes a text clip; cues that overlap in
/// time go to further layers. Each cue's box is measured with the text
/// engine and reported when it leaves the frame or the title-safe area.
pub fn burn_subtitles(input: &Path, opts: &BurnOptions, args: &EncodeArgs) -> Result<Compiled> {
    let src = Input::probe(input)?;
    if !src.has_video {
        bail!(
            "{} has no video stream to burn subtitles into",
            input.display()
        );
    }
    let text = std::fs::read_to_string(&opts.file)
        .with_context(|| format!("reading {}", opts.file.display()))?;
    let cues = geneva_media::subtitles::parse(&text)
        .with_context(|| format!("parsing {}", opts.file.display()))?;
    let (root, rel) = common_root(&[input.to_owned()])?;
    let (w, h) = (even(src.width), even(src.height));
    let mut tl = base_timeline(w, h, src.fps, encode_block(args));
    tl.output.color = shared_color(&[&src]);
    tl.output.duration = src.duration.map(seconds);
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
        clips: vec![video_clip("in", None, None, !args.no_audio, None)],
    });

    // The style: defaults for this frame size, then the caller's fields.
    let mut style = subtitle_style(h, w);
    if let Some(json) = &opts.style {
        let user: serde_json::Value =
            serde_json::from_str(json).context("--style is not valid JSON")?;
        let serde_json::Value::Object(fields) = user else {
            bail!("--style must be a JSON object of text fields, for example {{\"size\": 40}}");
        };
        // A `font` shorthand carries its own size, weight, style and line
        // height; those replace the default look unless given explicitly.
        let shorthand = fields
            .get("font")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|f| geneva_timeline::css::parse_font(f).is_some());
        if shorthand {
            for key in ["size", "weight", "italic", "line_height"] {
                if !fields.contains_key(key) {
                    style.as_object_mut().map(|o| o.remove(key));
                }
            }
        }
        for (k, v) in fields {
            style[k] = v;
        }
    }
    let mut template: TextSource = serde_json::from_value(style)
        .context("--style: not a valid text style (see docs/timeline.md, text sources)")?;
    geneva_timeline::css::expand_font(&mut template.style, &mut template.line_height)
        .map_err(|e| anyhow::anyhow!("--style: font: {e}"))?;
    // The default margin keeps the cues inside the title-safe area.
    let margin = opts
        .margin
        .unwrap_or_else(|| (f64::from(h) * (opts.safe / 100.0).max(0.05)).ceil());
    let (anchor, position) = match opts.position {
        SubtitlePosition::Bottom => (
            Point {
                x: Length::Percent(50.0),
                y: Length::Percent(100.0),
            },
            Point {
                x: Length::Percent(50.0),
                y: Length::Px(f64::from(h) - margin),
            },
        ),
        SubtitlePosition::Top => (
            Point {
                x: Length::Percent(50.0),
                y: Length::Percent(0.0),
            },
            Point {
                x: Length::Percent(50.0),
                y: Length::Px(margin),
            },
        ),
        SubtitlePosition::Center => (
            Point {
                x: Length::Percent(50.0),
                y: Length::Percent(50.0),
            },
            Point {
                x: Length::Percent(50.0),
                y: Length::Percent(50.0),
            },
        ),
    };

    // Measuring each cue the way the renderer lays it out.
    let mut engine = geneva_render::TextEngine::new();
    let max_width = template
        .max_width
        .map_or(f64::from(w), |m| m.to_px(f64::from(w)));
    let safe_x = f64::from(w) * opts.safe / 100.0;
    let safe_y = f64::from(h) * opts.safe / 100.0;
    // Per layer: when its current cue ends and how tall its box is, so a
    // cue that overlaps in time stacks beyond the ones still showing.
    let mut layer_ends: Vec<(Ratio, f64)> = Vec::new();
    let mut layers: Vec<Vec<Clip>> = Vec::new();
    let mut diagnostics = Vec::new();
    let mut offscreen = 0usize;
    let mut unsafe_cues = 0usize;
    let mut shrunk_cues = 0usize;
    for (n, cue) in cues.iter().enumerate() {
        if cue.end <= cue.start {
            continue;
        }
        let mut spec = template.clone();
        spec.text = Some(geneva_media::subtitles::strip_tags(&cue.text));
        // Layer assignment: the first layer whose last cue has ended. A
        // cue that lands on a higher layer sits beyond the boxes of the
        // cues still showing on the layers below, away from the edge.
        let layer = layer_ends
            .iter()
            .position(|(end, _)| *end <= cue.start)
            .unwrap_or_else(|| {
                layer_ends.push((Ratio::ZERO, 0.0));
                layers.push(Vec::new());
                layer_ends.len() - 1
            });
        let stacked: f64 = layer_ends[..layer]
            .iter()
            .filter(|(end, _)| *end > cue.start)
            .map(|(_, height)| height)
            .sum();
        let position = match opts.position {
            SubtitlePosition::Bottom => Point {
                x: position.x,
                y: Length::Px(position.y.to_px(f64::from(h)) - stacked),
            },
            SubtitlePosition::Top | SubtitlePosition::Center => Point {
                x: position.x,
                y: Length::Px(position.y.to_px(f64::from(h)) + stacked),
            },
        };
        let px = position.x.to_px(f64::from(w));
        let py = position.y.to_px(f64::from(h));
        let box_of = |bw: f64, bh: f64| {
            let (x0, y0) = (px - anchor.x.to_px(bw), py - anchor.y.to_px(bh));
            (x0, y0, x0 + bw, y0 + bh)
        };
        let (mut bw, mut bh) = measure_cue(&mut engine, &spec, max_width);
        let mut shrunk = None;
        if opts.fit {
            let (lx0, ly0, lx1, ly1) = if opts.safe > 0.0 {
                (safe_x, safe_y, f64::from(w) - safe_x, f64::from(h) - safe_y)
            } else {
                (0.0, 0.0, f64::from(w), f64::from(h))
            };
            let fits = |(x0, y0, x1, y1): (f64, f64, f64, f64)| {
                x0 >= lx0 && y0 >= ly0 && x1 <= lx1 && y1 <= ly1
            };
            let base_size = template.style.size.unwrap_or(48.0);
            let mut ratio = 1.0f64;
            while !fits(box_of(bw, bh)) && ratio > 0.5 {
                ratio = (ratio - 0.05).max(0.5);
                scale_style(&mut spec, &template, ratio);
                (bw, bh) = measure_cue(&mut engine, &spec, max_width);
            }
            if ratio < 1.0 {
                shrunk = Some((base_size, base_size * ratio));
            }
        }
        layer_ends[layer] = (cue.end, bh);
        let (x0, y0, x1, y1) = box_of(bw, bh);
        let clip_index = layers[layer].len();
        let where_ = format!("/layers/{}/clips/{clip_index}", layer + 1);
        let cue_name = format!("cue {} ({}s to {}s)", n + 1, cue.start, cue.end);
        if let Some((from, to)) = shrunk {
            shrunk_cues += 1;
            if shrunk_cues <= LISTED {
                diagnostics.push(Diagnostic::note(
                    "N405",
                    where_.clone(),
                    format!("{cue_name} was shrunk from {from:.0} px to {to:.0} px to fit"),
                ));
            }
        }
        let mut over = Vec::new();
        if x0 < 0.0 {
            over.push(format!("{:.0} px past the left edge", -x0));
        }
        if x1 > f64::from(w) {
            over.push(format!("{:.0} px past the right edge", x1 - f64::from(w)));
        }
        if y0 < 0.0 {
            over.push(format!("{:.0} px past the top edge", -y0));
        }
        if y1 > f64::from(h) {
            over.push(format!("{:.0} px past the bottom edge", y1 - f64::from(h)));
        }
        if !over.is_empty() {
            offscreen += 1;
            if offscreen <= LISTED {
                diagnostics.push(
                    Diagnostic::warning(
                        "W403",
                        where_.clone(),
                        format!(
                            "{cue_name} extends off-screen: its {:.0}×{:.0} px box runs {}",
                            bw,
                            bh,
                            over.join(" and ")
                        ),
                    )
                    .with_help("use a smaller size, a larger margin, more line breaks or a narrower max_width in --style"),
                );
            }
        } else if opts.safe > 0.0
            && (x0 < safe_x
                || y0 < safe_y
                || x1 > f64::from(w) - safe_x
                || y1 > f64::from(h) - safe_y)
        {
            unsafe_cues += 1;
            if unsafe_cues <= LISTED {
                diagnostics.push(
                    Diagnostic::note(
                        "N404",
                        where_.clone(),
                        format!(
                            "{cue_name} lies outside the title-safe area ({}% in from each edge): its box spans x {x0:.0}..{x1:.0}, y {y0:.0}..{y1:.0} of the {w}×{h} frame",
                            opts.safe
                        ),
                    )
                    .with_help("some screens crop the edges; a larger --margin or a smaller size keeps it safe, or set --safe 0 to stop checking"),
                );
            }
        }
        layers[layer].push(Clip {
            id: None,
            source: Source::Text(Box::new(spec)),
            start: Some(seconds(cue.start)),
            duration: Some(seconds(cue.end - cue.start)),
            transition: None,
            fit: None,
            transform: Some(Transform {
                position: Some(Animated::Constant(position)),
                anchor: Some(anchor),
                scale: None,
                rotation: None,
            }),
            opacity: None,
            blend: None,
        });
    }
    if offscreen > LISTED {
        diagnostics.push(Diagnostic::warning(
            "W403",
            "/layers/1",
            format!("{} more cues extend off-screen", offscreen - LISTED),
        ));
    }
    if shrunk_cues > LISTED {
        diagnostics.push(Diagnostic::note(
            "N405",
            "/layers/1",
            format!("{} more cues were shrunk to fit", shrunk_cues - LISTED),
        ));
    }
    if unsafe_cues > LISTED {
        diagnostics.push(Diagnostic::note(
            "N404",
            "/layers/1",
            format!(
                "{} more cues lie outside the title-safe area",
                unsafe_cues - LISTED
            ),
        ));
    }
    if layers.is_empty() {
        diagnostics.push(Diagnostic::warning(
            "W402",
            "/layers",
            format!(
                "{} has no cues; the picture is written unchanged",
                opts.file.display()
            ),
        ));
    }
    for clips in layers {
        tl.layers.push(Layer {
            id: None,
            enabled: true,
            clips,
        });
    }
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics,
    })
}
