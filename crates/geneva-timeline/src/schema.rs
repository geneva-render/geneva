//! The timeline document model, version 0.2.
//!
//! Every type here is the source of truth for both the parser and the
//! published JSON Schema; doc comments become schema descriptions.

use std::collections::BTreeMap;

use geneva_color::ColorTags;
use schemars::JsonSchema;
use serde::de;
use serde::{Deserialize, Serialize};

use crate::animated::Animated;
use crate::color::ColorValue;
use crate::length::{Length, Point, Scale};
use crate::time::{Fps, Time};

/// The timeline format version this crate writes.
pub const FORMAT_VERSION: &str = "0.3";

/// The format versions this crate reads. A 0.2 document is a 0.1 document
/// with more optional fields (crop, effects, mask, speed), so both are
/// accepted as they are.
pub const ACCEPTED_VERSIONS: &[&str] = &["0.1", "0.2", "0.3"];

/// A complete composition: output settings, assets and layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Geneva timeline")]
pub struct Timeline {
    /// Format version: "0.3", or "0.1" and "0.2" for a document written before
    /// crop, effects, mask and speed existed (read as it is).
    pub geneva: String,
    /// Frame size, rate, duration and encoding settings of the output.
    pub output: Output,
    /// Further files rendered from the same composition in one pass:
    /// video renditions, a poster, a sprite sheet, audio. Keys name them;
    /// each is written under the directory `render -o` names. With
    /// outputs present, `output` describes the canvas the outputs are
    /// made from.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, OutputSpec>,
    /// Media files the composition may reference, keyed by a short id.
    /// Layers refer to assets by id and never by path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub assets: BTreeMap<String, Asset>,
    /// Reusable sub-compositions, keyed by name. A clip shows one with a
    /// source of kind "composition".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub compositions: BTreeMap<String, CompositionDef>,
    /// Named motion, spelled like CSS `@keyframes`: each rule maps an
    /// offset ("from", "to", "60%") to a declaration block
    /// ("transform: translateX(-656px); opacity: 0"). A clip plays one
    /// with its `animation` field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, BTreeMap<String, String>>,
    /// Visual layers, composited from first (bottom) to last (top).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
    /// Audio-only tracks, mixed together with the audio of video clips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio: Vec<AudioTrack>,
    /// Subtitle tracks written to the output as text streams.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<SubtitleTrack>,
}

/// A reusable composition: its own frame, its own layers, rendered as a
/// unit and placed like an image. Times inside it are relative to the
/// clip that shows it, and percentages refer to its own size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompositionDef {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Clear color. Defaults to transparent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorValue>,
    /// Visual layers, composited from first (bottom) to last (top).
    pub layers: Vec<Layer>,
}

/// Output frame and encoding settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame rate.
    pub fps: Fps,
    /// Total duration. Defaults to the end of the last clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Time>,
    /// Color the frame is cleared to before compositing. Defaults to opaque
    /// black; use "transparent" for outputs with alpha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorValue>,
    /// Color tags written to the output. Defaults to BT.709 SDR, limited
    /// range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorTags>,
    /// Audio output format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioOutput>,
    /// Encoder settings. Defaults are chosen per container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode: Option<Encode>,
}

/// Audio output format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioOutput {
    /// Sample rate in Hz. Defaults to 48000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// Channel count: 1 (mono) or 2 (stereo). Defaults to 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<u8>,
}

/// One file of a multi-output render, made from the composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    /// What to write.
    pub kind: OutputKind,
    /// File name, relative to the output directory. Defaults to the
    /// output's name with the kind's usual extension (`.mp4`, `.jpg`,
    /// `.wav`); a sprite sheet writes a `.vtt` map next to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Picture width. For a video rendition or a poster, the canvas is
    /// scaled to it; giving one of width and height keeps the aspect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Picture height. For a sprite sheet, the height of one tile
    /// (default 90).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Encoder settings for a video or audio output; a video rendition
    /// falls back to `output.encode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode: Option<Encode>,
    /// Audio format for a video or audio output; falls back to
    /// `output.audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioOutput>,
    /// Poster: the time of the frame to take. Without it the frame is
    /// chosen: the first clear, non-black frame after the opening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Time>,
    /// Sprite sheet: the interval between tiles. Without it the sheet
    /// holds about a hundred tiles, at least a second apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<Time>,
    /// Sprite sheet: tiles per row (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<u32>,
}

