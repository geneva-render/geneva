use std::path::Path;

use ffmpeg_next::codec;
use ffmpeg_next::media::Type;
use ffmpeg_next::util::format::Pixel;
use geneva_timeline::Ratio;

use super::{init, open_error, tags};
use crate::{AudioInfo, MediaError, MediaInfo, SubtitleInfo, VideoInfo};

/// Converts a libav rational to an exact ratio, or `None` for zero.
pub(super) fn ratio(r: ffmpeg_next::Rational) -> Option<Ratio> {
    if r.denominator() == 0 || r.numerator() == 0 {
        None
    } else {
        Some(Ratio::new(
            i64::from(r.numerator()),
            i64::from(r.denominator()),
        ))
    }
}

/// Timestamp in `time_base` units to seconds.
pub(super) fn ts_to_secs(ts: i64, time_base: ffmpeg_next::Rational) -> Ratio {
    Ratio::from_int(ts)
        * Ratio::new(
            i64::from(time_base.numerator()),
            i64::from(time_base.denominator()),
        )
}

/// Reads the headers of a media file.
pub fn probe(path: &Path) -> Result<MediaInfo, MediaError> {
    init();
    let ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    let container = ictx.format().name().to_owned();
    let duration = if ictx.duration() > 0 {
        Some(
            Ratio::from_int(ictx.duration())
                / Ratio::from_int(i64::from(ffmpeg_next::ffi::AV_TIME_BASE)),
        )
    } else {
        None
    };

    let video = ictx.streams().best(Type::Video).and_then(|stream| {
        let ctx = codec::context::Context::from_parameters(stream.parameters()).ok()?;
        let decoder = ctx.decoder().video().ok()?;
        let fmt = decoder.format();
        let fps = ratio(stream.avg_frame_rate())
            .or_else(|| ratio(stream.rate()))
            .unwrap_or(Ratio::from_int(25));
        Some(VideoInfo {
            index: stream.index(),
            codec: decoder
                .codec()
                .map(|c| c.name().to_owned())
                .unwrap_or_default(),
            width: decoder.width(),
            height: decoder.height(),
            fps,
            duration: (stream.duration() > 0)
                .then(|| ts_to_secs(stream.duration(), stream.time_base())),
            frames: (stream.frames() > 0).then(|| stream.frames() as u64),
            pixel_format: format!("{fmt:?}").to_ascii_lowercase(),
            color: tags::from_codec_tags(
                decoder.color_space(),
                decoder.color_range(),
                decoder.color_primaries(),
                decoder.color_transfer_characteristic(),
            ),
            has_alpha: has_alpha(fmt),
        })
    });

    let audio = ictx.streams().best(Type::Audio).and_then(|stream| {
        let ctx = codec::context::Context::from_parameters(stream.parameters()).ok()?;
        let decoder = ctx.decoder().audio().ok()?;
        Some(AudioInfo {
            index: stream.index(),
            codec: decoder
                .codec()
                .map(|c| c.name().to_owned())
                .unwrap_or_default(),
            sample_rate: decoder.rate(),
            channels: decoder.channels(),
            duration: (stream.duration() > 0)
                .then(|| ts_to_secs(stream.duration(), stream.time_base())),
        })
    });

    let subtitles = ictx
        .streams()
        .filter(|s| s.parameters().medium() == Type::Subtitle)
        .map(|s| SubtitleInfo {
            index: s.index(),
            codec: format!("{:?}", s.parameters().id()).to_ascii_lowercase(),
            language: s.metadata().get("language").map(str::to_owned),
        })
        .collect();

    Ok(MediaInfo {
        container,
        duration,
        video,
        audio,
        subtitles,
    })
}

fn has_alpha(fmt: Pixel) -> bool {
    matches!(
        fmt,
        Pixel::RGBA
            | Pixel::BGRA
            | Pixel::ARGB
            | Pixel::ABGR
            | Pixel::YUVA420P
            | Pixel::YUVA422P
            | Pixel::YUVA444P
            | Pixel::YUVA420P10LE
            | Pixel::YUVA422P10LE
            | Pixel::YUVA444P10LE
            | Pixel::YUVA444P16LE
            | Pixel::RGBA64LE
            | Pixel::BGRA64LE
            | Pixel::GBRAP
            | Pixel::GBRAP10LE
            | Pixel::GBRAP12LE
            | Pixel::GBRAP16LE
    )
}
