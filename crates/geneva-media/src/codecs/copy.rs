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
use geneva_timeline::schema::{Container, Fit, VideoCodec};
use geneva_timeline::{Composition, Ratio, ResolvedSource};

use super::ffi;
use super::probe::ts_to_secs;
use super::subtitle_streams::{SubtitleSettings, SubtitleWriter};
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
    /// Video segments in output order; empty for audio-only outputs.
    pub segments: Vec<CopySegment>,
    /// Audio segments in output order; empty when the output has no audio.
    /// They may come from other files than the video.
    pub audio: Vec<CopySegment>,
    /// Why copying is possible, for the user.
    pub reason: String,
}

/// Requested start, actual start (a keyframe) and actual end of one
/// copied segment, in source seconds.
pub type SegmentReport = (Ratio, Ratio, Ratio);

/// What a copy actually did.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyReport {
    /// One entry per copied segment.
    pub segments: Vec<SegmentReport>,
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
/// Copying applies when the visual part is one layer of video clips shown
/// as they are (natural size at the frame center, full opacity, no
/// rotation, no transitions, nothing else on top) with output size, rate
/// and codec matching the sources and compatible coded parameters across
/// them; and the audio is absent, the video clips' own audio, or a single
/// untouched audio track that spans the output. Audio-only outputs copy
/// when the audio track is one untouched clip and the source codec fits
/// the container.
pub fn plan_stream_copy(
    comp: &Composition,
    root: &Path,
    container: Container,
    requested_codec: Option<VideoCodec>,
) -> Result<Option<CopyPlan>, MediaError> {
    init();
    if comp.audio.len() > 1 {
        return Ok(None);
    }
    let Some(untouched) = untouched_video_clips(comp, root)? else {
        return Ok(None);
    };
    if !untouched.is_empty() && container.is_audio_only() {
        return Ok(None);
    }
    let mut video_segments = Vec::new();
    let mut own_audio: Option<bool> = None;
    let mut reference: Option<StreamShape> = None;
    for clip in &untouched {
        own_audio = Some(clip.audio);
        let shape = StreamShape::read(&clip.path)?;
        if let Some(code) = requested_codec {
            if shape.codec != Some(code) {
                return Ok(None);
            }
        }
        if !container_accepts(container, shape.codec) {
            return Ok(None);
        }
        if clip.audio && !shape.has_audio {
            return Ok(None);
        }
        if clip.audio
            && shape
                .audio_id
                .is_some_and(|id| !container_accepts_audio(container, id))
        {
            return Ok(None);
        }
        match &reference {
            None => reference = Some(shape),
            Some(r) if !r.compatible(&shape) => return Ok(None),
            _ => {}
        }
        video_segments.push(CopySegment {
            path: clip.path.clone(),
            from: clip.in_,
            to: Some(clip.in_ + (clip.end - clip.start)),
        });
    }

    // Audio: the clips' own audio, one untouched separate track, or none.
    let mut audio_segments = Vec::new();
    match (own_audio, comp.audio.first()) {
        (Some(true), None) => {
            audio_segments.clone_from(&video_segments);
        }
        (Some(true), Some(_)) => return Ok(None),
        (own, Some(track)) => {
            if own == Some(true) || track.clips.len() != 1 {
                return Ok(None);
            }
            let clip = &track.clips[0];
            let untouched = clip.gain_db.is_constant()
                && clip.gain_db.sample(0.0) == 0.0
                && clip.fade_in.is_zero()
                && clip.fade_out.is_zero()
                && clip.start.is_zero()
                && clip.end == comp.duration;
            if !untouched {
                return Ok(None);
            }
            let Some(asset_info) = comp.assets.get(&clip.asset) else {
                return Ok(None);
            };
            let path = root.join(&asset_info.src);
            let audio_shape = AudioShape::read(&path)?;
            if !container_accepts_audio(container, audio_shape.codec_id) {
                return Ok(None);
            }
            audio_segments.push(CopySegment {
                path,
                from: clip.in_,
                to: Some(clip.in_ + (clip.end - clip.start)),
            });
        }
        (Some(false) | None, None) => {}
    }
    if video_segments.is_empty() && audio_segments.is_empty() {
        return Ok(None);
    }
    let reason = match (video_segments.len(), audio_segments.is_empty()) {
        (0, _) => "the audio stream is used as is, so it is copied without re-encoding".to_owned(),
        (1, _) => "the video stream is used as is, so it is copied without re-encoding".to_owned(),
        (n, _) => format!(
            "all {n} sources share the same stream parameters, so they are joined without re-encoding"
        ),
    };
    Ok(Some(CopyPlan {
        segments: video_segments,
        audio: audio_segments,
        reason,
    }))
}