/// What a multi-output entry writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputKind {
    /// A video file: the composition, scaled to the entry's size.
    Video,
    /// One frame as an image.
    Poster,
    /// A tiled sheet of small frames with a WebVTT map, for scrubbing.
    Sprites,
    /// An audio file of the mix.
    Audio,
}

impl OutputKind {
    /// The JSON spelling of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Poster => "poster",
            Self::Sprites => "sprites",
            Self::Audio => "audio",
        }
    }
}

/// Encoder settings for the rendered file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Encode {
    /// Container format. Defaults to the extension of the output path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<Container>,
    /// Video encoder settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoEncode>,
    /// Audio encoder settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioEncode>,
    /// Whether MP4, MOV and M4A files carry their index at the front, so
    /// playback can start before the download ends. Defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_start: Option<bool>,
}

/// Output container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Container {
    /// MP4 (ISO base media).
    Mp4,
    /// QuickTime.
    Mov,
    /// Matroska.
    Mkv,
    /// WebM.
    Webm,
    /// MXF (Material Exchange Format), for broadcast delivery.
    Mxf,
    /// MPEG-4 audio only (AAC).
    M4a,
    /// Ogg audio only (Opus or Vorbis).
    Ogg,
    /// FLAC audio only, lossless.
    Flac,
    /// WAV audio only, uncompressed.
    Wav,
    /// MP3 audio only.
    Mp3,
    /// One image file per frame. The output path is a pattern such as
    /// `frames/%04d.png`; the extension picks PNG or JPEG.
    ImageSequence,
}

impl Container {
    /// Whether the container holds audio only.
    pub fn is_audio_only(self) -> bool {
        matches!(
            self,
            Self::M4a | Self::Ogg | Self::Flac | Self::Wav | Self::Mp3
        )
    }

    /// Whether the container holds video only.
    pub fn is_video_only(self) -> bool {
        matches!(self, Self::ImageSequence)
    }
}

/// Video encoder settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoEncode {
    /// Codec. Defaults to h264 for mp4/mov/mkv and vp9 for webm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<VideoCodec>,
    /// Constant-quality level; lower is better quality and larger files.
    /// The useful range is roughly 18 to 30 for h264 and h265.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crf: Option<u8>,
    /// Encoder speed/efficiency preset, for example "medium" or "slow".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Hardware encoder policy. Defaults to "auto".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<HardwarePolicy>,
    /// Codec profile, for prores and dnxhd. Defaults to "hq" for prores
    /// and "dnxhr-hq" for dnxhd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<VideoProfile>,
    /// Seconds between keyframes. Defaults to the encoder's own choice;
    /// 2 is the usual value for anything played over a network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyframe_interval: Option<f64>,
    /// Bitrate ceiling in kb/s. Quality stays constant until the ceiling
    /// bites, as a player's buffer or a platform's limit requires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bitrate_kbps: Option<u32>,
    /// Average bitrate in kb/s to aim for. Encoders that hold constant
    /// quality under a ceiling (x264) ignore it; hardware encoders, which
    /// cannot, switch to their bitrate mode at this rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    /// H.264 or H.265 level, for example "4.1", which tells old decoders
    /// what the stream needs. Defaults to what the encoder picks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// What the picture is like, so the encoder spends its bits
    /// accordingly, in x264's names. Encoders without an equivalent
    /// ignore it and the report says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tune: Option<VideoTune>,
    /// Place keyframes at the interval only, never at scene changes, as
    /// streaming platforms and segmenters want. Needs `keyframe_interval`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_keyframes: Option<bool>,
    /// How many stretches the output is encoded in at once, each on its
    /// own cores, joined afterwards: `"auto"` (the default) decides from
    /// the encoder and the machine, a number forces it, `1` turns it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Chunks>,
}

