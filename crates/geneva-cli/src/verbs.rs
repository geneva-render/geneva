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
    Asset, AudioClip, AudioTrack, AutoChunks, Chunks, Clip, Crop, Encode, Fit, Layer, Output,
    OutputKind, OutputSpec, Source, SubtitleTrack, TextSource, Timeline, Transform, Transition,
    TransitionKind, VideoCodec, VideoEncode, VideoProfile, VideoTune,
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
    /// Sample rate and channel count of the audio stream, if any.
    pub audio: Option<(u32, u16)>,
    /// The video codec's name, as the probe reports it.
    pub codec: Option<String>,
    /// The video's pixel format name.
    pub pixel_format: Option<String>,
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
            audio: info.audio.as_ref().map(|a| (a.sample_rate, a.channels)),
            codec: info.video.as_ref().map(|v| v.codec.clone()),
            pixel_format: info.video.as_ref().map(|v| v.pixel_format.clone()),
        })
    }
}

/// The audio format to write when every input with audio shares one:
/// the sound then keeps its sample rate and channel count (mono stays
/// mono, 44.1 kHz stays 44.1 kHz) instead of the 48 kHz stereo default.
/// More than two channels are written as stereo.
fn shared_audio(inputs: &[&Input]) -> Option<geneva_timeline::schema::AudioOutput> {
    let mut found: Option<(u32, u16)> = None;
    for input in inputs {
        let Some(audio) = input.audio else {
            continue;
        };
        match found {
            None => found = Some(audio),
            Some(f) if f != audio => return None,
            _ => {}
        }
    }
    let (sample_rate, channels) = found?;
    Some(geneva_timeline::schema::AudioOutput {
        sample_rate: Some(sample_rate),
        channels: Some(u8::try_from(channels.min(2)).expect("at most 2")),
    })
}

/// The color encoding to write when every video input shares one: the
/// picture then keeps its encoding (SD material stays BT.601), the tags
/// travel into the file, and nothing is converted on the way. HDR sources
/// fall back to the SDR default, which the renderer maps down to.
fn shared_color(inputs: &[&Input], keep_hdr: bool) -> Option<ColorTags> {
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
    found
        .filter(|c| keep_hdr || !c.is_hdr())
        .map(ColorTags::from)
}

/// Encoder options shared by every verb.
#[derive(Args, Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools, reason = "command-line switches")]
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
    /// What the picture is like, in x264's names; encoders without an
    /// equivalent say so in the report.
    #[arg(long, value_enum)]
    pub tune: Option<TuneArg>,
    /// Seconds between keyframes.
    #[arg(long, value_name = "SECONDS")]
    pub keyframe_interval: Option<f64>,
    /// Keyframes at the interval only, never at scene changes.
    #[arg(long, requires = "keyframe_interval")]
    pub fixed_keyframes: bool,
    /// Keep HDR sources HDR: the output takes their tags (PQ or HLG,
    /// BT.2020) and a ten-bit codec, h265 unless --codec says otherwise.
    /// Without it, HDR sources are tone-mapped to SDR.
    #[arg(long)]
    pub keep_hdr: bool,
    /// Encode the output in this many stretches at once, on separate
    /// cores, joined afterwards: "auto" (the default) decides from the
    /// encoder and the machine, 1 turns it off.
    #[arg(long, value_name = "N|auto", value_parser = parse_chunks)]
    pub chunks: Option<Chunks>,
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
    /// What fills the frame around a picture that does not cover it
    /// (a landscape video on a portrait canvas): bars in the background
    /// color (default), or blur, the picture whole over a blurred,
    /// scaled-up copy of itself.
    #[arg(long, value_enum, value_name = "FILL")]
    pub fill: Option<FillArg>,
}

/// What surrounds a picture that does not cover the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FillArg {
    /// The background color.
    Bars,
    /// A blurred, scaled-up copy of the picture.
    Blur,
}

