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
        let fps = frame_rate(ratio(stream.avg_frame_rate()), ratio(stream.rate()))
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

/// The frame rate to work at, from the stream's average rate (frames
/// over duration) and its base rate (the timestamp grid). When they
/// agree, the base rate is the exact one (24/1, 30000/1001) while the
/// average carries the rounding of the file's timestamps and length
/// (a millisecond grid makes 24 fps read as 24.016). On a
/// variable-rate file the average is not a rate any frame has, so the
/// base rate is used there too, unless it is the doubled figure some
/// H.264 streams carry (field rate) or nonsense, which the average
/// corrects.
pub(super) fn frame_rate(average: Option<Ratio>, base: Option<Ratio>) -> Option<Ratio> {
    match (average, base) {
        (Some(avg), Some(base)) => {
            let (a, b) = (avg.to_f64(), base.to_f64());
            let doubled = (b - 2.0 * a).abs() <= a * 0.01;
            if doubled || !(1.0..=240.0).contains(&b) {
                Some(avg)
            } else {
                Some(base)
            }
        }
        (avg, base) => avg.or(base),
    }
}

/// Seeks `ictx` so that reading resumes at or before `target` (seconds
/// in the file's own timestamps) on stream `stream_index`. Indexed
/// containers land on a keyframe at or before the target; MPEG-TS has
/// no index and can land past it, in which case the seek is repeated
/// a few seconds earlier and the caller reads forward from there.
pub(super) fn seek_before(
    ictx: &mut ffmpeg_next::format::context::Input,
    stream_index: usize,
    target: Ratio,
    time_base: ffmpeg_next::Rational,
) -> Result<(), ffmpeg_next::Error> {
    let micros = |t: Ratio| (t.to_f64() * 1_000_000.0) as i64;
    ictx.seek(micros(target), ..micros(target))?;
    // Where did it land? The packet read here is read again after the
    // seek below.
    let mut packet = ffmpeg_next::Packet::empty();
    let mut landed = None;
    while packet.read(ictx).is_ok() {
        if packet.stream() == stream_index {
            landed = packet.pts().or(packet.dts());
            break;
        }
    }
    let late = landed.is_none_or(|ts| ts_to_secs(ts, time_base) > target);
    let again = if late && target > Ratio::ZERO {
        (target - Ratio::from_int(3)).max(Ratio::ZERO)
    } else {
        target
    };
    ictx.seek(micros(again), ..micros(again))
}
