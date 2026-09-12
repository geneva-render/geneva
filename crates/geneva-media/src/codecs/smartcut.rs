//! Smart cut: writing a composition that shows its H.264 sources as they
//! are by copying their coded packets wherever nothing changes, and
//! encoding only the frames that do (the run from an exact cut to the
//! next keyframe, the frames an overlay touches) into the same stream.
//!
//! The copied stretches and the encoded runs share one track. Each copied
//! stretch starts at an IDR picture of the source and ends where the
//! source's decode order is clean (every picture before the boundary is
//! shown before every picture after it), so no picture is left without
//! its references. The encoded runs come from the system's x264 with
//! parameter sets under an id the source does not use, so both sets sit
//! side by side in the container header and every picture names its own.

use std::ops::Range;
use std::path::{Path, PathBuf};

use ffmpeg_next::media::Type;
use ffmpeg_next::{Packet, Rational, codec};
use geneva_color::ResolvedTags;
use geneva_timeline::schema::{Container, VideoCodec};
use geneva_timeline::{Composition, Ratio};

use super::copy::{base_video_clips, container_accepts_audio};
use super::decode::VideoReader;
use super::h264::{self, ParameterSets};
use super::probe::{ratio, seek_before, ts_to_secs};
use super::{codec_error, ffi, init, open_error, x264};
use crate::MediaError;

/// Shortest stretch worth copying, in frames: below this the parameter
/// set switches cost more than the encode they save.
const MIN_COPY_FRAMES: usize = 2;
/// The frames copied must be at least this fraction of all frames for
/// the path to pay: its encoded runs go at a higher quality than a plain
/// encode, so with little to copy the plain encode is faster.
const MIN_COPY_SHARE: u64 = 5;

/// One coded picture of a source, in decode order.
#[derive(Debug, Clone)]
pub struct IndexedPacket {
    /// Presentation timestamp in the stream's time base.
    pub pts: i64,
    /// Display index of the picture, counted from the first shown.
    pub frame: i64,
    /// Whether the picture is an IDR: decoding can start here.
    pub idr: bool,
}

/// A source whose packets can be copied: a window of its video packets
/// around the pictures a clip wants, from a keyframe before them to the
/// IDR after (or the end of the file).
#[derive(Debug, Clone)]
pub struct SourceStream {
    /// The file.
    pub path: PathBuf,
    /// Its video stream.
    pub stream_index: usize,
    /// The stream's time base.
    pub time_base: Rational,
    /// Decode position of the first packet of the window in the file,
    /// which is also its display index: the window starts clean.
    pub base: usize,
    /// The window's packets, in decode order.
    pub packets: Vec<IndexedPacket>,
    /// Bytes of each NAL length prefix in the packets.
    pub length_size: usize,
    /// How many pictures decoding runs ahead of display, at most.
    pub reorder: u32,
}

impl SourceStream {
    /// The packet at decode position `k` of the file.
    pub fn packet(&self, k: usize) -> &IndexedPacket {
        &self.packets[k - self.base]
    }
}

/// Audio copied as coded alongside a stitched video track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioCopy {
    /// A file whose audio stream gives the output stream its parameters.
    pub template: PathBuf,
    /// The pieces, in output order.
    pub segments: Vec<AudioSegment>,
}

/// One stretch of one file's audio, placed at `offset` in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSegment {
    /// The file.
    pub path: PathBuf,
    /// Start in the source, in seconds.
    pub from: Ratio,
    /// End in the source, in seconds.
    pub to: Ratio,
    /// Where the stretch starts in the output, in seconds.
    pub offset: Ratio,
}

/// One piece of the output, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Packets `packets` of `sources[source]`, copied; picture `frame` of
    /// the source shows at output frame `frame + offset`.
    Copy {
        /// Index into the plan's sources.
        source: usize,
        /// Packets of the source, in decode order.
        packets: Range<usize>,
        /// Output frame minus source picture index.
        offset: i64,
        /// Output frames the stretch covers.
        frames: Range<u64>,
    },
    /// Output frames rendered and encoded.
    Encode {
        /// Output frames of the run.
        frames: Range<u64>,
    },
}