/// The blur's standard deviation for a fill, in output pixels: a
/// twenty-fourth of the frame's shorter side (45 px on a 1080-wide
/// portrait frame), heavy enough to read as a backdrop.
pub fn fill_blur_radius(width: u32, height: u32) -> f64 {
    (f64::from(width.min(height)) / 24.0).round().max(1.0)
}

/// Puts a blurred, cover-fitted copy of the timeline's video under it,
/// for a picture that does not cover a `width`×`height` frame; returns
/// whether it did. The copy carries no audio and is the first layer;
/// the picture keeps its own fit.
pub fn add_blur_fill(tl: &mut Timeline, src_w: u32, src_h: u32) -> bool {
    let (w, h) = (tl.output.width, tl.output.height);
    if w == 0 || h == 0 || src_w == 0 || src_h == 0 {
        return false;
    }
    let same_shape =
        (f64::from(w) / f64::from(h) - f64::from(src_w) / f64::from(src_h)).abs() < 0.01;
    if same_shape {
        return false;
    }
    let Some(picture) = tl
        .layers
        .iter()
        .flat_map(|l| &l.clips)
        .find(|c| matches!(c.source, Source::Video { .. }) && c.effects.is_empty())
    else {
        return false;
    };
    if matches!(picture.fit, Some(Fit::Cover | Fit::Fill)) {
        return false;
    }
    let mut fill = picture.clone();
    fill.id = Some("fill".to_owned());
    if let Source::Video { audio, .. } = &mut fill.source {
        *audio = Some(false);
    }
    fill.fit = Some(Fit::Cover);
    fill.transform = None;
    fill.effects = vec![geneva_timeline::schema::Effect::Blur {
        radius: Animated::Constant(fill_blur_radius(w, h)),
    }];
    tl.layers.insert(
        0,
        Layer {
            id: Some("fill".to_owned()),
            enabled: true,
            clips: vec![fill],
        },
    );
    true
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

/// Parses `--chunks`: a count or the word `auto`.
fn parse_chunks(text: &str) -> Result<Chunks, String> {
    if text.eq_ignore_ascii_case("auto") {
        return Ok(Chunks::Auto(AutoChunks::Auto));
    }
    match text.parse::<u32>() {
        Ok(n) if n >= 1 => Ok(Chunks::Count(n)),
        _ => Err("a count of at least 1, or \"auto\"".to_owned()),
    }
}

/// `--tune` values, x264's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TuneArg {
    Film,
    Animation,
    Grain,
    #[value(name = "stillimage")]
    StillImage,
    #[value(name = "fastdecode")]
    FastDecode,
    #[value(name = "zerolatency")]
    ZeroLatency,
}

