//! Stream copying: writing a file from the coded packets of others,
//! without decoding, for cuts and joins that need no re-encoding.
//!
//! Cuts can only happen at keyframes of the source, so the copied range is
//! widened to the keyframe at or before the requested start; the report
//! says exactly which times were used.

use std::path::{Path, PathBuf};

use ffmpeg_next::codec;
use ffmpeg_next::media::Type;
use ffmpeg_next::{Packet, Rational};
use geneva_timeline::schema::{Container, VideoCodec};
use geneva_timeline::{Composition, Ratio, ResolvedSource};

use super::ffi;
use super::probe::ts_to_secs;
use super::{init, open_error};
use crate::MediaError;

/// One piece of an input file to copy.
#[derive(Debug, Clone, PartialEq)]
pub struct CopySegment {
    /// Source file.
    pub path: PathBuf,
    /// Requested start in the source, in seconds.
    pub from: Ratio,
    /// Requested end in the source, in seconds, if bounded.
    pub to: Option<Ratio>,
}

/// A decision that the composition can be produced by copying packets.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyPlan {
    /// Segments in output order.
    pub segments: Vec<CopySegment>,
    /// Whether the sources' audio is copied too.
    pub audio: bool,
    /// Why copying is possible, for the user.
    pub reason: String,
}

/// What a copy actually did.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyReport {
    /// Per segment: requested start, actual start (a keyframe), and the
    /// actual end, in source seconds.
    pub segments: Vec<(Ratio, Ratio, Ratio)>,
    /// Duration of the output in seconds.
    pub duration: Ratio,
    /// Number of video packets written.
    pub video_packets: u64,
}

impl CopyReport {
    /// Human-readable notes about cuts that moved to keyframes.
    pub fn notes(&self) -> Vec<String> {
        self.segments
            .iter()
            .filter(|(req, actual, _)| req != actual)
            .map(|(req, actual, _)| format!("cut at {req}s moved to the keyframe at {actual}s"))
            .collect()
    }
}

/// Decides whether `comp` can be written by copying packets from its
/// sources, and returns the plan when it can.
///
/// Copying applies when the composition is one visual layer of video clips
/// shown at natural size and full opacity with nothing else on top, the
/// output matches the sources' size, rate and codec, every source has
/// compatible coded parameters, and audio is either absent or passed
/// through untouched.
pub fn plan_stream_copy(
    comp: &Composition,
    root: &Path,
    container: Container,
    requested_codec: Option<VideoCodec>,
) -> Result<Option<CopyPlan>, MediaError> {
    init();
    if comp.layers.len() != 1 {
        return Ok(None);
    }
    let layer = &comp.layers[0];
    if layer.clips.is_empty() {
        return Ok(None);
    }
    // Audio: either no audio at all, or exactly the clips' own audio.
    if !comp.audio.is_empty() {
        return Ok(None);
    }
    let mut segments = Vec::new();
    let mut wants_audio: Option<bool> = None;
    let mut expected_end = Ratio::ZERO;
    let mut reference: Option<StreamShape> = None;
    for clip in &layer.clips {
        let ResolvedSource::Video { asset, in_, audio } = &clip.source else {
            return Ok(None);
        };
        if clip.start != expected_end || clip.transition_in.is_some() {
            return Ok(None);
        }
        if !clip.opacity.is_constant() || clip.opacity.sample(0.0) < 1.0 {
            return Ok(None);
        }
        if !clip.scale.is_constant() || clip.scale.sample(0.0) != [1.0, 1.0] {
            return Ok(None);
        }
        if !clip.rotation.is_constant() || clip.rotation.sample(0.0) != 0.0 {
            return Ok(None);
        }
        if clip.blend != geneva_timeline::schema::BlendMode::Normal {
            return Ok(None);
        }
        match wants_audio {
            None => wants_audio = Some(*audio),
            Some(a) if a != *audio => return Ok(None),
            _ => {}
        }
        let Some(asset_info) = comp.assets.get(asset) else {
            return Ok(None);
        };
        let path = root.join(&asset_info.src);
        let shape = StreamShape::read(&path)?;
        // Natural size at the frame center means the clip covers the
        // output exactly when sizes match; any fit mode gives the same
        // placement at equal sizes.
        if shape.width != comp.width || shape.height != comp.height || shape.fps != comp.fps {
            return Ok(None);
        }
        let center = [f64::from(comp.width) / 2.0, f64::from(comp.height) / 2.0];
        if !clip.position.is_constant() || clip.position.sample(0.0) != center {
            return Ok(None);
        }
        if clip
            .anchor
            .to_px(f64::from(comp.width), f64::from(comp.height))
            != center
        {
            return Ok(None);
        }
        if let Some(code) = requested_codec {
            if shape.codec != Some(code) {
                return Ok(None);
            }
        }
        if !container_accepts(container, shape.codec) {
            return Ok(None);
        }
        if wants_audio == Some(true) && !shape.has_audio {
            return Ok(None);
        }
        match &reference {
            None => reference = Some(shape),
            Some(r) if !r.compatible(&shape) => return Ok(None),
            _ => {}
        }
        segments.push(CopySegment {
            path,
            from: *in_,
            to: Some(*in_ + clip.duration()),
        });
        expected_end = clip.end;
    }
    if expected_end != comp.duration {
        return Ok(None);
    }
    let codec = reference
        .and_then(|r| r.codec)
        .map_or("video".to_owned(), |c| format!("{c:?}").to_lowercase());
    let reason = if segments.len() == 1 {
        format!("the {codec} stream is used as is, so it is copied without re-encoding")
    } else {
        format!(
            "all {} sources share the same {codec} stream parameters, so they are joined without re-encoding",
            segments.len()
        )
    };
    Ok(Some(CopyPlan {
        segments,
        audio: wants_audio.unwrap_or(false),
        reason,
    }))
}

