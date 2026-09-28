//! Subtitle streams in containers: writing cues as text packets and
//! reading them back.

use std::collections::VecDeque;
use std::path::Path;

use ffmpeg_next::codec;
use ffmpeg_next::media::Type;
use ffmpeg_next::{Dictionary, Packet, Rational};
use geneva_timeline::Ratio;
use geneva_timeline::schema::Container;

use super::probe::ts_to_secs;
use super::{codec_error, ffi, init, open_error};
use crate::MediaError;
use crate::subtitles::{Cue, strip_tags};

/// A subtitle track to write into an output.
#[derive(Debug, Clone)]
pub struct SubtitleSettings {
    /// Language code stored on the stream, if any.
    pub language: Option<String>,
    /// Title stored on the stream, if any.
    pub title: Option<String>,
    /// The cues, in time order.
    pub cues: Vec<Cue>,
}

/// The text codec a container stores subtitles as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextCodec {
    /// 3GPP timed text, as MP4 and QuickTime carry it.
    MovText,
    /// SubRip text blocks, as Matroska carries them.
    Subrip,
    /// WebVTT cue payloads, as WebM carries them.
    WebVtt,
}

fn text_codec_for(container: Container) -> Option<TextCodec> {
    match container {
        Container::Mp4 | Container::Mov | Container::M4a => Some(TextCodec::MovText),
        Container::Mkv => Some(TextCodec::Subrip),
        Container::Webm => Some(TextCodec::WebVtt),
        _ => None,
    }
}

struct SubtitleStream {
    index: usize,
    codec: TextCodec,
    pending: VecDeque<Cue>,
}

/// Writes subtitle tracks as text packets, keeping them interleaved with
/// the picture and sound by writing each cue once the output has reached
/// its start time.
pub(super) struct SubtitleWriter {
    streams: Vec<SubtitleStream>,
}

/// Cue timestamps are written in milliseconds.
const CUE_TIME_BASE: Rational = Rational(1, 1000);

impl SubtitleWriter {
    /// Adds one stream per track to `octx`, which must not have written its
    /// header yet.
    pub(super) fn add_streams(
        octx: &mut ffmpeg_next::format::context::Output,
        container: Option<Container>,
        tracks: &[SubtitleSettings],
        path: &Path,
    ) -> Result<Self, MediaError> {
        let mut streams = Vec::new();
        if tracks.is_empty() {
            return Ok(Self { streams });
        }
        let codec = container
            .and_then(text_codec_for)
            .ok_or_else(|| MediaError::Codec {
                context: "encoder setup".to_owned(),
                reason: format!(
                    "the {} container cannot hold subtitle streams; use mp4, mov, mkv or webm",
                    container.map_or("chosen".to_owned(), |c| format!("{c:?}"))
                ),
            })?;
        let id = match codec {
            TextCodec::MovText => codec::Id::MOV_TEXT,
            TextCodec::Subrip => codec::Id::SUBRIP,
            TextCodec::WebVtt => codec::Id::WEBVTT,
        };
        for track in tracks {
            let ctx = ffi::subtitle_context(id);
            let mut stream = octx
                .add_stream_with(&ctx)
                .map_err(|e| open_error(path, e))?;
            stream.set_time_base(CUE_TIME_BASE);
            let mut metadata = Dictionary::new();
            if let Some(language) = &track.language {
                metadata.set("language", &crate::subtitles::iso639_2(language));
            }
            if let Some(title) = &track.title {
                metadata.set("title", title);
                // MP4 and MOV keep no stream title; players show the
                // handler name in its place.
                if codec == TextCodec::MovText {
                    metadata.set("handler_name", title);
                }
            }
            stream.set_metadata(metadata);
            let mut cues: Vec<Cue> = track.cues.clone();
            cues.sort_by_key(|a| a.start);
            streams.push(SubtitleStream {
                index: stream.index(),
                codec,
                pending: cues.into(),
            });
        }
        Ok(Self { streams })
    }

    /// Writes every cue that starts at or before `time`.
    pub(super) fn write_due(
        &mut self,
        octx: &mut ffmpeg_next::format::context::Output,
        time: Ratio,
    ) -> Result<(), MediaError> {
        for stream in &mut self.streams {
            while stream.pending.front().is_some_and(|c| c.start <= time) {
                let cue = stream.pending.pop_front().expect("checked");
                write_cue(octx, stream.index, stream.codec, &cue)?;
            }
        }
        Ok(())
    }