impl From<TuneArg> for VideoTune {
    fn from(t: TuneArg) -> Self {
        match t {
            TuneArg::Film => Self::Film,
            TuneArg::Animation => Self::Animation,
            TuneArg::Grain => Self::Grain,
            TuneArg::StillImage => Self::StillImage,
            TuneArg::FastDecode => Self::FastDecode,
            TuneArg::ZeroLatency => Self::ZeroLatency,
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
    /// Whether to write `manifest.json` into the output directory: the
    /// report's file list with paths relative to the directory, for
    /// whatever uploads or serves the files later.
    pub manifest: bool,
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
    if args.crf.is_none()
        && args.preset.is_none()
        && args.codec.is_none()
        && args.profile.is_none()
        && args.tune.is_none()
        && args.keyframe_interval.is_none()
        && !args.fixed_keyframes
        && !args.keep_hdr
        && args.chunks.is_none()
    {
        return None;
    }
    // A profile names its codec; spelling the codec out keeps the
    // document valid on its own.
    let profile: Option<VideoProfile> = args.profile.map(Into::into);
    let codec = args
        .codec
        .map(Into::into)
        .or_else(|| profile.map(VideoProfile::codec))
        // HDR needs ten bits: H.265 unless told otherwise.
        .or(args.keep_hdr.then_some(VideoCodec::H265));
    Some(Encode {
        container: None,
        video: Some(VideoEncode {
            codec,
            crf: args.crf,
            preset: args.preset.clone(),
            hardware: None,
            profile,
            keyframe_interval: args.keyframe_interval,
            max_bitrate_kbps: None,
            bitrate_kbps: None,
            level: None,
            tune: args.tune.map(Into::into),
            fixed_keyframes: args.fixed_keyframes.then_some(true),
            chunks: args.chunks,
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
        crop: None,
        fit,
        transform: None,
        opacity: None,
        blend: None,
        effects: Vec::new(),
        mask: None,
        speed: None,
    }
}

/// A crop from the command line: `X,Y,WxH`, or `WxH` for a centered
/// rectangle; each value a pixel count or a percentage of the source.
#[derive(Clone, Debug, PartialEq)]
pub struct CropArg {
    x: Option<Length>,
    y: Option<Length>,
    width: Length,
    height: Length,
}

fn parse_crop_length(s: &str) -> Result<Length> {
    let s = s.trim();
    let (num, percent) = match s.strip_suffix('%') {
        Some(n) => (n, true),
        None => (s.strip_suffix("px").unwrap_or(s), false),
    };
    let v: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{s:?} is not a length (a number or a percentage)"))?;
    Ok(if percent {
        Length::Percent(v)
    } else {
        Length::Px(v)
    })
}

impl std::str::FromStr for CropArg {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split(',').collect();
        let (x, y, size) = match parts.as_slice() {
            [size] => (None, None, *size),
            [x, y, size] => (Some(*x), Some(*y), *size),
            _ => bail!("--crop takes X,Y,WxH or WxH, for example 240,0,1440x1080 or 80%x80%"),
        };
        let (w, h) = size
            .split_once('x')
            .ok_or_else(|| anyhow::anyhow!("--crop size must be WxH, for example 1080x1080"))?;
        Ok(Self {
            x: x.map(parse_crop_length).transpose()?,
            y: y.map(parse_crop_length).transpose()?,
            width: parse_crop_length(w)?,
            height: parse_crop_length(h)?,
        })
    }
}

impl CropArg {
    /// The crop for a `w`×`h` source, centered when no corner was given,
    /// and its size in whole pixels.
    fn resolve(&self, w: u32, h: u32) -> Result<(Crop, u32, u32)> {
        let (fw, fh) = (f64::from(w), f64::from(h));
        let cw = self.width.to_px(fw).min(fw);
        let ch = self.height.to_px(fh).min(fh);
        if cw < 2.0 || ch < 2.0 {
            bail!("--crop leaves nothing of the {w}×{h} picture");
        }
        let x = match self.x {
            Some(l) => l,
            None => Length::Px(((fw - cw) / 2.0).floor()),
        };
        let y = match self.y {
            Some(l) => l,
            None => Length::Px(((fh - ch) / 2.0).floor()),
        };
        if x.to_px(fw) + cw > fw + 0.5 || y.to_px(fh) + ch > fh + 0.5 {
            bail!("--crop reaches outside the {w}×{h} picture");
        }
        Ok((
            Crop {
                x: Some(x),
                y: Some(y),
                width: Some(self.width),
                height: Some(self.height),
            },
            cw.round() as u32,
            ch.round() as u32,
        ))
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
        outputs: BTreeMap::new(),
    }
}

/// `convert` and `resize`: one input, optionally a crop and a new size.
/// The output takes the crop's size unless a size is given.
#[allow(clippy::too_many_arguments)]
pub fn convert(
    input: &Path,
    crop: Option<&CropArg>,
    speed: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    fit: Option<FitArg>,
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
    let (crop, pic_w, pic_h) = match crop {
        Some(c) => {
            let (crop, w, h) = c.resolve(src.width, src.height)?;
            (Some(crop), w, h)
        }
        None => (None, src.width, src.height),
    };
    let (w, h) = output_size(pic_w, pic_h, width, height);
    let (root, rel) = common_root(&[input.to_owned()])?;
    let mut tl = base_timeline(w, h, fps.map_or(src.fps, |f| f.0), encode_block(args));
    tl.output.color = shared_color(&[&src], args.keep_hdr);
    tl.output.audio = shared_audio(&[&src]);
    tl.assets.insert(
        "in".to_owned(),
        Asset {
            src: rel[0].clone(),
            kind: None,
            color: None,
        },
    );
    let resized = (w, h) != (pic_w, pic_h);
    let mut clip = video_clip(
        "in",
        None,
        None,
        !args.no_audio,
        // A fit given without a size still counts: `--for` may build
        // a differently shaped canvas after this.
        if resized || crop.is_some() {
            Some(fit.unwrap_or_default().into())
        } else {
            fit.map(Into::into)
        },
    );
    clip.crop = crop;
    clip.speed = speed;
    tl.layers.push(Layer {
        id: None,
        enabled: true,
        clips: vec![clip],
    });
    if args.fill == Some(FillArg::Blur) {
        add_blur_fill(&mut tl, pic_w, pic_h);
    }
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
        manifest: false,
    })
}

/// `trim`: one input, a range, optionally a crop (the output takes its
/// size).
pub fn trim(
    input: &Path,
    from: Option<Time>,
    to: Option<Time>,
    duration: Option<Time>,
    crop: Option<&CropArg>,
    speed: Option<f64>,
    args: &EncodeArgs,
) -> Result<Compiled> {
    let src = Input::probe(input)?;
    let (root, rel) = common_root(&[input.to_owned()])?;
    let (crop, pic_w, pic_h) = match crop {
        Some(c) => {
            let (crop, w, h) = c.resolve(src.width, src.height)?;
            (Some(crop), w, h)
        }
        None => (None, src.width, src.height),
    };
    let from_secs = from.map_or(Ratio::ZERO, |t| t.resolve(src.fps));
    let out = match (to, duration) {
        (Some(t), _) => Some(t),
        (None, Some(d)) => Some(seconds(from_secs + d.resolve(src.fps))),
        (None, None) => None,
    };
    let mut tl = base_timeline(even(pic_w), even(pic_h), src.fps, encode_block(args));
    tl.output.color = shared_color(&[&src], args.keep_hdr);
    tl.output.audio = shared_audio(&[&src]);
    tl.assets.insert(
        "in".to_owned(),
        Asset {
            src: rel[0].clone(),
            kind: None,
            color: None,
        },
    );
    let mut clip = video_clip(
        "in",
        from.map(|_| seconds(from_secs)),
        out,
        !args.no_audio,
        crop.map(|_| Fit::Contain),
    );
    clip.crop = crop;
    clip.speed = speed;
    tl.layers.push(Layer {
        id: None,
        enabled: true,
        clips: vec![clip],
    });
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
        manifest: false,
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
    tl.output.color = shared_color(&probed.iter().collect::<Vec<_>>(), args.keep_hdr);
    tl.output.audio = shared_audio(&probed.iter().collect::<Vec<_>>());
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
        manifest: false,
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
    tl.output.color = shared_color(&[&src], args.keep_hdr);
    tl.output.audio = shared_audio(&[&src]);
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
            crop: None,
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
            effects: Vec::new(),
            mask: None,
            speed: None,
        }],
    });
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
        manifest: false,
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