fn container_accepts(container: Container, codec: Option<VideoCodec>) -> bool {
    match codec {
        None => false,
        Some(VideoCodec::Vp9 | VideoCodec::Av1) => true,
        Some(VideoCodec::H264 | VideoCodec::H265) => container != Container::Webm,
    }
}

/// The coded parameters that must agree between joined sources.
#[derive(Debug, Clone, PartialEq)]
struct StreamShape {
    codec: Option<VideoCodec>,
    codec_id: codec::Id,
    width: u32,
    height: u32,
    fps: Ratio,
    format: ffmpeg_next::util::format::Pixel,
    extradata: Vec<u8>,
    has_audio: bool,
    audio_id: Option<codec::Id>,
    audio_extradata: Vec<u8>,
}

impl StreamShape {
    fn read(path: &Path) -> Result<Self, MediaError> {
        let ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
        let video = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| MediaError::NoStream {
                path: path.to_owned(),
                kind: "video",
            })?;
        let params = video.parameters();
        let ctx = codec::context::Context::from_parameters(params.clone()).map_err(|e| {
            super::codec_error(format!("{}: reading stream parameters", path.display()), e)
        })?;
        let decoder = ctx.decoder().video().map_err(|e| {
            super::codec_error(format!("{}: reading stream parameters", path.display()), e)
        })?;
        let fps = super::probe::ratio(video.avg_frame_rate())
            .or_else(|| super::probe::ratio(video.rate()))
            .unwrap_or(Ratio::from_int(25));
        let audio = ictx.streams().best(Type::Audio);
        Ok(Self {
            codec: match params.id() {
                codec::Id::H264 => Some(VideoCodec::H264),
                codec::Id::HEVC => Some(VideoCodec::H265),
                codec::Id::VP9 => Some(VideoCodec::Vp9),
                codec::Id::AV1 => Some(VideoCodec::Av1),
                _ => None,
            },
            codec_id: params.id(),
            width: decoder.width(),
            height: decoder.height(),
            fps,
            format: decoder.format(),
            extradata: ffi::extradata(&params),
            has_audio: audio.is_some(),
            audio_id: audio.as_ref().map(|a| a.parameters().id()),
            audio_extradata: audio
                .as_ref()
                .map(|a| ffi::extradata(&a.parameters()))
                .unwrap_or_default(),
        })
    }

    fn compatible(&self, other: &Self) -> bool {
        self.codec_id == other.codec_id
            && self.width == other.width
            && self.height == other.height
            && self.format == other.format
            && self.extradata == other.extradata
            && self.audio_id == other.audio_id
            && self.audio_extradata == other.audio_extradata
    }
}