/// A video clip shown exactly as decoded: natural size at the frame
/// center, full opacity, no rotation or transition, normal blending, in a
/// composition whose output size and rate match the source's. When scaling
/// is allowed the clip may instead be fitted to the whole frame.
pub(super) struct UntouchedClip {
    pub path: PathBuf,
    pub asset: String,
    pub start: Ratio,
    pub end: Ratio,
    pub in_: Ratio,
    pub audio: bool,
    /// Source picture size.
    pub width: u32,
    pub height: u32,
}

/// The composition's single video layer as a contiguous list of untouched
/// clips covering the whole output, or `None` when anything would change
/// the picture. An empty list means the composition has no video layer.
pub(super) fn untouched_video_clips(
    comp: &Composition,
    root: &Path,
) -> Result<Option<Vec<UntouchedClip>>, MediaError> {
    video_layer_clips(comp, root, false)
}

/// Like [`untouched_video_clips`], but also accepts clips that are only
/// scaled: fitted so that the picture fills the whole output frame (to
/// within the rounding of an even output size), with nothing else changed.
pub(super) fn filling_video_clips(
    comp: &Composition,
    root: &Path,
) -> Result<Option<Vec<UntouchedClip>>, MediaError> {
    video_layer_clips(comp, root, true)
}

fn video_layer_clips(
    comp: &Composition,
    root: &Path,
    allow_scale: bool,
) -> Result<Option<Vec<UntouchedClip>>, MediaError> {
    if comp.layers.len() > 1 {
        return Ok(None);
    }
    let Some(layer) = comp.layers.first() else {
        return Ok(Some(Vec::new()));
    };
    let mut clips = Vec::new();
    let mut own_audio: Option<bool> = None;
    let mut expected_end = Ratio::ZERO;
    let out_w = f64::from(comp.width);
    let out_h = f64::from(comp.height);
    let center = [out_w / 2.0, out_h / 2.0];
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
        if !clip.position.is_constant() || clip.position.sample(0.0) != center {
            return Ok(None);
        }
        if clip.anchor.to_px(out_w, out_h) != center {
            return Ok(None);
        }
        match own_audio {
            None => own_audio = Some(*audio),
            Some(a) if a != *audio => return Ok(None),
            _ => {}
        }
        let Some(asset_info) = comp.assets.get(asset) else {
            return Ok(None);
        };
        let path = root.join(&asset_info.src);
        let shape = StreamShape::read(&path)?;
        if shape.fps != comp.fps {
            return Ok(None);
        }
        let same_size = shape.width == comp.width && shape.height == comp.height;
        let fills = allow_scale && fills_frame(clip.fit, shape.width, shape.height, comp);
        if !(same_size || fills) {
            return Ok(None);
        }
        clips.push(UntouchedClip {
            path,
            asset: asset.clone(),
            start: clip.start,
            end: clip.end,
            in_: *in_,
            audio: *audio,
            width: shape.width,
            height: shape.height,
        });
        expected_end = clip.end;
    }
    if clips.is_empty() || expected_end != comp.duration {
        return Ok(None);
    }
    Ok(Some(clips))
}

/// Whether a `w`×`h` picture fitted into the output covers the whole frame
/// to within two pixels on each axis: the slack that rounding an output
/// size to even numbers leaves (854×480 at height 360 is 640.5 wide,
/// written as 642), where the renderer would show at most a one-pixel
/// edge and a scaler stretches by a fraction of a percent instead.
fn fills_frame(fit: Fit, w: u32, h: u32, comp: &Composition) -> bool {
    let (w, h) = (f64::from(w), f64::from(h));
    let (out_w, out_h) = (f64::from(comp.width), f64::from(comp.height));
    let scale = match fit {
        Fit::None => [1.0, 1.0],
        Fit::Contain => {
            let s = (out_w / w).min(out_h / h);
            [s, s]
        }
        Fit::Cover => {
            let s = (out_w / w).max(out_h / h);
            [s, s]
        }
        Fit::Fill => [out_w / w, out_h / h],
    };
    (w * scale[0] - out_w).abs() <= 2.0 && (h * scale[1] - out_h).abs() <= 2.0
}