/// `audio`: extract, mute, replace or mix. `speech` writes the extracted
/// audio the way speech recognizers want it: 16 kHz, one channel.
pub fn audio(input: &Path, op: &AudioOp, speech: bool, args: &EncodeArgs) -> Result<Compiled> {
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
    tl.output.color = shared_color(&[&src], args.keep_hdr);
    tl.output.audio = shared_audio(&[&src]);
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
            speed: None,
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
            if speech {
                tl.output.audio = Some(geneva_timeline::schema::AudioOutput {
                    sample_rate: Some(16000),
                    channels: Some(1),
                });
            }
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
        manifest: false,
    })
}

/// An output entry with nothing set but its kind.
fn output_spec(kind: OutputKind) -> OutputSpec {
    OutputSpec {
        kind,
        path: None,
        width: None,
        height: None,
        encode: None,
        audio: None,
        at: None,
        every: None,
        columns: None,
    }
}

/// A one-input timeline showing the whole picture, for the verbs that
/// write pictures of it rather than a new video.
fn whole_picture(input: &Path) -> Result<(Timeline, PathBuf, Input)> {
    let src = Input::probe(input)?;
    if !src.has_video {
        bail!("{} has no video stream", input.display());
    }
    let (root, rel) = common_root(&[input.to_owned()])?;
    let (w, h) = (even(src.width), even(src.height));
    let mut tl = base_timeline(w, h, src.fps, None);
    tl.output.color = shared_color(&[&src], false);
    tl.output.audio = shared_audio(&[&src]);
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
        clips: vec![video_clip("in", None, None, true, None)],
    });
    Ok((tl, root, src))
}