/// How many stretches to encode at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Chunks {
    /// Decided from the encoder's scaling and the machine's cores.
    Auto(AutoChunks),
    /// This many; `1` encodes in one run.
    Count(u32),
}

/// The word `"auto"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AutoChunks {
    /// Decided at run time.
    Auto,
}

/// What the picture is like, in x264's tune names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoTune {
    /// Live-action film: keeps fine detail.
    Film,
    /// Animation: flat areas and sharp edges.
    Animation,
    /// Grainy material: keeps the grain instead of smoothing it.
    Grain,
    /// Slideshows and near-still pictures.
    StillImage,
    /// Cheaper to decode, for weak players.
    FastDecode,
    /// No lookahead or frame delay, for live use.
    ZeroLatency,
}

impl VideoTune {
    /// The name x264 uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Film => "film",
            Self::Animation => "animation",
            Self::Grain => "grain",
            Self::StillImage => "stillimage",
            Self::FastDecode => "fastdecode",
            Self::ZeroLatency => "zerolatency",
        }
    }
}

/// Video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VideoCodec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    H265,
    /// VP9.
    Vp9,
    /// AV1.
    Av1,
    /// Apple ProRes, 10-bit 4:2:2 (4:4:4 for the 4444 profiles).
    Prores,
    /// Avid DNxHD / DNxHR, 8-bit or 10-bit 4:2:2 by profile.
    Dnxhd,
    /// PNG frames, lossless RGBA.
    Png,
    /// Motion JPEG.
    Mjpeg,
}

/// Profiles of the intermediate codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VideoProfile {
    /// ProRes 422 Proxy.
    Proxy,
    /// ProRes 422 LT.
    Lt,
    /// ProRes 422.
    Standard,
    /// ProRes 422 HQ.
    Hq,
    /// ProRes 4444.
    #[serde(rename = "4444")]
    P4444,
    /// ProRes 4444 XQ.
    #[serde(rename = "4444-xq")]
    P4444Xq,
    /// DNxHR LB (low bandwidth, 8-bit 4:2:2).
    DnxhrLb,
    /// DNxHR SQ (standard quality, 8-bit 4:2:2).
    DnxhrSq,
    /// DNxHR HQ (high quality, 8-bit 4:2:2).
    DnxhrHq,
    /// DNxHR HQX (10-bit 4:2:2).
    DnxhrHqx,
    /// DNxHR 444 (10-bit 4:4:4).
    Dnxhr444,
}

impl VideoProfile {
    /// The codec a profile belongs to.
    pub fn codec(self) -> VideoCodec {
        match self {
            Self::Proxy | Self::Lt | Self::Standard | Self::Hq | Self::P4444 | Self::P4444Xq => {
                VideoCodec::Prores
            }
            Self::DnxhrLb | Self::DnxhrSq | Self::DnxhrHq | Self::DnxhrHqx | Self::Dnxhr444 => {
                VideoCodec::Dnxhd
            }
        }
    }

    /// The name the encoder knows the profile by.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Lt => "lt",
            Self::Standard => "standard",
            Self::Hq => "hq",
            Self::P4444 => "4444",
            Self::P4444Xq => "4444xq",
            Self::DnxhrLb => "dnxhr_lb",
            Self::DnxhrSq => "dnxhr_sq",
            Self::DnxhrHq => "dnxhr_hq",
            Self::DnxhrHqx => "dnxhr_hqx",
            Self::Dnxhr444 => "dnxhr_444",
        }
    }
}

/// Whether to use a hardware encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HardwarePolicy {
    /// Use a hardware encoder when one is available and works, otherwise
    /// fall back to software.
    Auto,
    /// Always use the software encoder.
    Never,
    /// Fail if no hardware encoder is available.
    Require,
}

/// Audio encoder settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioEncode {
    /// Codec. Defaults to aac for mp4/mov/mkv and opus for webm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<AudioCodec>,
    /// Bitrate in kilobits per second. Defaults to 160.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
}