/// A decision that the composition can be written by stitching.
#[derive(Debug, Clone)]
pub struct SmartPlan {
    /// The files whose packets are copied.
    pub sources: Vec<SourceStream>,
    /// The output, in order.
    pub segments: Vec<Segment>,
    /// The sources' `avcC` record (all copied sources share it).
    pub extradata: Vec<u8>,
    /// Parameter set id for the encoded runs, unused by the sources.
    pub sps_id: u8,
    /// Decode-ahead of the copied sources, in pictures.
    pub reorder: u32,
    /// Frames taken from the sources as coded.
    pub copied_frames: u64,
    /// Frames rendered and encoded.
    pub encoded_frames: u64,
    /// The clips' own audio, copied as coded, when every clip has one of
    /// the same kind that the container takes; `None` means mix and
    /// encode the audio as usual.
    pub audio: Option<AudioCopy>,
}

impl SmartPlan {
    /// Why this path was taken, for the render report.
    pub fn reason(&self) -> String {
        let cuts = self
            .segments
            .iter()
            .filter(|s| matches!(s, Segment::Encode { .. }))
            .count();
        let audio = if self.audio.is_some() {
            "; audio copied as coded, each cut within half a packet"
        } else {
            ""
        };
        format!(
            "smart cut: {} of {} frames copied from the source, {} encoded in {} run{} around the cuts and overlays{audio}",
            self.copied_frames,
            self.copied_frames + self.encoded_frames,
            self.encoded_frames,
            cuts,
            if cuts == 1 { "" } else { "s" }
        )
    }
}

/// A copied packet on its way to the output.
#[derive(Debug, Clone)]
pub struct CopiedPacket {
    /// Length-prefixed NAL units, as stored.
    pub data: Vec<u8>,
    /// Output frame the picture shows at.
    pub frame: i64,
    /// Whether the picture is a keyframe.
    pub keyframe: bool,
}