/// `poster`: one still of the video, at a time or the first clear frame.
pub fn poster(
    input: &Path,
    at: Option<Time>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<Compiled> {
    let (mut tl, root, _) = whole_picture(input)?;
    let mut spec = output_spec(OutputKind::Poster);
    spec.at = at;
    spec.width = width;
    spec.height = height;
    tl.outputs.insert("poster".to_owned(), spec);
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
        manifest: false,
    })
}

/// `sprites`: a sheet of thumbnails and the WebVTT file that maps times
/// to tiles, for a player's seek preview.
pub fn sprites(
    input: &Path,
    every: Option<Time>,
    columns: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<Compiled> {
    let (mut tl, root, _) = whole_picture(input)?;
    let mut spec = output_spec(OutputKind::Sprites);
    spec.every = every;
    spec.columns = columns;
    spec.width = width;
    spec.height = height;
    tl.outputs.insert("sprites".to_owned(), spec);
    Ok(Compiled {
        timeline: tl,
        root,
        diagnostics: Vec::new(),
        manifest: false,
    })
}

/// What `publish` writes besides the video.
#[derive(Debug, Clone, Default)]
pub struct PublishOptions {
    /// The poster's time; the first clear frame when unset.
    pub poster_at: Option<Time>,
    /// Seconds between sprite tiles.
    pub every: Option<Time>,
    /// Also write the audio alone, at 16 kHz mono, for transcription.
    pub speech: bool,
    /// Leave the poster out.
    pub no_poster: bool,
    /// Leave the sprites out.
    pub no_sprites: bool,
}

/// `publish`: the set of files a video needs on a web page, from one
/// pass over the source: the video fitted to a target (web unless
/// `--for` says otherwise), a poster and seek-preview sprites. Returns
/// the compiled document and the encode arguments to render it with:
/// a source that already fits the target is copied, not re-encoded,
/// so the target is dropped from them.
pub fn publish(
    input: &Path,
    opts: &PublishOptions,
    args: &EncodeArgs,
) -> Result<(Compiled, EncodeArgs)> {
    let (mut tl, root, src) = whole_picture(input)?;
    let mut args = args.clone();
    let target_name = args.for_.clone().unwrap_or_else(|| "web".to_owned());
    let target = crate::targets::find(&target_name).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown target {target_name:?}; the targets are {} (see `geneva targets`)",
            crate::targets::names()
        )
    })?;
    let mut diagnostics = Vec::new();
    let asks_encode =
        encode_block(&args).is_some() || args.quality.is_some() || args.budget.is_some();
    let bytes = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let fits = !asks_encode && !args.exact && fits_target(input, &src, bytes, target);
    if fits {
        // The planner copies the streams when the container agrees; the
        // target would otherwise force a re-encode through its quality.
        args.for_ = None;
        diagnostics.push(Diagnostic::note(
            "N600",
            "",
            format!(
                "{} already fits {target_name} ({}×{} {}, {} kb/s): copied, not re-encoded",
                input.display(),
                src.width,
                src.height,
                src.codec.as_deref().unwrap_or("?"),
                kbps(bytes, src.duration)
            ),
        ));
    } else {
        args.for_ = Some(target_name);
    }
    tl.outputs
        .insert("video".to_owned(), output_spec(OutputKind::Video));
    if !opts.no_poster {
        let mut spec = output_spec(OutputKind::Poster);
        spec.at = opts.poster_at;
        tl.outputs.insert("poster".to_owned(), spec);
    }
    if !opts.no_sprites {
        let mut spec = output_spec(OutputKind::Sprites);
        spec.every = opts.every;
        tl.outputs.insert("sprites".to_owned(), spec);
    }
    if opts.speech {
        if !src.has_audio {
            bail!("{} has no audio stream for --speech", input.display());
        }
        let mut spec = output_spec(OutputKind::Audio);
        spec.path = Some("speech.wav".to_owned());
        spec.audio = Some(geneva_timeline::schema::AudioOutput {
            sample_rate: Some(16000),
            channels: Some(1),
        });
        tl.outputs.insert("speech".to_owned(), spec);
    }
    Ok((
        Compiled {
            timeline: tl,
            root,
            diagnostics,
            manifest: true,
        },
        args,
    ))
}

