use geneva_color::ColorTags;
use geneva_timeline::Ratio;
use serde::Serialize;

/// What a media file contains, as far as its headers say.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaInfo {
    /// Container format name as reported by the demuxer.
    pub container: String,
    /// Overall duration in seconds, if the container declares one.
    pub duration: Option<Ratio>,
    /// The first video stream.
    pub video: Option<VideoInfo>,
    /// The first audio stream.
    pub audio: Option<AudioInfo>,
    /// Subtitle streams, in container order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<SubtitleInfo>,
}

/// A subtitle stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubtitleInfo {
    /// Index of the stream in the container.
    pub index: usize,
    /// Codec name.
    pub codec: String,
    /// Language tag, if the file carries one.
    pub language: Option<String>,
}

/// A video stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VideoInfo {
    /// Index of the stream in the container.
    pub index: usize,
    /// Codec name.
    pub codec: String,
    /// Width in pixels, as displayed: after the rotation the file asks
    /// for, so a portrait phone clip reads taller than wide.
    pub width: u32,
    /// Height in pixels, as displayed.
    pub height: u32,
    /// Rotation the file asks its player for, in degrees clockwise: 0,
    /// 90, 180 or 270. The coded picture is `height`×`width` when it is
    /// 90 or 270.
    pub rotation: u16,
    /// Frame rate, from the stream's average rate.
    pub fps: Ratio,
    /// Duration in seconds, if declared.
    pub duration: Option<Ratio>,
    /// Frame count, if declared.
    pub frames: Option<u64>,
    /// Pixel format name.
    pub pixel_format: String,
    /// Color tags as written in the file; missing ones are `None`.
    pub color: ColorTags,
    /// Whether the stream carries an alpha channel.
    pub has_alpha: bool,
}

/// An audio stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AudioInfo {
    /// Index of the stream in the container.
    pub index: usize,
    /// Codec name.
    pub codec: String,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Duration in seconds, if declared.
    pub duration: Option<Ratio>,
}