/// Decides whether `comp` can be written by stitching copied packets and
/// encoded runs, and returns the plan when it can and when at least one
/// stretch would be copied.
///
/// This applies to one layer of H.264 4:2:0 video clips shown as they
/// are at the output size and rate, with the output's color tags, in an
/// MP4, MOV or Matroska output with H.264 video; layers above are
/// allowed and force the frames they touch to be encoded. Clips whose
/// coded parameters differ from the first copyable clip's are encoded
/// whole. Needs the system's x264 for the encoded runs.
pub fn plan_smart_cut(
    comp: &Composition,
    root: &Path,
    container: Container,
    requested: Option<VideoCodec>,
    output_tags: ResolvedTags,
) -> Result<Option<SmartPlan>, MediaError> {
    init();
    if x264::library().is_none() {
        return Ok(None);
    }
    if !matches!(container, Container::Mp4 | Container::Mov | Container::Mkv) {
        return Ok(None);
    }
    if requested.is_some_and(|c| c != VideoCodec::H264) {
        return Ok(None);
    }
    let Some(clips) = base_video_clips(comp, root)? else {
        return Ok(None);
    };
    if clips.is_empty() {
        return Ok(None);
    }
    let fps = comp.fps;
    let total = comp.frame_count();
    let frame_at = |t: Ratio| -> u64 { (t * fps).ceil().max(0) as u64 };

    // Frames an overlay touches must be encoded.
    let mut must_encode = vec![false; total as usize];
    for layer in comp.layers.iter().skip(1) {
        for clip in &layer.clips {
            let (a, b) = (frame_at(clip.start), frame_at(clip.end).min(total));
            for n in a..b {
                must_encode[n as usize] = true;
            }
        }
    }

    let mut sources: Vec<SourceStream> = Vec::new();
    let mut sets: Option<(ParameterSets, Vec<u8>)> = None;
    let mut segments: Vec<Segment> = Vec::new();
    let mut copied = 0u64;
    let mut audio: Option<AudioCopy> = None;
    let mut audio_shape: Option<(codec::Id, Vec<u8>)> = None;
    let mut audio_ok = comp.audio.is_empty();
    for clip in &clips {
        let out_start = frame_at(clip.start);
        let out_end = frame_at(clip.end).min(total);
        if out_end <= out_start {
            continue;
        }
        // The clip's own audio can be copied when every clip's is of one
        // kind the container takes.
        if audio_ok && clip.audio {
            match audio_stream(&clip.path)? {
                Some(shape) if container_accepts_audio(container, shape.0) => {
                    if audio_shape.get_or_insert_with(|| shape.clone()) == &shape {
                        let copy = audio.get_or_insert_with(|| AudioCopy {
                            template: clip.path.clone(),
                            segments: Vec::new(),
                        });
                        copy.segments.push(AudioSegment {
                            path: clip.path.clone(),
                            from: clip.in_,
                            to: clip.in_ + (clip.end - clip.start),
                            offset: clip.start,
                        });
                    } else {
                        audio_ok = false;
                    }
                }
                _ => audio_ok = false,
            }
        } else {
            audio_ok = false;
        }
        let as_is = clip.width == comp.width && clip.height == comp.height && clip.place.is_none();
        let overrides = comp
            .assets
            .get(&clip.asset)
            .map(|a| a.color)
            .unwrap_or_default();
        // Output frame n shows source picture n + shift.
        let shift = ((clip.in_ - clip.start) * fps).floor();
        let wanted =
            (out_start as i64 + shift).max(0) as u64..(out_end as i64 + shift).max(0) as u64;
        let source = if as_is && !wanted.is_empty() {
            index_source(
                &clip.path,
                overrides,
                output_tags,
                comp.width,
                comp.height,
                fps,
                wanted,
            )?
        } else {
            None
        };
        let source = match (source, &sets) {
            (Some((stream, s, bytes)), None) => {
                sets = Some((s, bytes));
                Some(stream)
            }
            (Some((stream, s, _)), Some((first, _))) if s == *first => Some(stream),
            _ => None,
        };
        let Some(stream) = source else {
            push_encode(&mut segments, out_start..out_end);
            continue;
        };
        // Decode order is clean before position p when the first p packets
        // of the file are exactly pictures 0..p; those positions are where
        // a copied stretch may start (at an IDR) or end. The window starts
        // at a clean position, so the test runs on it with its base added.
        let base = stream.base;
        let window_end = base + stream.packets.len();
        let mut clean = vec![false; stream.packets.len() + 1];
        let mut max_frame = base as i64 - 1;
        clean[0] = true;
        for (p, packet) in stream.packets.iter().enumerate() {
            max_frame = max_frame.max(packet.frame);
            clean[p + 1] = max_frame == (base + p) as i64;
        }
        let is_clean = |k: usize| k >= base && k <= window_end && clean[k - base];
        let source_index = sources.len();
        sources.push(stream);
        let stream = &sources[source_index];
        let mut n = out_start;
        while n < out_end {
            if must_encode[n as usize] {
                let start = n;
                while n < out_end && must_encode[n as usize] {
                    n += 1;
                }
                push_encode(&mut segments, start..n);
                continue;
            }
            let start = n;
            while n < out_end && !must_encode[n as usize] {
                n += 1;
            }
            // Source pictures [lo, hi) are wanted as they are. A stretch
            // starts at an IDR that is shown at its own decode position
            // (nothing after it in decode order is shown before it).
            let lo = ((start as i64 + shift).max(0) as usize).max(base);
            let hi = ((n as i64 + shift).max(0) as usize).min(window_end);
            let a = (lo..hi).find(|&p| {
                is_clean(p) && stream.packet(p).idr && stream.packet(p).frame == p as i64
            });
            let b = (lo..=hi).rev().find(|&p| is_clean(p));
            match (a, b) {
                (Some(a), Some(b)) if b >= a + MIN_COPY_FRAMES => {
                    let out_a = a as i64 - shift;
                    let out_b = b as i64 - shift;
                    push_encode(&mut segments, start..out_a as u64);
                    segments.push(Segment::Copy {
                        source: source_index,
                        packets: a..b,
                        offset: -shift,
                        frames: out_a as u64..out_b as u64,
                    });
                    copied += (b - a) as u64;
                    push_encode(&mut segments, out_b as u64..n);
                }
                _ => push_encode(&mut segments, start..n),
            }
        }
    }
    let Some((sets, extradata)) = sets else {
        return Ok(None);
    };
    if copied * MIN_COPY_SHARE < total {
        return Ok(None);
    }
    let Some(sps_id) = h264::free_id(&sets) else {
        return Ok(None);
    };
    let reorder = sources.iter().map(|s| s.reorder).max().unwrap_or(0);
    Ok(Some(SmartPlan {
        sources,
        segments,
        extradata,
        sps_id,
        reorder,
        copied_frames: copied,
        encoded_frames: total - copied,
        audio: if audio_ok { audio } else { None },
    }))
}