/// Audio codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AudioCodec {
    /// AAC-LC.
    Aac,
    /// Opus.
    Opus,
    /// FLAC, lossless.
    Flac,
    /// 16-bit PCM, uncompressed.
    Pcm,
    /// 24-bit PCM, uncompressed.
    Pcm24,
    /// MP3.
    Mp3,
    /// Vorbis.
    Vorbis,
    /// Apple Lossless.
    Alac,
    /// Dolby Digital (AC-3).
    Ac3,
}

/// A media file referenced by the composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    /// Path relative to the asset root. Absolute paths and ".." segments
    /// are not allowed.
    pub src: String,
    /// What the file contains. Inferred from the extension when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AssetKind>,
    /// Color tags that override or complete what the file declares.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorTags>,
}

/// The kind of content in an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AssetKind {
    /// A video file, possibly with audio.
    Video,
    /// A still image.
    Image,
    /// An audio file.
    Audio,
    /// A font file (TrueType or OpenType).
    Font,
    /// A subtitle file (SubRip `.srt` or WebVTT `.vtt`).
    Subtitle,
    /// A caption file a clip draws: SubRip, WebVTT, or the JSON a
    /// speech recogniser writes, one entry per word.
    Captions,
    /// A markup file drawn by the HTML source.
    Html,
}

impl AssetKind {
    /// Guesses the kind from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "mp4" | "mov" | "mkv" | "webm" | "m4v" | "mxf" | "ts" | "avi" => Some(Self::Video),
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" => Some(Self::Image),
            "wav" | "mp3" | "aac" | "m4a" | "flac" | "ogg" | "opus" | "oga" => Some(Self::Audio),
            "ttf" | "otf" | "ttc" => Some(Self::Font),
            "html" | "htm" => Some(Self::Html),
            "srt" | "vtt" => Some(Self::Subtitle),
            "json" => Some(Self::Captions),
            _ => None,
        }
    }

    /// The JSON spelling of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Font => "font",
            Self::Subtitle => "subtitle",
            Self::Captions => "captions",
            Self::Html => "html",
        }
    }
}

/// A visual layer: a sequence of non-overlapping clips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// Optional identifier used in diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Set to false to skip the layer without removing it.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Clips in time order. A clip without "start" begins where the
    /// previous clip ends.
    pub clips: Vec<Clip>,
}

fn default_true() -> bool {
    true
}

/// A clip: a source placed on the timeline with a transform and opacity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    /// Optional identifier used in diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// What the clip shows.
    pub source: Source,
    /// When the clip begins on the timeline. Defaults to the end of the
    /// previous clip in the layer, or 0 for the first clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Time>,
    /// How long the clip lasts. Defaults to the source range for video and
    /// audio, and to the end of the output for images, solids, shapes and
    /// text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Time>,
    /// A transition from the previous clip in the layer. The clip starts
    /// early by the transition duration and blends over the previous one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
    /// A rectangle of the source to show; the rest is discarded before
    /// "fit" and the transform see the picture, so the clip's box is the
    /// rectangle. Percentages refer to the source's own size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
    /// How the source is sized to the output frame before the transform.
    /// Defaults to "contain" for video and to "none" (natural size) for
    /// everything else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,
    /// Motion from the document's `keyframes`, spelled like the CSS
    /// `animation` shorthand: a rule name with a duration, and optionally
    /// a delay, a timing function, an iteration count and a direction
    /// ("slide-in 0.5s ease-out", "pulse 2s infinite alternate").
    /// Several are separated by commas. What a rule sets is applied on
    /// top of `transform` and replaces `opacity`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<String>,
    /// Position, scale and rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
    /// Opacity from 0 (invisible) to 1 (opaque). Defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Animated<f64>>,
    /// Blend mode against the layers below. Defaults to "normal".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend: Option<BlendMode>,
    /// Effects applied to the placed picture, in order, before opacity
    /// and blending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// A mask limiting what the clip shows: a shape cut from the clip's
    /// box, or the luma of an image stretched over it. It moves and
    /// scales with the clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
    /// How fast the source plays: 2 is twice as fast, 0.5 half speed.
    /// The clip lasts its source range divided by this; the source's
    /// audio is resampled, so its pitch follows. The clip's own keyframes
    /// stay in output time. Defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
}