    /// Writes whatever is left.
    pub(super) fn finish(
        &mut self,
        octx: &mut ffmpeg_next::format::context::Output,
    ) -> Result<(), MediaError> {
        for stream in &mut self.streams {
            while let Some(cue) = stream.pending.pop_front() {
                write_cue(octx, stream.index, stream.codec, &cue)?;
            }
        }
        Ok(())
    }
}

fn write_cue(
    octx: &mut ffmpeg_next::format::context::Output,
    index: usize,
    codec: TextCodec,
    cue: &Cue,
) -> Result<(), MediaError> {
    let text = match codec {
        TextCodec::MovText => strip_tags(&cue.text),
        TextCodec::Subrip | TextCodec::WebVtt => cue.text.clone(),
    };
    if text.trim().is_empty() {
        return Ok(());
    }
    let data = match codec {
        // 3GPP timed text: a big-endian length prefix, then the text.
        TextCodec::MovText => {
            let bytes = text.as_bytes();
            let len = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
            let mut d = len.to_be_bytes().to_vec();
            d.extend_from_slice(&bytes[..usize::from(len)]);
            d
        }
        TextCodec::Subrip | TextCodec::WebVtt => text.into_bytes(),
    };
    let start_ms = (cue.start.to_f64() * 1000.0).round() as i64;
    let end_ms = (cue.end.to_f64() * 1000.0).round() as i64;
    let stream_tb = octx.stream(index).expect("stream added").time_base();
    let mut packet = Packet::copy(&data);
    packet.set_stream(index);
    packet.set_pts(Some(start_ms));
    packet.set_dts(Some(start_ms));
    packet.set_duration((end_ms - start_ms).max(1));
    packet.set_flags(codec::packet::Flags::KEY);
    packet.rescale_ts(CUE_TIME_BASE, stream_tb);
    packet
        .write_interleaved(octx)
        .map_err(|e| codec_error("writing subtitle cue", e))
}

/// A subtitle stream found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleStreamInfo {
    /// Index of the stream in the container.
    pub index: usize,
    /// Codec name.
    pub codec: String,
    /// Language tag, if the file carries one.
    pub language: Option<String>,
}

/// Lists the subtitle streams of a file.
pub fn subtitle_streams(path: &Path) -> Result<Vec<SubtitleStreamInfo>, MediaError> {
    init();
    let ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    Ok(ictx
        .streams()
        .filter(|s| s.parameters().medium() == Type::Subtitle)
        .map(|s| SubtitleStreamInfo {
            index: s.index(),
            codec: format!("{:?}", s.parameters().id()).to_ascii_lowercase(),
            language: s.metadata().get("language").map(str::to_owned),
        })
        .collect())
}

/// Reads the cues of one subtitle stream: the `nth` subtitle stream of the
/// file (0 for the first). Only text codecs can be read; bitmap subtitles
/// are refused.
pub fn read_subtitles(path: &Path, nth: usize) -> Result<Vec<Cue>, MediaError> {
    init();
    let mut ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
    let (index, id, tb) = ictx
        .streams()
        .filter(|s| s.parameters().medium() == Type::Subtitle)
        .nth(nth)
        .map(|s| (s.index(), s.parameters().id(), s.time_base()))
        .ok_or_else(|| MediaError::NoStream {
            path: path.to_owned(),
            kind: "subtitle",
        })?;
    let strip = match id {
        codec::Id::MOV_TEXT => 2,
        codec::Id::SUBRIP | codec::Id::TEXT | codec::Id::WEBVTT => 0,
        other => {
            return Err(MediaError::Codec {
                context: format!("{}: reading subtitles", path.display()),
                reason: format!("{other:?} subtitles are not text and cannot be extracted"),
            });
        }
    };
    let mut cues: Vec<Cue> = Vec::new();
    let mut packet = Packet::empty();
    while packet.read(&mut ictx).is_ok() {
        if packet.stream() != index {
            continue;
        }
        let Some(pts) = packet.pts().or(packet.dts()) else {
            continue;
        };
        let data = packet.data().unwrap_or(&[]);
        if data.len() <= strip {
            continue;
        }
        let text = String::from_utf8_lossy(&data[strip..]).trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let start = ts_to_secs(pts, tb);
        let end = if packet.duration() > 0 {
            start + ts_to_secs(packet.duration(), tb)
        } else {
            start + Ratio::from_int(3)
        };
        cues.push(Cue::plain(start, end, text));
    }
    Ok(cues)
}