/// The codec and parameters of a file's audio stream, if any.
fn audio_stream(path: &Path) -> Result<Option<(codec::Id, Vec<u8>)>, MediaError> {
    let ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    Ok(ictx.streams().best(Type::Audio).map(|s| {
        let params = s.parameters();
        (params.id(), ffi::extradata(&params))
    }))
}

/// Appends an encoded run, merging it into a preceding one.
fn push_encode(segments: &mut Vec<Segment>, frames: Range<u64>) {
    if frames.is_empty() {
        return;
    }
    if let Some(Segment::Encode { frames: last }) = segments.last_mut() {
        if last.end == frames.start {
            last.end = frames.end;
            return;
        }
    }
    segments.push(Segment::Encode { frames });
}

/// Indexes the video packets of `path` around pictures `wanted` when its
/// stream can be copied into an H.264 output of `width`×`height` at
/// `fps` with `tags`: from the keyframe at or before the first wanted
/// picture to the IDR at or after the last (or the end of the file).
fn index_source(
    path: &Path,
    overrides: geneva_color::ColorTags,
    tags: ResolvedTags,
    width: u32,
    height: u32,
    fps: Ratio,
    wanted: Range<u64>,
) -> Result<Option<(SourceStream, ParameterSets, Vec<u8>)>, MediaError> {
    let mut ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    let (stream_index, time_base, params, stream_fps) = {
        let Some(video) = ictx.streams().best(Type::Video) else {
            return Ok(None);
        };
        (
            video.index(),
            video.time_base(),
            video.parameters(),
            super::probe::frame_rate(ratio(video.avg_frame_rate()), ratio(video.rate())),
        )
    };
    if params.id() != codec::Id::H264 || stream_fps != Some(fps) {
        return Ok(None);
    }
    let decoder = codec::context::Context::from_parameters(params.clone())
        .and_then(|c| c.decoder().video())
        .map_err(|e| codec_error(format!("{}: reading stream parameters", path.display()), e))?;
    if decoder.width() != width
        || decoder.height() != height
        || decoder.format() != ffmpeg_next::util::format::Pixel::YUV420P
    {
        return Ok(None);
    }
    let extradata = super::ffi::extradata(&params);
    let Some(sets) = h264::parse_avcc(&extradata) else {
        return Ok(None);
    };
    if sets.length_size != 4 || sets.sps.is_empty() || sets.pps.is_empty() {
        return Ok(None);
    }
    // The copied pictures keep their color encoding, so it must be the
    // output's.
    if VideoReader::open(path, overrides)?.tags() != tags {
        return Ok(None);
    }
    // Picture 0 is the first packet of the file: the IDR the stream opens
    // with, shown first.
    let mut packet = Packet::empty();
    let mut first_pts = None;
    while packet.read(&mut ictx).is_ok() {
        if packet.stream() == stream_index {
            first_pts = packet.pts().or(packet.dts());
            break;
        }
    }
    let Some(first_pts) = first_pts else {
        return Ok(None);
    };
    let frame_of = |pts: i64| (ts_to_secs(pts - first_pts, time_base) * fps).round();
    // Back to the keyframe at or before the first wanted picture.
    let t = Ratio::from_int(wanted.start as i64) / fps + ts_to_secs(first_pts, time_base);
    seek_before(&mut ictx, stream_index, t, time_base)
        .map_err(|e| codec_error(format!("{}: seeking", path.display()), e))?;
    let mut packets: Vec<IndexedPacket> = Vec::new();
    let mut base: Option<usize> = None;
    let mut reorder = 0i64;
    while packet.read(&mut ictx).is_ok() {
        if packet.stream() != stream_index {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            return Ok(None);
        };
        let frame = frame_of(pts);
        let idr = packet.is_key()
            && packet
                .data()
                .is_some_and(|d| h264::is_idr(d, sets.length_size));
        let Some(b) = base else {
            // The window opens at a keyframe.
            if !packet.is_key() || frame < 0 {
                continue;
            }
            base = Some(frame as usize);
            packets.push(IndexedPacket { pts, frame, idr });
            continue;
        };
        // Stop at the IDR after the last wanted picture: every picture
        // before it in display order has been read.
        if idr && frame as u64 >= wanted.end {
            break;
        }
        let k = b + packets.len();
        reorder = reorder.max(k as i64 - frame);
        packets.push(IndexedPacket { pts, frame, idr });
    }
    let Some(base) = base else {
        return Ok(None);
    };
    // The window must hold exactly the pictures base..base + len, once
    // each (constant rate, nothing dropped or repeated, and no picture
    // from before the opening keyframe decoded after it).
    let mut seen = vec![false; packets.len()];
    for p in &packets {
        let i = p.frame - base as i64;
        if i < 0 || i as usize >= seen.len() || seen[i as usize] {
            return Ok(None);
        }
        seen[i as usize] = true;
    }
    Ok(Some((
        SourceStream {
            path: path.to_owned(),
            stream_index,
            time_base,
            base,
            packets,
            length_size: sets.length_size,
            reorder: reorder as u32,
        },
        sets,
        extradata,
    )))
}

