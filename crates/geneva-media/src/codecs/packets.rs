//! Reads where every packet of a file is, for [`crate::ranges`].

use std::path::Path;

use super::{init, open_error};
use crate::MediaError;
use crate::ranges::{PacketKind, PacketMap, PacketSpan};

/// Where every packet of `path` is, when the file is MP4 or QuickTime;
/// `None` for any other container, which a worker must fetch whole. Reads
/// the whole file once.
pub fn packet_map(path: &Path) -> Result<Option<PacketMap>, MediaError> {
    init();
    let mut ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    if !ictx
        .format()
        .name()
        .split(',')
        .any(|n| n == "mov" || n == "mp4")
    {
        return Ok(None);
    }
    let streams: Vec<(PacketKind, f64)> = ictx
        .streams()
        .map(|s| {
            let kind = match s.parameters().medium() {
                ffmpeg_next::media::Type::Video => PacketKind::Video,
                ffmpeg_next::media::Type::Audio => PacketKind::Audio,
                _ => PacketKind::Other,
            };
            (kind, f64::from(s.time_base()))
        })
        .collect();
    let size = std::fs::metadata(path)
        .map_err(|e| MediaError::Open {
            path: path.to_owned(),
            reason: e.to_string(),
        })?
        .len();
    let mut packets = Vec::new();
    let mut packet = ffmpeg_next::Packet::empty();
    while super::read_packet(&mut packet, &mut ictx).is_ok() {
        let Ok(pos) = u64::try_from(packet.position()) else {
            // A packet the demuxer cannot place: the file cannot be split.
            return Ok(None);
        };
        let (kind, tb) = streams[packet.stream()];
        let ts = packet.pts().or(packet.dts()).unwrap_or(0);
        packets.push(PacketSpan {
            stream: packet.stream(),
            kind,
            time: ts as f64 * tb,
            key: packet.is_key(),
            pos,
            size: packet.size() as u64,
        });
    }
    Ok(Some(PacketMap { size, packets }))
}
