//! Media files in and out of the Geneva engine.
//!
//! This crate is the only place that touches container formats and codecs.
//! Everything above it works with [`geneva_render::Frame`]s, linear-light
//! images and `f32` audio samples. The libav libraries do the demuxing,
//! decoding and encoding behind the `libav` feature; the color conversions
//! between coded pixels and the compositing format are implemented here in
//! Rust so that the same color pipeline applies to every source.

#![forbid(unsafe_code)]

#[cfg(feature = "libav")]
mod assets;
pub mod convert;
mod info;
#[cfg(feature = "libav")]
mod libav;
#[cfg(feature = "libav")]
pub mod mix;

use std::path::PathBuf;

use thiserror::Error;

#[cfg(feature = "libav")]
pub use assets::MediaAssets;
pub use info::{AudioInfo, MediaInfo, VideoInfo};
#[cfg(feature = "libav")]
pub use libav::{
    AudioReader, AudioSettings, EncodeSettings, Encoder, VideoReader, container_for,
    default_codecs, probe,
};

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
    #[error("this build has no media support; rebuild with the `libav` feature")]
    Unavailable,
}
