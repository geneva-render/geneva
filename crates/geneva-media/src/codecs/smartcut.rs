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

use super::copy::base_video_clips;
use super::decode::VideoReader;
use super::h264::{self, ParameterSets};
use super::probe::{ratio, ts_to_secs};
use super::{codec_error, init, open_error, x264};
use crate::MediaError;

/// Shortest stretch worth copying, in frames: below this the parameter
/// set switches cost more than the encode they save.
const MIN_COPY_FRAMES: usize = 2;

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

/// A source whose packets can be copied.
#[derive(Debug, Clone)]
pub struct SourceStream {
    /// The file.
    pub path: PathBuf,
    /// Its video stream.
    pub stream_index: usize,
    /// The stream's time base.
    pub time_base: Rational,
    /// Every video packet of the file, in decode order.
    pub packets: Vec<IndexedPacket>,
    /// Bytes of each NAL length prefix in the packets.
    pub length_size: usize,
    /// How many pictures decoding runs ahead of display, at most.
    pub reorder: u32,
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
}

impl SmartPlan {
    /// Why this path was taken, for the render report.
    pub fn reason(&self) -> String {
        let cuts = self
            .segments
            .iter()
            .filter(|s| matches!(s, Segment::Encode { .. }))
            .count();
        format!(
            "smart cut: {} of {} frames copied from the source, {} encoded in {} run{} around the cuts and overlays",
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
    for clip in &clips {
        let out_start = frame_at(clip.start);
        let out_end = frame_at(clip.end).min(total);
        if out_end <= out_start {
            continue;
        }
        let as_is = clip.width == comp.width && clip.height == comp.height && clip.place.is_none();
        let overrides = comp
            .assets
            .get(&clip.asset)
            .map(|a| a.color)
            .unwrap_or_default();
        let source = if as_is {
            index_source(
                &clip.path,
                overrides,
                output_tags,
                comp.width,
                comp.height,
                fps,
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
        // Output frame n shows source picture n + offset.
        let shift = ((clip.in_ - clip.start) * fps).floor();
        let n_packets = stream.packets.len();
        // Decode order is clean before packet p when the first p packets
        // are exactly pictures 0..p; those positions are where a copied
        // stretch may start (at an IDR) or end.
        let mut clean = vec![false; n_packets + 1];
        let mut max_frame = -1i64;
        clean[0] = true;
        for (p, packet) in stream.packets.iter().enumerate() {
            max_frame = max_frame.max(packet.frame);
            clean[p + 1] = max_frame == p as i64;
        }
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
            // Source pictures [lo, hi) are wanted as they are.
            let lo = (start as i64 + shift).max(0) as usize;
            let hi = ((n as i64 + shift).max(0) as usize).min(n_packets);
            let a = (lo..hi).find(|&p| clean[p] && stream.packets[p].idr);
            let b = (lo..=hi).rev().find(|&p| clean[p]);
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
    if copied == 0 {
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

/// Indexes the video packets of `path` when its stream can be copied
/// into an H.264 output of `width`×`height` at `fps` with `tags`.
fn index_source(
    path: &Path,
    overrides: geneva_color::ColorTags,
    tags: ResolvedTags,
    width: u32,
    height: u32,
    fps: Ratio,
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
            ratio(video.avg_frame_rate()).or_else(|| ratio(video.rate())),
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
    let mut packets = Vec::new();
    let mut packet = Packet::empty();
    let mut min_pts = i64::MAX;
    while packet.read(&mut ictx).is_ok() {
        if packet.stream() != stream_index {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            return Ok(None);
        };
        min_pts = min_pts.min(pts);
        let idr = packet.is_key()
            && packet
                .data()
                .is_some_and(|d| h264::is_idr(d, sets.length_size));
        packets.push(IndexedPacket { pts, frame: 0, idr });
    }
    if packets.is_empty() {
        return Ok(None);
    }
    // Display indices from timestamps; the pictures must be exactly one
    // per frame slot (constant rate, nothing dropped or repeated).
    let mut seen = vec![false; packets.len()];
    let mut reorder = 0i64;
    for (k, p) in packets.iter_mut().enumerate() {
        let frame = (ts_to_secs(p.pts - min_pts, time_base) * fps).round();
        if frame < 0 || frame as usize >= seen.len() || seen[frame as usize] {
            return Ok(None);
        }
        seen[frame as usize] = true;
        p.frame = frame;
        reorder = reorder.max(k as i64 - frame);
    }
    Ok(Some((
        SourceStream {
            path: path.to_owned(),
            stream_index,
            time_base,
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
    let first = &source.packets[range.start];
    // Seek to the stretch's IDR (or an earlier keyframe) and skip up to it.
    let micros = (ts_to_secs(first.pts, source.time_base).to_f64() * 1_000_000.0) as i64;
    ictx.seek(micros, ..micros)
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
        let expected = &source.packets[k];
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