/// Reads packets `range` of `source` and hands each to `sink` with its
/// output frame (`frame + offset`); stops early when `sink` says so.
pub fn read_copied(
    source: &SourceStream,
    range: Range<usize>,
    offset: i64,
    sink: &mut dyn FnMut(CopiedPacket) -> bool,
) -> Result<(), MediaError> {
    if range.is_empty() {
        return Ok(());
    }
    let path = &source.path;
    let mut ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    let first = source.packet(range.start);
    // Seek to the stretch's IDR (or an earlier keyframe) and skip up to it.
    seek_before(
        &mut ictx,
        source.stream_index,
        ts_to_secs(first.pts, source.time_base),
        source.time_base,
    )
    .map_err(|e| codec_error(format!("{}: seeking", path.display()), e))?;
    let mut packet = Packet::empty();
    let mut index = None;
    while packet.read(&mut ictx).is_ok() {
        if packet.stream() != source.stream_index {
            continue;
        }
        let pts = packet.pts().or(packet.dts()).unwrap_or(i64::MIN);
        let k = match index {
            None => {
                if pts != first.pts {
                    continue;
                }
                range.start
            }
            Some(k) => k + 1,
        };
        index = Some(k);
        if k >= range.end {
            break;
        }
        let expected = source.packet(k);
        if pts != expected.pts {
            return Err(MediaError::Codec {
                context: "smart cut".to_owned(),
                reason: format!(
                    "{}: packet {k} has timestamp {pts}, expected {}",
                    path.display(),
                    expected.pts
                ),
            });
        }
        let copied = CopiedPacket {
            data: packet.data().unwrap_or_default().to_vec(),
            frame: expected.frame + offset,
            keyframe: packet.is_key(),
        };
        if !sink(copied) {
            return Ok(());
        }
    }
    match index {
        Some(k) if k + 1 >= range.end => Ok(()),
        _ => Err(MediaError::Codec {
            context: "smart cut".to_owned(),
            reason: format!(
                "{}: the file ended before packet {}",
                path.display(),
                range.end
            ),
        }),
    }
}