/// Audio codecs each container can hold without re-encoding.
fn container_accepts_audio(container: Container, codec: codec::Id) -> bool {
    use codec::Id;
    match container {
        Container::Mp4 | Container::M4a => {
            matches!(
                codec,
                Id::AAC | Id::MP3 | Id::ALAC | Id::AC3 | Id::EAC3 | Id::OPUS | Id::FLAC
            )
        }
        Container::Mov => matches!(
            codec,
            Id::AAC
                | Id::MP3
                | Id::ALAC
                | Id::AC3
                | Id::EAC3
                | Id::OPUS
                | Id::FLAC
                | Id::PCM_S16LE
                | Id::PCM_S24LE
                | Id::PCM_F32LE
        ),
        Container::Mkv => true,
        Container::Webm => matches!(codec, Id::OPUS | Id::VORBIS),
        Container::Ogg => matches!(codec, Id::OPUS | Id::VORBIS | Id::FLAC),
        Container::Flac => codec == Id::FLAC,
        Container::Mp3 => codec == Id::MP3,
        Container::Wav | Container::Mxf => {
            matches!(codec, Id::PCM_S16LE | Id::PCM_S24LE | Id::PCM_F32LE)
        }
        Container::ImageSequence => false,
    }
}

/// Coded parameters of an audio stream, for copy decisions.
struct AudioShape {
    codec_id: codec::Id,
}

impl AudioShape {
    fn read(path: &Path) -> Result<Self, MediaError> {
        let ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
        let audio = ictx
            .streams()
            .best(Type::Audio)
            .ok_or_else(|| MediaError::NoStream {
                path: path.to_owned(),
                kind: "audio",
            })?;
        Ok(Self {
            codec_id: audio.parameters().id(),
        })
    }
}

