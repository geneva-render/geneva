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

use std::path::PathBuf;

use thiserror::Error;

#[cfg(feature = "media")]
pub use assets::MediaAssets;
#[cfg(feature = "media")]
pub use codecs::{
    AudioReader, AudioSettings, CopyPlan, CopyReport, CopySegment, EncodeSettings, Encoder,
    VideoReader, container_for, default_codecs, plan_stream_copy, probe, stream_copy,
};
pub use info::{AudioInfo, MediaInfo, VideoInfo};

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
