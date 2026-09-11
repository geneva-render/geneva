//! Media I/O over the bundled media libraries.

mod copy;
mod decode;
mod direct;
mod encode;
mod ffi;
mod probe;
mod subtitle_streams;
mod tags;

use std::path::Path;

use crate::MediaError;

pub use copy::{CopyPlan, CopyReport, CopySegment, plan_stream_copy, stream_copy};
pub use decode::{AudioReader, VideoReader};
pub use direct::DirectSource;
pub use encode::{
    AudioEncoder, AudioSettings, EncodeSettings, Encoder, VideoSettings, container_accepts_audio,
    container_accepts_video, container_for, default_codecs, default_image_codec, output_tags_for,
    plane_format_for,
};
/// An encoded packet on its way to the muxer.
pub use ffmpeg_next::Packet;
pub use probe::probe;
pub use subtitle_streams::{
    SubtitleSettings, SubtitleStreamInfo, read_subtitles, subtitle_streams,
};

/// Initializes the libraries once. Safe to call repeatedly.
fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Failure here means the libraries are unusable; every later call
        // would fail with a clearer message, so the result is ignored.
        let _ = ffmpeg_next::init();
        ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);
    });
}

fn open_error(path: &Path, e: ffmpeg_next::Error) -> MediaError {
    MediaError::Open {
        path: path.to_owned(),
        reason: e.to_string(),
    }
}

fn codec_error(context: impl Into<String>, e: ffmpeg_next::Error) -> MediaError {
    MediaError::Codec {
        context: context.into(),
        reason: e.to_string(),
    }
}