fn container_accepts(container: Container, codec: Option<VideoCodec>) -> bool {
    match codec {
        None => false,
        // Image sequences are written frame by frame, never copied.
        Some(_) if container == Container::ImageSequence => false,
        Some(c) => super::encode::container_accepts_video(container, c),
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
                codec::Id::PRORES => Some(VideoCodec::Prores),
                codec::Id::DNXHD => Some(VideoCodec::Dnxhd),
                codec::Id::MJPEG => Some(VideoCodec::Mjpeg),
                codec::Id::PNG => Some(VideoCodec::Png),
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
pub fn stream_copy(
    plan: &CopyPlan,
    output: &Path,
    subtitles: &[SubtitleSettings],
) -> Result<CopyReport, MediaError> {
    init();
    let container = super::encode::container_for(output, None);
    let mut octx = match container.and_then(super::encode::muxer_name) {
        Some(name) => {
            ffmpeg_next::format::output_as(output, name).map_err(|e| open_error(output, e))?
        }
        None => ffmpeg_next::format::output(output).map_err(|e| open_error(output, e))?,
    };

    // Output streams take their parameters from the first source of each kind.
    let out_video = match plan.segments.first() {
        Some(first) => {
            let template =
                ffmpeg_next::format::input(&first.path).map_err(|e| open_error(&first.path, e))?;
            let video_in =
                template
                    .streams()
                    .best(Type::Video)
                    .ok_or_else(|| MediaError::NoStream {
                        path: first.path.clone(),
                        kind: "video",
                    })?;
            Some(add_copied_stream(&mut octx, &video_in, output)?)
        }
        None => None,
    };
    let out_audio = match plan.audio.first() {
        Some(first) => {
            let template =
                ffmpeg_next::format::input(&first.path).map_err(|e| open_error(&first.path, e))?;
            let audio_in =
                template
                    .streams()
                    .best(Type::Audio)
                    .ok_or_else(|| MediaError::NoStream {
                        path: first.path.clone(),
                        kind: "audio",
                    })?;
            Some(add_copied_stream(&mut octx, &audio_in, output)?)
        }
        None => None,
    };
    if out_video.is_none() && out_audio.is_none() {
        return Err(MediaError::Codec {
            context: "stream copy".to_owned(),
            reason: "nothing to copy".to_owned(),
        });
    }
    let mut cues = SubtitleWriter::add_streams(&mut octx, container, subtitles, output)?;
    octx.write_header().map_err(|e| open_error(output, e))?;

    let mut report_segments = Vec::new();
    let mut video_packets = 0u64;
    let mut duration = Ratio::ZERO;
    if let Some(out_idx) = out_video {
        let (segs, total) = copy_track(
            &mut octx,
            &plan.segments,
            Type::Video,
            out_idx,
            &mut video_packets,
            &mut cues,
        )?;
        report_segments = segs;
        duration = total;
    }
    if let Some(out_idx) = out_audio {
        let mut ignored = 0u64;
        let (segs, total) = copy_track(
            &mut octx,
            &plan.audio,
            Type::Audio,
            out_idx,
            &mut ignored,
            &mut cues,
        )?;
        if out_video.is_none() {
            report_segments = segs;
            duration = total;
        }
    }
    cues.finish(&mut octx)?;
    octx.write_trailer().map_err(|e| open_error(output, e))?;
    Ok(CopyReport {
        segments: report_segments,
        duration,
        video_packets,
    })
}

/// Copies one kind of stream from a list of segments into output stream
/// `out_idx`, returning the per-segment report and the total duration.
fn copy_track(
    octx: &mut ffmpeg_next::format::context::Output,
    segments: &[CopySegment],
    kind: Type,
    out_idx: usize,
    packets: &mut u64,
    cues: &mut SubtitleWriter,
) -> Result<(Vec<SegmentReport>, Ratio), MediaError> {
    let mut report = Vec::new();
    let mut offset = Ratio::ZERO;
    // Cues are written alongside the video packets, or the audio ones when
    // the output has no video.
    let cues_follow_video = octx
        .streams()
        .any(|s| s.parameters().medium() == Type::Video);
    let out_tb = octx.stream(out_idx).expect("added").time_base();
    let out_scale = Ratio::new(
        i64::from(out_tb.denominator()),
        i64::from(out_tb.numerator()),
    );
    for segment in segments {
        let mut ictx =
            ffmpeg_next::format::input(&segment.path).map_err(|e| open_error(&segment.path, e))?;
        let (in_idx, tb, start) = {
            let s = ictx.streams().best(kind).expect("checked by the plan");
            (s.index(), s.time_base(), s.start_time().max(0))
        };
        if segment.from > Ratio::ZERO {
            let micros = (segment.from.to_f64() * 1_000_000.0) as i64;
            ictx.seek(micros, ..micros).map_err(|e| {
                super::codec_error(format!("{}: seeking", segment.path.display()), e)
            })?;
        }
        // The segment starts at the first keyframe the demuxer yields (every
        // audio packet is one); everything is timed relative to it.
        let mut segment_start: Option<Ratio> = None;
        let mut last_end = segment.from;
        let mut packet = Packet::empty();
        while packet.read(&mut ictx).is_ok() {
            if packet.stream() != in_idx {
                continue;
            }
            let Some(pts) = packet.pts().or(packet.dts()) else {
                continue;
            };
            let time = ts_to_secs(pts - start, tb);
            if segment_start.is_none() {
                if !packet.is_key() {
                    continue;
                }
                segment_start = Some(time);
            }
            let seg_start = segment_start.expect("set above");
            if time < seg_start {
                continue;
            }
            if segment.to.is_some_and(|to| time >= to) {
                break;
            }
            let packet_duration = if packet.duration() > 0 {
                ts_to_secs(packet.duration(), tb)
            } else {
                Ratio::ZERO
            };
            last_end = last_end.max(time + packet_duration);
            let shifted = time - seg_start + offset;
            let out_pts = (shifted * out_scale).round();
            let out_dts = match packet.dts() {
                Some(d) => ((shifted + ts_to_secs(d - pts, tb)) * out_scale).round(),
                None => out_pts,
            };
            packet.set_stream(out_idx);
            packet.set_pts(Some(out_pts));
            packet.set_dts(Some(out_dts));
            if packet.duration() > 0 {
                packet.set_duration(rescale(packet.duration(), tb, out_tb));
            }
            packet.set_position(-1);
            packet
                .write_interleaved(octx)
                .map_err(|e| super::codec_error("writing copied packet", e))?;
            *packets += 1;
            if kind == Type::Video || !cues_follow_video {
                cues.write_due(octx, shifted)?;
            }
        }
        let seg_start = segment_start.unwrap_or(segment.from);
        let seg_end = segment
            .to
            .map_or(last_end, |t| t.min(last_end).max(seg_start));
        // Packets before time zero are encoder priming, not a moved cut.
        report.push((segment.from, seg_start.max(Ratio::ZERO), seg_end));
        offset = offset + (seg_end - seg_start);
    }
    Ok((report, offset))
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