/// A mask on a clip, in the coordinates of the clip's box (after the
/// crop, before the fit and the transform): pixels of the box, or
/// percentages of its size.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mask {
    /// The shape: "rect" (default) or "ellipse". Ignored with "asset".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<ShapeKind>,
    /// Left edge of the shape's box. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Length>,
    /// Top edge of the shape's box. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Length>,
    /// Width of the shape's box. Defaults to the clip's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<Length>,
    /// Height of the shape's box. Defaults to the clip's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Length>,
    /// Corner radius of a rectangle, in pixels of the clip's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// Width of the soft edge in pixels of the clip's box; 0 (the
    /// default) is a hard edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feather: Option<f64>,
    /// Id of an image asset whose luma, times its alpha, is the
    /// coverage: white shows, black hides. Stretched over the clip's box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Whether to show what the mask hides and hide what it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert: Option<bool>,
}

/// An effect on a clip's picture, selected by "kind". Effects act on the
/// picture as placed in the output frame, so their sizes are in output
/// pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Effect {
    /// A Gaussian blur.
    Blur {
        /// The blur's standard deviation in output pixels (as CSS's
        /// `blur()`); 0 leaves the picture as it is. Animatable.
        radius: Animated<f64>,
    },
}

/// A rectangle of a source, in source pixels or in percentages of the
/// source's own width and height.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    /// Left edge. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Length>,
    /// Top edge. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Length>,
    /// Width. Defaults to the rest of the source to the right of "x".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<Length>,
    /// Height. Defaults to the rest of the source below "y".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Length>,
}

impl Crop {
    /// The rectangle `[x, y, width, height]` in pixels of a `w`×`h`
    /// source, clamped to the source; `None` when nothing is left.
    pub fn to_px(self, w: f64, h: f64) -> Option<[f64; 4]> {
        let x = self.x.map_or(0.0, |v| v.to_px(w)).clamp(0.0, w);
        let y = self.y.map_or(0.0, |v| v.to_px(h)).clamp(0.0, h);
        let width = self.width.map_or(w - x, |v| v.to_px(w)).min(w - x);
        let height = self.height.map_or(h - y, |v| v.to_px(h)).min(h - y);
        (width > 0.0 && height > 0.0).then_some([x, y, width, height])
    }
}

/// A transition into a clip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// Transition type.
    pub kind: TransitionKind,
    /// How long the two clips overlap.
    pub duration: Time,
    /// The color a "fade" passes through. Defaults to black. Ignored by
    /// "crossfade", which never shows a color of its own (W304).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorValue>,
}

/// Transition types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TransitionKind {
    /// Fade the incoming clip in over the outgoing one. Both are visible
    /// at once, and the sound crosses at constant power.
    Crossfade,
    /// Dip through a color: the outgoing clip fades to it over the first
    /// half of the overlap, the incoming clip out of it over the second.
    /// Only one is ever visible, and the sound goes to silence and back.
    Fade,
}

/// How a source is sized to the output frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Fit {
    /// Natural size, one source pixel per output pixel.
    #[default]
    None,
    /// Scale uniformly so the whole source fits inside the frame.
    Contain,
    /// Scale uniformly so the source covers the whole frame, cropping edges.
    Cover,
    /// Stretch to the frame, ignoring aspect ratio.
    Fill,
}

/// Blend modes, computed in linear light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum BlendMode {
    /// Source over destination.
    #[default]
    Normal,
    /// Darkens: the product of the two colors.
    Multiply,
    /// Lightens: the inverse product of the inverted colors.
    Screen,
    /// Multiplies dark areas and screens light ones, increasing contrast.
    Overlay,
    /// Keeps the darker of the two colors per channel.
    Darken,
    /// Keeps the lighter of the two colors per channel.
    Lighten,
    /// The absolute difference of the two colors.
    Difference,
    /// A gentler version of overlay.
    SoftLight,
    /// Adds the two colors; bright areas can clip.
    Add,
}

