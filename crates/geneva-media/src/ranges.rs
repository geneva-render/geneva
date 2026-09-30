//! Which bytes of a media file a stretch of it needs, so that a machine
//! rendering one part of a video can fetch those bytes and leave the
//! rest of the file as a hole. The file keeps its size and every byte it
//! has stays at its offset, so the demuxer reads it as it would the
//! whole file, as long as it reads no hole.
//!
//! Only containers with an index at a known place qualify: MP4 and
//! QuickTime, whose sample table gives every sample's offset and size
//! and whose seeks read no sample data. Matroska, WebM and MPEG-TS are
//! read whole, since a seek in them may scan the data itself.

use std::ops::Range;

/// What a packet carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    /// Picture.
    Video,
    /// Sound.
    Audio,
    /// Anything else: subtitles, timecode, data.
    Other,
}

/// Where one packet's bytes are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacketSpan {
    /// The stream's index in the file.
    pub stream: usize,
    /// What the stream carries.
    pub kind: PacketKind,
    /// Presentation time in seconds from the file's start, or the decode
    /// time when the packet has none.
    pub time: f64,
    /// Whether decoding can start here.
    pub key: bool,
    /// Byte offset in the file.
    pub pos: u64,
    /// Length in bytes.
    pub size: u64,
}

/// Every packet of a file, in the order the file stores them, and the
/// file's size.
#[derive(Debug, Clone, Default)]
pub struct PacketMap {
    /// The file's size in bytes.
    pub size: u64,
    /// The packets, in file order.
    pub packets: Vec<PacketSpan>,
}

/// Packets decoded after the last one a stretch shows, for the decoder's
/// own delay (frame threads, reordering) before it hands the last frames
/// out.
const TRAILING_PACKETS: usize = 48;

/// Seconds of each stream from the start that are always fetched, for
/// the look at the first packets a file's opening takes.
const OPENING_SECS: f64 = 1.0;

/// Adjacent ranges closer than this are fetched as one.
const MERGE_GAP: u64 = 64 * 1024;

impl PacketMap {
    /// The bytes that are not any packet's (the container's headers and
    /// index), and each stream's first second. Needed by every part.
    pub fn base(&self) -> Vec<Range<u64>> {
        let mut spans: Vec<Range<u64>> =
            self.packets.iter().map(|p| p.pos..p.pos + p.size).collect();
        spans.sort_by_key(|r| r.start);
        let mut out = Vec::new();
        let mut at = 0;
        for s in &spans {
            if s.start > at {
                out.push(at..s.start);
            }
            at = at.max(s.end);
        }
        if at < self.size {
            out.push(at..self.size);
        }
        let first = self
            .packets
            .iter()
            .map(|p| p.time)
            .fold(f64::INFINITY, f64::min);
        out.extend(
            self.packets
                .iter()
                .filter(|p| p.time < first + OPENING_SECS)
                .map(|p| p.pos..p.pos + p.size),
        );
        merge(out)
    }

    /// The picture packets that decode the frames from `from` to `to`
    /// seconds of the file: each video stream's packets, in the order
    /// they are decoded, from the last keyframe at or before `from` to
    /// the last packet shown at or before `to`, and some more for the
    /// decoder's delay (reordering, frame threads) before it hands the
    /// last frames out.
    pub fn picture(&self, from: f64, to: f64) -> Vec<Range<u64>> {
        let mut out = Vec::new();
        let streams: std::collections::BTreeSet<usize> = self
            .packets
            .iter()
            .filter(|p| p.kind == PacketKind::Video)
            .map(|p| p.stream)
            .collect();
        for stream in streams {
            let packets: Vec<&PacketSpan> =
                self.packets.iter().filter(|p| p.stream == stream).collect();
            let first = packets
                .iter()
                .rposition(|p| p.key && p.time <= from)
                .unwrap_or(0);
            let last = packets
                .iter()
                .rposition(|p| p.time <= to)
                .map_or(0, |i| i + 1)
                .max(first + 1);
            let last = (last + TRAILING_PACKETS).min(packets.len());
            out.extend(packets[first..last].iter().map(|p| p.pos..p.pos + p.size));
        }
        merge(out)
    }