/// The output audio's packet grid, shared by the segments of one output:
/// every slot holds exactly one copied packet, so the track is gapless
/// and never overlaps itself, and a decoder that plays packets back to
/// back stays in time with the video whatever the joins.
#[derive(Debug, Clone, Default)]
pub struct AudioGrid {
    /// Timestamp of the next free slot, in the source stream's time
    /// base; `None` before the first packet fixes the grid.
    next: Option<i64>,
    /// Slot length: the packets' duration.
    step: i64,
}

/// Reads the audio packets of `segment` into the next slots of `grid`
/// and hands each to `sink`, timed for the output in the source stream's
/// time base; stops early when `sink` says so.
///
/// The first packet of the output is the one that straddles the start,
/// placed early by the part before the cut (a negative start, which the
/// container turns into an edit, so the sound starts on the sample).
/// Every later segment starts with the packet nearest to its cut, so a
/// join is off by at most half a packet and the error never adds up.
pub fn read_copied_audio(
    segment: &AudioSegment,
    grid: &mut AudioGrid,
    sink: &mut dyn FnMut(Packet) -> bool,
) -> Result<(), MediaError> {
    let path = &segment.path;
    let mut ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    let (index, tb) = {
        let Some(s) = ictx.streams().best(Type::Audio) else {
            return Err(MediaError::NoStream {
                path: path.clone(),
                kind: "audio",
            });
        };
        (s.index(), s.time_base())
    };
    // Time zero of the file is its first video frame, as everywhere: the
    // audio keeps its offset from the picture.
    let zero = super::copy::file_zero(&ictx);
    let to_ts = |secs: Ratio| {
        (secs * Ratio::new(i64::from(tb.denominator()), i64::from(tb.numerator()))).round()
    };
    let start = to_ts(zero);
    let from_ts = to_ts(segment.from);
    let offset_ts = to_ts(segment.offset);
    let end_ts = offset_ts + to_ts(segment.to - segment.from);
    if segment.from > Ratio::ZERO {
        seek_before(&mut ictx, index, segment.from + zero, tb)
            .map_err(|e| codec_error(format!("{}: seeking", path.display()), e))?;
    }
    let mut packet = Packet::empty();
    let mut placing = false;
    while packet.read(&mut ictx).is_ok() {
        if packet.stream() != index {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            continue;
        };
        let pts = pts - start;
        let duration = packet.duration().max(1);
        if !placing {
            match grid.next {
                // The first packet fixes the grid: the one that straddles
                // the cut, or the first at or after it.
                None => {
                    if pts + duration <= from_ts {
                        continue;
                    }
                    grid.step = duration;
                    grid.next = Some(pts - from_ts + offset_ts);
                }
                // A later segment: the packet nearest to the slot's time.
                Some(next) => {
                    let wanted = from_ts + (next - offset_ts);
                    if pts + grid.step / 2 < wanted {
                        continue;
                    }
                }
            }
            placing = true;
        }
        let slot = grid.next.expect("set above");
        if slot >= end_ts {
            break;
        }
        let mut out = packet.clone();
        out.set_pts(Some(slot));
        out.set_dts(Some(slot));
        out.set_duration(grid.step);
        out.set_position(-1);
        grid.next = Some(slot + grid.step);
        if !sink(out) {
            return Ok(());
        }
    }
    Ok(())
}