/// Position, anchor, scale and rotation of a clip.
///
/// The anchor is a point on the clip's own box, expressed as a percentage of
/// the box or in pixels of the box. It is placed at "position" in the output
/// frame; scale and rotation are applied around it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Where the anchor lands in the output frame. Defaults to the frame
    /// center ("50%", "50%").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Animated<Point>>,
    /// The point of the clip's box that is placed at "position".
    /// Percentages refer to the box size. Defaults to the box center.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Point>,
    /// Scale factor around the anchor. Defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Animated<Scale>>,
    /// Rotation around the anchor in degrees, clockwise. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<Animated<f64>>,
}

/// What a clip shows, selected by "kind".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Source {
    /// Frames (and optionally audio) from a video asset.
    Video {
        /// Id of an asset of kind "video".
        asset: String,
        /// Where in the source to start. Defaults to 0.
        #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
        in_: Option<Time>,
        /// Where in the source to stop. Defaults to the end of the file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        out: Option<Time>,
        /// Whether the asset's audio is mixed into the output. Defaults to
        /// true.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio: Option<bool>,
    },
    /// A still image asset.
    Image {
        /// Id of an asset of kind "image".
        asset: String,
    },
    /// A frame-sized rectangle of one color.
    Solid {
        /// Fill color.
        color: Animated<ColorValue>,
    },
    /// A vector shape.
    Shape {
        /// Geometry.
        shape: ShapeKind,
        /// Width of the shape's box.
        width: Length,
        /// Height of the shape's box.
        height: Length,
        /// Fill color. Defaults to white.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<Animated<ColorValue>>,
        /// Outline drawn inside the box edge.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke: Option<Stroke>,
        /// Corner radius for rectangles, in pixels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        radius: Option<f64>,
    },
    /// Styled text, optionally with per-word timing.
    Text(Box<TextSource>),
    /// A box of markup: HTML and CSS laid out with flexbox and drawn.
    ///
    /// The markup comes from `html` or from an asset of kind "html", and
    /// the styles from a `<style>` element inside it, from `css`, or
    /// both. There is no inline layout: an element's text is one
    /// paragraph and a child element is a box of its own.
    Html {
        /// Markup written in the document.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        html: Option<String>,
        /// Id of an asset of kind "html", read as the markup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        asset: Option<String>,
        /// A stylesheet applied after any `<style>` in the markup.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        css: Option<String>,
        /// Box width: a length, or "auto" to fit the content. Defaults to
        /// the frame width, so markup is laid out on a surface the size of
        /// the picture, the way a page is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<BoxSize>,
        /// Box height: a length, or "auto" to fit the content, which is
        /// how a card sizes itself to its text. Defaults to the frame
        /// height.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<BoxSize>,
    },
    /// Captions from a file: SubRip, WebVTT, or the JSON a speech
    /// recogniser writes.
    ///
    /// The resolver turns the file into one clip per cue, so a caption
    /// track costs only the frames it is on, and takes its times from
    /// the file rather than from `start` and `duration`. Where it sits
    /// comes from `position` and `margin`, inside the title-safe area by
    /// default; a WebVTT cue that asks for somewhere else is obeyed
    /// unless the document overrides it.
    Captions {
        /// Id of an asset of kind "captions".
        asset: String,
        /// Where cues sit. Defaults to the bottom.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<CaptionPosition>,
        /// Distance from the edge. Defaults to the title-safe inset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        margin: Option<Length>,
        /// Title-safe inset as a percentage of each side; a cue outside
        /// it is a note. Defaults to 5; 0 turns the note off.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        safe: Option<f64>,
        /// Whether a WebVTT file's own placement wins over the fields
        /// above. Defaults to true; set false to place every cue the
        /// same way whatever the file says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        follow_file: Option<bool>,
        /// Most lines a cue shows at once. Defaults to 2.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_lines: Option<u32>,
        /// Shortest a cue may be, so that a word lasting a fifth of a
        /// second is still readable. Defaults to 1.2s.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_duration: Option<Time>,
        /// A gap no longer than this between cues is closed rather than
        /// blinking the caption off and on. Defaults to 0.1s.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        merge_gap: Option<Time>,
        /// How the captions look: the same fields a text source takes,
        /// minus the ones that say what the text is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<Box<TextSource>>,
    },
    /// A reusable composition declared under "compositions".
    Composition {
        /// Name of the composition.
        composition: String,
    },
}