/// Average bit rate of a file in kb/s, from its size and length.
fn kbps(bytes: u64, duration: Option<Ratio>) -> u64 {
    match duration.map(Ratio::to_f64) {
        Some(secs) if secs > 0.0 => (bytes as f64 * 8.0 / secs / 1000.0).round() as u64,
        _ => 0,
    }
}

/// Whether a source already meets a target: H.264 4:2:0 in an MP4 or
/// MOV, no larger than the target's ceiling, no faster than its frame
/// rate, and no heavier than its bitrate cap for that size.
fn fits_target(input: &Path, src: &Input, bytes: u64, target: &crate::targets::Target) -> bool {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let h264 = src.codec.as_deref() == Some("h264");
    let sdr = src.color.is_none_or(|c| !c.is_hdr());
    let pixels = src.pixel_format.as_deref() == Some("yuv420p");
    let (long, short) = (src.width.max(src.height), src.width.min(src.height));
    let fits_size = long <= target.max_width.max(target.max_height)
        && short <= target.max_width.min(target.max_height)
        && !(target.portrait && src.width > src.height);
    let fps = src.fps.to_f64();
    let cap = crate::targets::cap_kbps(target, short, fps);
    let rate = kbps(bytes, src.duration);
    matches!(ext.as_str(), "mp4" | "mov" | "m4v")
        && h264
        && sdr
        && pixels
        && fits_size
        && fps <= target.max_fps + 0.01
        && (rate == 0 || cap.is_none_or(|c| rate <= u64::from(c)))
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
    tl.output.color = shared_color(&[&src], args.keep_hdr);
    tl.output.audio = shared_audio(&[&src]);
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
        manifest: false,
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
    tl.output.color = shared_color(&[&src], args.keep_hdr);
    tl.output.audio = shared_audio(&[&src]);
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
            crop: None,
            fit: None,
            transform: Some(Transform {
                position: Some(Animated::Constant(position)),
                anchor: Some(anchor),
                scale: None,
                rotation: None,
            }),
            opacity: None,
            blend: None,
            effects: Vec::new(),
            mask: None,
            speed: None,
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
        manifest: false,
    })
}
