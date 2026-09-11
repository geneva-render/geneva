//! Media files in and out of the Geneva engine.
//!
//! This crate is the only place that touches container formats and codecs.
//! Everything above it works with [`geneva_render::Frame`]s, linear-light
//! images and `f32` audio samples. The bundled media libraries do the
//! demuxing, decoding and encoding behind the `media` feature; the color
//! conversions between coded pixels and the compositing format are
//! implemented here in Rust so that the same color pipeline applies to
//! every source.

#![deny(unsafe_code)]

#[cfg(feature = "media")]
mod assets;
#[cfg(feature = "media")]
mod codecs;
pub mod convert;
mod info;
#[cfg(feature = "media")]
pub mod mix;
pub mod subtitles;

use std::path::PathBuf;

use thiserror::Error;

#[cfg(feature = "media")]
pub use assets::MediaAssets;
#[cfg(feature = "media")]
pub use codecs::{
    AudioCopy, AudioEncoder, AudioGrid, AudioReader, AudioSegment, AudioSettings, CopiedPacket,
    CopyPlan, CopyReport, CopySegment, DirectSource, EncodeSettings, Encoder, Packet, Segment,
    SmartPlan, SourceStream, StitchSettings, SubtitleSettings, SubtitleStreamInfo, VideoReader,
    VideoSettings, audio_sample_rate_for, container_accepts_audio, container_accepts_video,
    container_for, default_codecs, default_image_codec, output_tags_for, plan_smart_cut,
    plan_stream_copy, plane_format_for, probe, read_copied, read_copied_audio, read_subtitles,
    stream_copy, subtitle_streams, system_x264, system_x264_error,
};
pub use info::{AudioInfo, MediaInfo, SubtitleInfo, VideoInfo};

/// Errors from reading or writing media.
#[derive(Debug, Error)]
pub enum MediaError {
    /// The file could not be opened or parsed as a container.
    #[error("{path}: {reason}")]
    Open {
        /// File involved.
        path: PathBuf,
        /// Library message.
        reason: String,
    },
    /// The file has no stream of the requested kind.
    #[error("{path}: no {kind} stream")]
    NoStream {
        /// File involved.
        path: PathBuf,
        /// `video` or `audio`.
        kind: &'static str,
    },
    /// A decoder or encoder reported an error.
    #[error("{context}: {reason}")]
    Codec {
        /// What was being done.
        context: String,
        /// Library message.
        reason: String,
    },
    /// A requested codec is not available in the linked libraries.
    #[error("encoder {name:?} is not available in this build")]
    MissingEncoder {
        /// Library name of the encoder.
        name: String,
    },
    /// Media support was compiled out.
    #[error("this build has no media support; rebuild with the `media` feature")]
    Unavailable,
}
