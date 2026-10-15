//! Media I/O over the bundled media libraries.

mod copy;
mod decode;
mod direct;
mod encode;
mod ffi;
mod h264;
mod packets;
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
    stream_copy, stream_copy_mixing_audio,
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
pub use packets::packet_map;
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

/// Reads the input's next packet into `packet`, releasing what it held
/// first. The demuxer clears the packet it is given without releasing
/// its buffer (libavcodec 61), so a packet reused across reads leaked
/// every one it had held: a reader of one stream leaked all the others,
/// as much as the file for an audio reader of a ProRes file.
#[allow(clippy::disallowed_methods)]
fn read_packet(
    packet: &mut ffmpeg_next::Packet,
    ictx: &mut ffmpeg_next::format::context::Input,
) -> Result<(), ffmpeg_next::Error> {
    *packet = ffmpeg_next::Packet::empty();
    packet.read(ictx)
}

fn open_error(path: &Path, e: ffmpeg_next::Error) -> MediaError {
    MediaError::Open {
        path: path.to_owned(),
        reason: e.to_string(),
    }
}

/// Samples asked for by each call that drains a converter at the end of a
/// stream. A drain gives back the converter's filter tail on top of what
/// it already held, which its upper bound does not count, so it asks for
/// at least this much and repeats until a call gives back nothing.
const RESAMPLE_DRAIN_BLOCK: usize = 4096;

/// Converts `input` with `ctx`, or drains what `ctx` still holds when
/// `input` is `None`, into a new frame.
///
/// The frame is allocated with room for everything the call can give
/// back. Left to the binding, an empty output frame is sized to the input,
/// so when the output rate is higher the converter keeps the excess and
/// the backlog grows for the whole stream. A new frame each call also
/// keeps a short first frame (Opus and Vorbis start with one) from capping
/// the later ones.
fn resample(
    ctx: &mut ffmpeg_next::software::resampling::Context,
    input: Option<&ffmpeg_next::util::frame::Audio>,
) -> Result<ffmpeg_next::util::frame::Audio, ffmpeg_next::Error> {
    use ffmpeg_next::util::frame::Audio;
    let target = *ctx.output();
    let mut room = ffi::resampler_out_samples(ctx, input.map_or(0, Audio::samples))?;
    if input.is_none() {
        room = room.max(RESAMPLE_DRAIN_BLOCK);
    }
    if room == 0 {
        return Ok(Audio::empty());
    }
    let mut out = Audio::new(target.format, room, target.channel_layout);
    out.set_rate(target.rate);
    match input {
        Some(input) => ctx.run(input, &mut out)?,
        None => ctx.flush(&mut out)?,
    };
    // With the output sized to fit, what the converter holds between calls
    // stays within its filter length rather than growing with the stream.
    debug_assert!(
        ctx.delay()
            .is_none_or(|d| d.output < i64::from(target.rate)),
        "the audio converter is holding back more than a second of audio"
    );
    Ok(out)
}

fn codec_error(context: impl Into<String>, e: ffmpeg_next::Error) -> MediaError {
    MediaError::Codec {
        context: context.into(),
        reason: e.to_string(),
    }
}
