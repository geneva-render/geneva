//! Media I/O over the bundled media libraries.

mod decode;
mod encode;
mod probe;
mod tags;

use std::path::Path;

use crate::MediaError;

pub use decode::{AudioReader, VideoReader};
pub use encode::{AudioSettings, EncodeSettings, Encoder, container_for, default_codecs};
pub use probe::probe;

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
