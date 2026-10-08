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
    let mut ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    let rate = video_frame_rate(&mut ictx);
    let container = ictx.format().name().to_owned();
    // MPEG program and transport streams have no duration in their
    // headers; the demuxer estimates one from the bit rate or the last
    // timestamps it reads, which with B-frames is not the last frame
    // shown. Their video is measured to its end instead.
    let measured = if matches!(container.as_str(), "mpeg" | "mpegts" | "mpegvideo") {
        measured_video_length(&mut ictx)
    } else {
        None
    };
    let duration = measured.or_else(|| {
        (ictx.duration() > 0).then(|| {
            Ratio::from_int(ictx.duration())
                / Ratio::from_int(i64::from(ffmpeg_next::ffi::AV_TIME_BASE))
        })
    });

    let mut undecodable = Vec::new();
    for (kind, medium) in [("video", Type::Video), ("audio", Type::Audio)] {
        let Some(stream) = ictx.streams().best(medium) else {
            continue;
        };
        let id = stream.parameters().id();
        let reason = match ffmpeg_next::decoder::find(id) {
            None => Some("no decoder for it in this build".to_owned()),
            Some(found) => codec::context::Context::from_parameters(stream.parameters())
                .and_then(|ctx| ctx.decoder().open_as(found).map(drop))
                .err()
                .map(|e| format!("its decoder would not open: {e}")),
        };
        if let Some(reason) = reason {
            undecodable.push(crate::UndecodableStream {
                index: stream.index(),
                kind,
                codec: id.name().to_owned(),
                reason,
            });
        }
    }

    let video = ictx.streams().best(Type::Video).and_then(|stream| {
        let ctx = codec::context::Context::from_parameters(stream.parameters()).ok()?;
        let decoder = ctx.decoder().video().ok()?;
        let fmt = decoder.format();
        let fps = rate.unwrap_or(Ratio::from_int(25));
        let rotation = super::ffi::display_rotation(&stream.parameters());
        let aspect = super::ffi::sample_aspect_ratio(&stream);
        let (shown_w, shown_h) = aspect.display_size(decoder.width(), decoder.height());
        let (width, height) = if rotation % 180 == 90 {
            (shown_h, shown_w)
        } else {
            (shown_w, shown_h)
        };
        Some(VideoInfo {
            index: stream.index(),
            codec: decoder
                .codec()
                .map(|c| c.name().to_owned())
                .unwrap_or_default(),
            width,
            height,
            stored_width: decoder.width(),
            stored_height: decoder.height(),
            sample_aspect_ratio: aspect,
            rotation,
            fps,
            duration: measured.or_else(|| {
                (stream.duration() > 0).then(|| ts_to_secs(stream.duration(), stream.time_base()))
            }),
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
        undecodable,
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

/// The frame rate of the file's video stream: what it declares
/// ([`frame_rate`]), or, where it declares nothing worth having, the mean
/// spacing of its first packets' timestamps, snapped to a standard rate
/// within 1%. An ASF (WMV) stream declares its millisecond time base as
/// its rate and no average, so 1000 fps for a 10 fps file. Reading the
/// packets moves the input, which is sought back to the start.
pub(super) fn video_frame_rate(ictx: &mut ffmpeg_next::format::context::Input) -> Option<Ratio> {
    const STANDARD: [(i64, i64); 13] = [
        (24000, 1001),
        (24, 1),
        (25, 1),
        (30000, 1001),
        (30, 1),
        (50, 1),
        (60000, 1001),
        (60, 1),
        (15, 1),
        (12, 1),
        (10, 1),
        (8, 1),
        (5, 1),
    ];
    let stream = ictx.streams().best(ffmpeg_next::media::Type::Video)?;
    let (index, time_base) = (stream.index(), stream.time_base());
    let declared = frame_rate(ratio(stream.avg_frame_rate()), ratio(stream.rate()));
    if declared.is_some_and(|r| (1.0..=240.0).contains(&r.to_f64())) {
        return declared;
    }
    let mut stamps: Vec<i64> = Vec::new();
    for (s, packet) in ictx.packets() {
        if s.index() == index {
            stamps.extend(packet.pts().or(packet.dts()));
            if stamps.len() >= 120 {
                break;
            }
        }
    }
    let _ = seek_before(ictx, index, Ratio::ZERO, time_base);
    stamps.sort_unstable();
    let (first, last) = (*stamps.first()?, *stamps.last()?);
    let span = ts_to_secs(last - first, time_base).to_f64();
    if stamps.len() < 2 || span <= 0.0 {
        return declared;
    }
    let fps = (stamps.len() - 1) as f64 / span;
    let snapped = STANDARD
        .iter()
        .map(|&(n, d)| Ratio::new(n, d))
        .find(|r| (r.to_f64() - fps).abs() <= r.to_f64() * 0.01);
    Some(snapped.unwrap_or_else(|| Ratio::new((fps * 1000.0).round() as i64, 1000)))
}

/// How long the video of a file without a reliable duration lasts, from
/// its first frame to the end of its last shown one. The last seconds are
/// decoded and the latest frame time plus a frame is the end: packets
/// alone cannot tell, since a program stream's reference frames carry
/// only a decode time and are shown after the B-frames that follow them.
/// The input is left wherever reading stopped.
fn measured_video_length(ictx: &mut ffmpeg_next::format::context::Input) -> Option<Ratio> {
    let stream = ictx.streams().best(Type::Video)?;
    let (index, time_base) = (stream.index(), stream.time_base());
    let start = stream.start_time();
    if start == i64::MIN {
        return None;
    }
    let frame = frame_rate(ratio(stream.avg_frame_rate()), ratio(stream.rate()))
        .map_or(Ratio::new(1, 25), Ratio::recip);
    let mut decoder = codec::context::Context::from_parameters(stream.parameters())
        .ok()?
        .decoder()
        .video()
        .ok()?;
    let estimate = ictx.duration().max(0);
    let back = i64::from(ffmpeg_next::ffi::AV_TIME_BASE) * 3;
    let start_us = (ts_to_secs(start, time_base).to_f64() * 1_000_000.0) as i64;
    let from = start_us.max(0) + (estimate - back).max(0);
    if ictx.seek(from, ..from).is_err() {
        let _ = ictx.seek(0, ..0);
    }
    // The latest frame time, and how many frames came out after it with
    // none, or with an earlier one (a reference frame timed by its decode
    // time): each follows the one before.
    let mut last: Option<(i64, i64)> = None;
    let mut decoded = ffmpeg_next::frame::Video::empty();
    let mut drain = |decoder: &mut ffmpeg_next::decoder::Video, last: &mut Option<(i64, i64)>| {
        while decoder.receive_frame(&mut decoded).is_ok() {
            *last = match (decoded.timestamp(), *last) {
                (Some(t), Some((l, n))) if t <= l => Some((l, n + 1)),
                (Some(t), _) => Some((t, 0)),
                (None, Some((l, n))) => Some((l, n + 1)),
                (None, None) => None,
            };
        }
    };
    for (s, packet) in ictx.packets() {
        if s.index() == index && decoder.send_packet(&packet).is_ok() {
            drain(&mut decoder, &mut last);
        }
    }
    let _ = decoder.send_eof();
    drain(&mut decoder, &mut last);
    let (latest, untimed) = last?;
    let end = ts_to_secs(latest - start, time_base) + frame * Ratio::from_int(untimed + 1);
    (end > Ratio::ZERO).then_some(end)
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

/// The static HDR metadata of a file's video stream, if it carries any.
pub fn hdr_metadata_of(path: &Path) -> Result<Option<super::ffi::HdrMetadata>, MediaError> {
    super::init();
    let ictx = ffmpeg_next::format::input(path).map_err(|e| super::open_error(path, e))?;
    Ok(ictx
        .streams()
        .best(Type::Video)
        .and_then(|s| super::ffi::hdr_metadata(&s.parameters())))
}
