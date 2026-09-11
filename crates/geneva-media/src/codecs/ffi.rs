//! The one place in the workspace that touches raw library structures.
//!
//! The safe binding does not expose codec extradata or the container codec
//! tag, both of which stream copying needs. Each function here reads or
//! writes one field of a structure the binding already owns.

use ffmpeg_next::codec::Parameters;

/// The codec extradata (for example H.264 parameter sets) of a stream's
/// parameters, or an empty vector when there is none.
#[allow(unsafe_code)]
pub fn extradata(params: &Parameters) -> Vec<u8> {
    // SAFETY: `params` wraps a valid `AVCodecParameters` owned by the
    // binding for its whole lifetime; `extradata` is either null with
    // `extradata_size == 0` or points at `extradata_size` readable bytes.
    unsafe {
        let raw = &*params.as_ptr();
        if raw.extradata.is_null() || raw.extradata_size <= 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(raw.extradata, raw.extradata_size as usize).to_vec()
        }
    }
}

/// Clears the container-specific codec tag so the output muxer picks its
/// own; copying a tag between different containers is rejected by muxers.
#[allow(unsafe_code)]
pub fn clear_codec_tag(params: &mut Parameters) {
    // SAFETY: as above; `codec_tag` is a plain integer field.
    unsafe {
        (*params.as_mut_ptr()).codec_tag = 0;
    }
}

/// A codec context describing a subtitle stream of the given codec, for
/// adding a text stream to an output without an encoder.
#[allow(unsafe_code)]
pub fn subtitle_context(id: ffmpeg_next::codec::Id) -> ffmpeg_next::codec::context::Context {
    let mut ctx = ffmpeg_next::codec::context::Context::new();
    // SAFETY: `ctx` owns a freshly allocated `AVCodecContext`; `codec_type`
    // and `codec_id` are plain enum fields the muxer reads when the stream's
    // parameters are copied from the context.
    unsafe {
        let raw = &mut *ctx.as_mut_ptr();
        raw.codec_type = ffmpeg_next::ffi::AVMediaType::AVMEDIA_TYPE_SUBTITLE;
        raw.codec_id = id.into();
    }
    ctx
}

/// Lets the codec use every core: frame and slice threading are both
/// permitted (the codec picks what it supports) and the thread count is
/// chosen from the machine.
#[allow(unsafe_code)]
pub fn use_all_threads(ctx: &mut ffmpeg_next::codec::context::Context) {
    // SAFETY: `ctx` owns an `AVCodecContext` that has not been opened yet;
    // `thread_type` and `thread_count` are plain integer fields read when
    // the codec opens.
    unsafe {
        let raw = &mut *ctx.as_mut_ptr();
        raw.thread_type = ffmpeg_next::ffi::FF_THREAD_FRAME | ffmpeg_next::ffi::FF_THREAD_SLICE;
        raw.thread_count = 0;
    }
}

/// Whether a pixel format holds RGB samples (as opposed to YCbCr or gray).
#[allow(unsafe_code)]
pub fn is_rgb(pixel: ffmpeg_next::util::format::Pixel) -> bool {
    let Some(descriptor) = pixel.descriptor() else {
        return false;
    };
    // SAFETY: the descriptor points at a static table entry owned by the
    // library; `flags` is a plain integer field.
    let flags = unsafe { (*descriptor.as_ptr()).flags };
    flags & (1u64 << 5) != 0 // AV_PIX_FMT_FLAG_RGB
}