/// Shape geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ShapeKind {
    /// An axis-aligned rectangle filling the box.
    Rect,
    /// An ellipse inscribed in the box.
    Ellipse,
}

/// An outline: an object, or the shorthand `"2px black"` (see
/// [`css`](crate::css)).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stroke {
    /// Outline color.
    pub color: ColorValue,
    /// Outline width in pixels.
    pub width: f64,
}

/// A drop shadow: an object, or the `text-shadow` shorthand
/// `"0 2px 8px #0008"` (see [`css`](crate::css)).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Shadow {
    /// Shadow color. Defaults to 50% black.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorValue>,
    /// Horizontal offset in pixels.
    #[serde(default)]
    pub x: f64,
    /// Vertical offset in pixels.
    #[serde(default)]
    pub y: f64,
    /// Blur radius in pixels.
    #[serde(default)]
    pub blur: f64,
}

/// A text source.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextSource {
    /// The text to show. Required unless "words" is given, in which case it
    /// defaults to the words joined by spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Timed words for caption-style highlighting. Times are relative to the
    /// clip start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<Word>>,
    /// Style applied to the word whose time range contains the current
    /// time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub highlight: Option<TextStyle>,
    /// Base text style.
    #[serde(flatten)]
    pub style: TextStyle,
    /// Maximum line width before wrapping. Defaults to the output width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<Length>,
    /// Horizontal alignment within the box. Defaults to "center".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<TextAlign>,
    /// Line height as a multiple of the font size. Defaults to 1.2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    /// Padding between the text and its background box, in pixels.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::css::de_opt_pixels"
    )]
    #[schemars(schema_with = "crate::css::pixels_schema")]
    pub padding: Option<f64>,
    /// Background box color. Defaults to none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<ColorValue>,
    /// Background box corner radius in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    /// Text outline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Stroke>,
    /// Drop shadow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
}

/// A box dimension: a length, or `"auto"` to fit what is inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoxSize {
    /// As large as the content.
    Auto,
    /// A fixed size; percentages refer to the frame.
    Fixed(Length),
}

impl BoxSize {
    /// The size in pixels, or `None` for `auto`.
    pub fn to_px(self, reference: f64) -> Option<f64> {
        match self {
            Self::Auto => None,
            Self::Fixed(l) => Some(l.to_px(reference)),
        }
    }
}

impl Serialize for BoxSize {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Fixed(l) => l.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BoxSize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;

        impl de::Visitor<'_> for V {
            type Value = BoxSize;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a length, or \"auto\" to fit the content")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<BoxSize, E> {
                if v.trim().eq_ignore_ascii_case("auto") {
                    return Ok(BoxSize::Auto);
                }
                Length::parse(v).map(BoxSize::Fixed).map_err(E::custom)
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<BoxSize, E> {
                if v.is_finite() {
                    Ok(BoxSize::Fixed(Length::Px(v)))
                } else {
                    Err(E::custom("length must be finite"))
                }
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<BoxSize, E> {
                Ok(BoxSize::Fixed(Length::Px(v as f64)))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<BoxSize, E> {
                Ok(BoxSize::Fixed(Length::Px(v as f64)))
            }
        }

        deserializer.deserialize_any(V)
    }
}

impl JsonSchema for BoxSize {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "BoxSize".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let length = generator.subschema_for::<Length>();
        schemars::json_schema!({
            "title": "BoxSize",
            "description": "A box dimension: a length (a number of pixels, \"120px\", or a percentage of the frame), or \"auto\" to fit the content.",
            "anyOf": [length, { "const": "auto" }]
        })
    }
}

/// Where captions sit in the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CaptionPosition {
    /// Along the bottom, the usual place.
    #[default]
    Bottom,
    /// Along the top, for when the lower third is busy.
    Top,
    /// In the middle of the frame.
    Center,
}