/// Writes `output` by copying the planned segments' packets.
pub fn stream_copy(plan: &CopyPlan, output: &Path) -> Result<CopyReport, MediaError> {
    init();
    let first = plan.segments.first().ok_or_else(|| MediaError::Codec {
        context: "stream copy".to_owned(),
        reason: "no segments to copy".to_owned(),
    })?;
    let mut octx = ffmpeg_next::format::output(output).map_err(|e| open_error(output, e))?;

    // Output streams take their parameters from the first source.
    let template =
        ffmpeg_next::format::input(&first.path).map_err(|e| open_error(&first.path, e))?;
    let video_in = template
        .streams()
        .best(Type::Video)
        .ok_or_else(|| MediaError::NoStream {
            path: first.path.clone(),
            kind: "video",
        })?;
    let out_video = add_copied_stream(&mut octx, &video_in, output)?;
    let out_audio = if plan.audio {
        let audio_in =
            template
                .streams()
                .best(Type::Audio)
                .ok_or_else(|| MediaError::NoStream {
                    path: first.path.clone(),
                    kind: "audio",
                })?;
        Some(add_copied_stream(&mut octx, &audio_in, output)?)
    } else {
        None
    };
    drop(template);
    octx.write_header().map_err(|e| open_error(output, e))?;

    let mut report_segments = Vec::new();
    let mut video_packets = 0u64;
    // Running offset of the output timeline, in seconds.
    let mut offset = Ratio::ZERO;
    for segment in &plan.segments {
        let mut ictx =
            ffmpeg_next::format::input(&segment.path).map_err(|e| open_error(&segment.path, e))?;
        let (vid_idx, vid_tb, vid_start) = {
            let s = ictx
                .streams()
                .best(Type::Video)
                .expect("checked by the plan");
            (s.index(), s.time_base(), s.start_time().max(0))
        };
        let aud = if plan.audio {
            ictx.streams()
                .best(Type::Audio)
                .map(|s| (s.index(), s.time_base(), s.start_time().max(0)))
        } else {
            None
        };
        if segment.from > Ratio::ZERO {
            let micros = (segment.from.to_f64() * 1_000_000.0) as i64;
            ictx.seek(micros, ..micros).map_err(|e| {
                super::codec_error(format!("{}: seeking", segment.path.display()), e)
            })?;
        }
        let out_vtb = octx.stream(out_video).expect("added").time_base();
        let out_atb = out_audio.map(|i| octx.stream(i).expect("added").time_base());

        // The segment starts at the first video keyframe the demuxer
        // yields; everything is timed relative to it.
        let mut segment_start: Option<Ratio> = None;
        let mut last_end = segment.from;
        let mut packet = Packet::empty();
        while packet.read(&mut ictx).is_ok() {
            let idx = packet.stream();
            let (tb, start) = if idx == vid_idx {
                (vid_tb, vid_start)
            } else if aud.is_some_and(|(i, _, _)| i == idx) {
                let (_, tb, start) = aud.expect("checked");
                (tb, start)
            } else {
                continue;
            };
            let Some(pts) = packet.pts().or(packet.dts()) else {
                continue;
            };
            let time = ts_to_secs(pts - start, tb);
            if idx == vid_idx && segment_start.is_none() {
                if !packet.is_key() {
                    continue;
                }
                segment_start = Some(time);
            }
            let Some(seg_start) = segment_start else {
                continue;
            };
            if time < seg_start {
                continue;
            }
            if let Some(to) = segment.to {
                if time >= to {
                    if idx == vid_idx {
                        break;
                    }
                    continue;
                }
            }
            let duration = if packet.duration() > 0 {
                ts_to_secs(packet.duration(), tb)
            } else {
                Ratio::ZERO
            };
            last_end = last_end.max(time + duration);
            let shifted = time - seg_start + offset;
            let (out_idx, out_tb) = if idx == vid_idx {
                (out_video, out_vtb)
            } else {
                (
                    out_audio.expect("audio planned"),
                    out_atb.expect("audio planned"),
                )
            };
            let out_pts = (shifted
                * Ratio::new(
                    i64::from(out_tb.denominator()),
                    i64::from(out_tb.numerator()),
                ))
            .round();
            let dts_shift = packet.dts().map(|d| ts_to_secs(d - pts, tb));
            packet.set_stream(out_idx);
            packet.set_pts(Some(out_pts));
            packet.set_dts(Some(match dts_shift {
                Some(shift) => ((shifted + shift)
                    * Ratio::new(
                        i64::from(out_tb.denominator()),
                        i64::from(out_tb.numerator()),
                    ))
                .round(),
                None => out_pts,
            }));
            let dur = packet.duration();
            if dur > 0 {
                packet.set_duration(rescale(dur, tb, out_tb));
            }
            packet.set_position(-1);
            packet
                .write_interleaved(&mut octx)
                .map_err(|e| super::codec_error("writing copied packet", e))?;
            if idx == vid_idx {
                video_packets += 1;
            }
        }
        let seg_start = segment_start.unwrap_or(segment.from);
        let seg_end = segment
            .to
            .map_or(last_end, |t| t.min(last_end).max(seg_start));
        report_segments.push((segment.from, seg_start, seg_end));
        offset = offset + (seg_end - seg_start);
    }
    octx.write_trailer().map_err(|e| open_error(output, e))?;
    Ok(CopyReport {
        segments: report_segments,
        duration: offset,
        video_packets,
    })
}

fn rescale(value: i64, from: Rational, to: Rational) -> i64 {
    let r = Ratio::from_int(value)
        * Ratio::new(i64::from(from.numerator()), i64::from(from.denominator()))
        * Ratio::new(i64::from(to.denominator()), i64::from(to.numerator()));
    r.round()
}

fn add_copied_stream(
    octx: &mut ffmpeg_next::format::context::Output,
    input: &ffmpeg_next::format::stream::Stream,
    output: &Path,
) -> Result<usize, MediaError> {
    let mut params = input.parameters();
    ffi::clear_codec_tag(&mut params);
    let ctx = codec::context::Context::from_parameters(params)
        .map_err(|e| super::codec_error("copying stream parameters", e))?;
    let mut stream = octx
        .add_stream_with(&ctx)
        .map_err(|e| open_error(output, e))?;
    stream.set_time_base(input.time_base());
    if input.parameters().medium() == Type::Video {
        stream.set_avg_frame_rate(input.avg_frame_rate());
        stream.set_rate(input.rate());
    }
    Ok(stream.index())
}
