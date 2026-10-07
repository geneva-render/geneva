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

/// How wide a stored pixel is shown against its height: 32:27 for
/// 720×480 shown as 16:9, 1:1 for square pixels. `num` and `den` are in
/// lowest terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelAspect {
    /// Shown width of a pixel.
    pub num: u32,
    /// Shown height of a pixel.
    pub den: u32,
}

impl PixelAspect {
    /// Square pixels.
    pub const SQUARE: Self = Self { num: 1, den: 1 };

    /// The ratio in lowest terms; square for a zero or a missing side,
    /// as libav reads an unset ratio.
    #[must_use]
    pub fn new(num: u32, den: u32) -> Self {
        if num == 0 || den == 0 {
            return Self::SQUARE;
        }
        let (mut a, mut b) = (num, den);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Self {
            num: num / a,
            den: den / a,
        }
    }

    /// Whether pixels are shown square.
    #[must_use]
    pub fn is_square(self) -> bool {
        self.num == self.den
    }

    /// The size a stored picture is shown at: its width stretched by the
    /// ratio and rounded to whole pixels, as a browser's `<video>` sizes
    /// it (720×480 at 32:27 is 853×480).
    #[must_use]
    pub fn display_size(self, width: u32, height: u32) -> (u32, u32) {
        if self.is_square() {
            return (width, height);
        }
        let stretched = (u64::from(width) * u64::from(self.num) * 2 + u64::from(self.den))
            / (2 * u64::from(self.den));
        (u32::try_from(stretched).unwrap_or(u32::MAX).max(1), height)
    }
}

impl std::fmt::Display for PixelAspect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.num, self.den)
    }
}

impl Serialize for PixelAspect {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// A video stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VideoInfo {
    /// Index of the stream in the container.
    pub index: usize,
    /// Codec name.
    pub codec: String,
    /// Width in pixels, as displayed: stretched by the pixel aspect and
    /// after the rotation the file asks for, so a portrait phone clip
    /// reads taller than wide and 720×480 at 32:27 reads 853×480.
    pub width: u32,
    /// Height in pixels, as displayed.
    pub height: u32,
    /// The picture as coded in the file, before the pixel aspect and the
    /// rotation.
    pub stored_width: u32,
    /// Height of the coded picture.
    pub stored_height: u32,
    /// How wide a stored pixel is shown, `"32:27"`; `"1:1"` for square
    /// pixels.
    pub sample_aspect_ratio: PixelAspect,
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

#[cfg(test)]
mod tests {
    use super::PixelAspect;

    /// A picture is shown stretched by its pixel aspect, rounded to whole
    /// pixels as a browser sizes it; square and unset ratios leave it.
    #[test]
    fn a_pixel_aspect_stretches_the_width() {
        let ntsc_wide = PixelAspect::new(64, 54);
        assert_eq!((ntsc_wide.num, ntsc_wide.den), (32, 27));
        assert_eq!(ntsc_wide.to_string(), "32:27");
        assert_eq!(ntsc_wide.display_size(720, 480), (853, 480));
        assert_eq!(
            PixelAspect::new(4, 3).display_size(1440, 1080),
            (1920, 1080)
        );
        assert_eq!(PixelAspect::new(10, 11).display_size(720, 480), (655, 480));
        assert!(PixelAspect::new(0, 1).is_square());
        assert!(PixelAspect::new(3, 0).is_square());
        assert_eq!(PixelAspect::SQUARE.display_size(720, 480), (720, 480));
    }
}