/// One timed word.
///
/// JSON forms: the object (`{"text": "hello", "start": 0, "end": "0.5s"}`)
/// or the same three in order (`["hello", 0, "0.5s"]`, or `["hello", 0]`
/// where the next word's start is also this word's end).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    /// The word.
    pub text: String,
    /// When the word becomes current, relative to the clip start.
    pub start: Time,
    /// When the word stops being current. Defaults to the next word's
    /// start; the last word needs its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Time>,
}

impl<'de> Deserialize<'de> for Word {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The canonical object form, kept on the derive so its field errors
        /// keep their paths and spelling.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Object {
            text: String,
            start: Time,
            #[serde(default)]
            end: Option<Time>,
        }

        struct WordVisitor;

        impl<'de> de::Visitor<'de> for WordVisitor {
            type Value = Word;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "a timed word: {\"text\": .., \"start\": ..} or [text, start] or [text, start, end]",
                )
            }

            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Word, A::Error> {
                let o = Object::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(Word {
                    text: o.text,
                    start: o.start,
                    end: o.end,
                })
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Word, A::Error> {
                let text: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a word needs its text and a start time"))?;
                let start: Time = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a word needs its text and a start time"))?;
                let end: Option<Time> = seq.next_element()?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(
                        "a word in list form is [text, start] or [text, start, end], nothing longer",
                    ));
                }
                Ok(Word { text, start, end })
            }
        }

        deserializer.deserialize_any(WordVisitor)
    }
}

impl JsonSchema for Word {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Word".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let time = generator.subschema_for::<Time>();
        schemars::json_schema!({
            "title": "Word",
            "description": "One timed word: {\"text\": .., \"start\": .., \"end\": ..} or the same three in order as [text, start] or [text, start, end]. Without an end, a word runs to the next word's start; the last word needs its own.",
            "anyOf": [
                {
                    "type": "object",
                    "properties": { "text": { "type": "string" }, "start": time, "end": time },
                    "required": ["text", "start"],
                    "additionalProperties": false
                },
                { "type": "array", "minItems": 2, "maxItems": 3 }
            ]
        })
    }
}

/// Font and color properties of text.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextStyle {
    /// Font: the id of an asset of kind "font", a family name available
    /// on the system, or the CSS `font` shorthand ("600 40px/1.2 Inter"),
    /// whose parts fill in size, weight, italic and line_height unless
    /// those are set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Font size in pixels. Defaults to 48.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Weight from 100 to 900. Defaults to 400.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    /// Italic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    /// Text color. Defaults to white.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorValue>,
    /// Extra spacing between letters in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TextAlign {
    /// Left.
    Left,
    /// Center.
    Center,
    /// Right.
    Right,
}

/// An audio-only track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioTrack {
    /// Optional identifier used in diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Set to false to mute the track without removing it.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Clips in time order. A clip without "start" begins where the
    /// previous clip ends.
    pub clips: Vec<AudioClip>,
}

/// A subtitle track: one subtitle file written to the output as a text
/// stream, for players to show or hide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubtitleTrack {
    /// Optional identifier used in diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Set to false to leave the track out without removing it.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The subtitle asset.
    pub asset: String,
    /// Language as a BCP 47 or ISO 639 code, for example "en" or "pt-BR".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Track title shown by players, for example "English (CC)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Shifts every cue by this much on the output timeline. Defaults to
    /// 0; negative values move cues earlier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<Time>,
}

/// A clip on an audio track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AudioClip {
    /// Optional identifier used in diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Id of an asset of kind "audio" or "video".
    pub asset: String,
    /// Where in the source to start. Defaults to 0.
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub in_: Option<Time>,
    /// Where in the source to stop. Defaults to the end of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<Time>,
    /// When the clip begins on the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Time>,
    /// How long the clip lasts. Defaults to the source range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Time>,
    /// Gain in decibels. Defaults to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<Animated<f64>>,
    /// Fade-in duration from silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_in: Option<Time>,
    /// Fade-out duration to silence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fade_out: Option<Time>,
    /// How fast the source plays: 2 is twice as fast, 0.5 half speed.
    /// The clip lasts its source range divided by this, and the pitch
    /// follows. Defaults to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
}
