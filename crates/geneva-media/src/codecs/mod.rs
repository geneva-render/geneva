//! Media I/O over the bundled media libraries.

mod copy;
mod decode;
mod direct;
mod encode;
mod ffi;
mod h264;
mod probe;
mod scale;
mod smartcut;
mod subtitle_streams;
mod tags;
mod x264;

use std::path::Path;

use crate::MediaError;

pub use copy::{
    CopyPlan, CopyRefusal, CopyReport, CopySegment, plan_stream_copy, plan_stream_copy_explained,
    stream_copy,
};
pub use decode::{AudioReader, AudioStream, VideoReader, set_decoder_threads_for_this_thread};
pub use direct::DirectSource;
pub use encode::{
    AudioEncoder, AudioSettings, EncodeSettings, Encoder, StitchSettings, VideoSettings,
    audio_sample_rate_for, container_accepts_audio, container_accepts_video, container_for,
    default_codecs, default_image_codec, output_tags_for, plane_format_for,
};
pub use ffi::HdrMetadata;
/// An encoded packet on its way to the muxer.
pub use ffmpeg_next::Packet;
pub use probe::{hdr_metadata_of, probe};
pub use scale::PlaneScaler;
pub use smartcut::{
    AudioCopy, AudioGrid, AudioSegment, CopiedPacket, Segment, SmartPlan, SourceStream,
    plan_smart_cut, read_copied, read_copied_audio,
};
pub use subtitle_streams::{
    SubtitleSettings, SubtitleStreamInfo, read_subtitles, subtitle_streams,
};
/// The system's x264 library, when installed and loadable.
pub use x264::library as system_x264;
/// Why a system x264 that was found could not be used.
pub use x264::load_error as system_x264_error;

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