    /// Every sound packet.
    pub fn sound(&self) -> Vec<Range<u64>> {
        merge(
            self.packets
                .iter()
                .filter(|p| p.kind == PacketKind::Audio)
                .map(|p| p.pos..p.pos + p.size)
                .collect(),
        )
    }
}

/// Sorts `ranges` and joins those that overlap or nearly touch, so that
/// a few bytes between two wanted stretches are fetched with them
/// rather than costing a request of their own.
pub fn merge(ranges: Vec<Range<u64>>) -> Vec<Range<u64>> {
    join(ranges, MERGE_GAP)
}

/// Sorts `ranges` and joins those that overlap or touch: exactly the
/// bytes they cover, for keeping track of what a file already holds.
pub fn union(ranges: Vec<Range<u64>>) -> Vec<Range<u64>> {
    join(ranges, 0)
}

fn join(mut ranges: Vec<Range<u64>>, gap: u64) -> Vec<Range<u64>> {
    ranges.retain(|r| r.end > r.start);
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<Range<u64>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end + gap => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// The parts of `want` not already in `have`, both merged.
pub fn missing(want: &[Range<u64>], have: &[Range<u64>]) -> Vec<Range<u64>> {
    let mut out = Vec::new();
    for w in want {
        let mut at = w.start;
        for h in have {
            if h.end <= at || h.start >= w.end {
                continue;
            }
            if h.start > at {
                out.push(at..h.start);
            }
            at = at.max(h.end);
        }
        if at < w.end {
            out.push(at..w.end);
        }
    }
    out
}

/// Total bytes in `ranges`.
pub fn total(ranges: &[Range<u64>]) -> u64 {
    ranges.iter().map(|r| r.end - r.start).sum()
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn packet(kind: PacketKind, time: f64, key: bool, pos: u64) -> PacketSpan {
        PacketSpan {
            stream: usize::from(kind == PacketKind::Audio),
            kind,
            time,
            key,
            pos,
            size: 100_000,
        }
    }

    /// Ten seconds of picture at one packet a second, a keyframe every
    /// three, interleaved with sound, after a 1000-byte header and before
    /// a 5000-byte index.
    fn map() -> PacketMap {
        let mut packets = Vec::new();
        let mut pos = 1000;
        for s in 0..10 {
            packets.push(packet(PacketKind::Video, f64::from(s), s % 3 == 0, pos));
            pos += 100_000;
            packets.push(packet(PacketKind::Audio, f64::from(s), true, pos));
            pos += 100_000;
        }
        PacketMap {
            size: pos + 5000,
            packets,
        }
    }

    #[test]
    fn the_base_is_the_headers_the_index_and_the_opening() {
        let m = map();
        let base = m.base();
        assert_eq!(base.first().unwrap().start, 0);
        assert_eq!(base.last().unwrap().end, m.size);
        // Header, the first second of both streams, and the index.
        assert_eq!(base, vec![0..1000 + 200_000, m.size - 5000..m.size]);
    }

    #[test]
    fn a_picture_stretch_starts_at_the_keyframe_before_it() {
        let m = map();
        let r = m.picture(4.5, 5.5);
        // Keyframes at 0, 3, 6, 9: from 3, and the trailing packets run
        // to the end of this short stream.
        let start = m
            .packets
            .iter()
            .find(|p| p.time == 3.0 && p.kind == PacketKind::Video)
            .unwrap()
            .pos;
        assert_eq!(r.first().unwrap().start, start);
        // Sound packets between the picture ones are left out.
        assert!(total(&r) < m.size / 2);
    }

    #[test]
    fn sound_is_every_sound_packet_and_nothing_else() {
        let m = map();
        assert_eq!(total(&m.sound()), 10 * 100_000);
    }

    #[test]
    fn missing_is_what_is_not_already_there() {
        assert_eq!(
            missing(&[0..100], &[10..20, 50..60]),
            vec![0..10, 20..50, 60..100]
        );
        assert_eq!(missing(&[0..100], &[0..100]), Vec::<Range<u64>>::new());
        assert_eq!(merge(vec![0..10, 5..20, 30..40]), vec![0..40]);
        assert_eq!(union(vec![0..10, 5..20, 30..40]), vec![0..20, 30..40]);
    }
}
