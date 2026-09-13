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

/// Lets the codec use `count` threads, frame or slice threading as it
/// supports; `0` for as many as the machine has.
#[allow(unsafe_code)]
pub fn set_threads(ctx: &mut ffmpeg_next::codec::context::Context, count: u32) {
    // SAFETY: `ctx` owns an `AVCodecContext` that has not been opened yet;
    // `thread_type` and `thread_count` are plain integer fields read when
    // the codec opens.
    unsafe {
        let raw = &mut *ctx.as_mut_ptr();
        raw.thread_type = ffmpeg_next::ffi::FF_THREAD_FRAME | ffmpeg_next::ffi::FF_THREAD_SLICE;
        raw.thread_count = count as std::os::raw::c_int;
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

/// A codec context describing an 8-bit 4:2:0 H.264 stream produced
/// outside the library (by the system's x264), with its parameter sets as
/// extradata, for adding the stream to an output without an encoder.
#[allow(unsafe_code)]
pub fn h264_context(
    width: u32,
    height: u32,
    extradata: &[u8],
    tags: (
        ffmpeg_next::color::Space,
        ffmpeg_next::color::Range,
        ffmpeg_next::color::Primaries,
        ffmpeg_next::color::TransferCharacteristic,
    ),
) -> ffmpeg_next::codec::context::Context {
    use ffmpeg_next::ffi;
    let mut ctx = ffmpeg_next::codec::context::Context::new();
    // SAFETY: `ctx` owns a freshly allocated `AVCodecContext`, which frees
    // its extradata when it is dropped; the fields set are plain values,
    // and the extradata is allocated by the library's own allocator with
    // the padding decoders require.
    unsafe {
        let raw = &mut *ctx.as_mut_ptr();
        raw.codec_type = ffi::AVMediaType::AVMEDIA_TYPE_VIDEO;
        raw.codec_id = ffi::AVCodecID::AV_CODEC_ID_H264;
        raw.width = width as i32;
        raw.height = height as i32;
        raw.pix_fmt = ffi::AVPixelFormat::AV_PIX_FMT_YUV420P;
        raw.colorspace = tags.0.into();
        raw.color_range = tags.1.into();
        raw.color_primaries = tags.2.into();
        raw.color_trc = tags.3.into();
        if !extradata.is_empty() {
            let padded = extradata.len() + ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize;
            let buf = ffi::av_mallocz(padded).cast::<u8>();
            if !buf.is_null() {
                std::ptr::copy_nonoverlapping(extradata.as_ptr(), buf, extradata.len());
                raw.extradata = buf;
                raw.extradata_size = extradata.len() as i32;
            }
        }
    }
    ctx
}

/// A scaler that runs on several threads, through the library's
/// frame-based interface (the binding's own context uses the older,
/// single-threaded call). Used for conversions the generic path serves,
/// such as YCbCr to RGB with full chroma interpolation.
pub struct ThreadedScaler {
    ptr: *mut ffmpeg_next::ffi::SwsContext,
}

// SAFETY: the context is used from one thread at a time through `&mut`;
// its worker threads are owned by the context itself.
#[allow(unsafe_code)]
unsafe impl Send for ThreadedScaler {}

impl ThreadedScaler {
    /// Creates a scaler between the two layouts and sizes.
    #[allow(unsafe_code)]
    pub fn new(
        src: ffmpeg_next::util::format::Pixel,
        src_size: (u32, u32),
        dst: ffmpeg_next::util::format::Pixel,
        dst_size: (u32, u32),
        flags: ffmpeg_next::software::scaling::Flags,
        threads: usize,
    ) -> Result<Self, ffmpeg_next::Error> {
        use ffmpeg_next::ffi;
        // SAFETY: the context is freshly allocated and owned here; every
        // option name is one the library defines for its context, set
        // before the context is initialized; on failure it is freed.
        unsafe {
            let ptr = ffi::sws_alloc_context();
            if ptr.is_null() {
                return Err(ffmpeg_next::Error::Unknown);
            }
            let opts: [(&[u8], i64); 8] = [
                (b"srcw\0", i64::from(src_size.0)),
                (b"srch\0", i64::from(src_size.1)),
                (
                    b"src_format\0",
                    i64::from(ffi::AVPixelFormat::from(src) as i32),
                ),
                (b"dstw\0", i64::from(dst_size.0)),
                (b"dsth\0", i64::from(dst_size.1)),
                (
                    b"dst_format\0",
                    i64::from(ffi::AVPixelFormat::from(dst) as i32),
                ),
                (b"sws_flags\0", i64::from(flags.bits())),
                (b"threads\0", threads.max(1) as i64),
            ];
            for (name, value) in opts {
                let rc = ffi::av_opt_set_int(ptr.cast(), name.as_ptr().cast(), value, 0);
                if rc < 0 {
                    ffi::sws_freeContext(ptr);
                    return Err(ffmpeg_next::Error::from(rc));
                }
            }
            let rc = ffi::sws_init_context(ptr, std::ptr::null_mut(), std::ptr::null_mut());
            if rc < 0 {
                ffi::sws_freeContext(ptr);
                return Err(ffmpeg_next::Error::from(rc));
            }
            Ok(Self { ptr })
        }
    }

    /// Tells the scaler which matrix and range its YCbCr input uses, for a
    /// conversion to RGB. Without this the library assumes BT.601 for
    /// every source. Returns `false` when the matrix has no table (RGB
    /// sources).
    #[allow(unsafe_code)]
    pub fn set_input_colorspace(
        &mut self,
        matrix: geneva_color::Matrix,
        range: geneva_color::Range,
    ) -> bool {
        use ffmpeg_next::ffi;
        let space = match matrix {
            geneva_color::Matrix::Bt709 => ffi::SWS_CS_ITU709,
            geneva_color::Matrix::Bt601 => ffi::SWS_CS_ITU601,
            geneva_color::Matrix::Bt2020Ncl => ffi::SWS_CS_BT2020,
            geneva_color::Matrix::Identity => return false,
        };
        let src_full = i32::from(range == geneva_color::Range::Full);
        // SAFETY: the context is owned here; the coefficient tables are
        // static arrays owned by the library; the remaining arguments are
        // plain integers (neutral brightness, unit contrast and saturation).
        unsafe {
            let table = ffi::sws_getCoefficients(space);
            let output = ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT);
            ffi::sws_setColorspaceDetails(self.ptr, table, src_full, output, 1, 0, 1 << 16, 1 << 16)
                >= 0
        }
    }

    /// Converts `src` into `dst`, which must be allocated for the
    /// destination layout and size.
    #[allow(unsafe_code)]
    pub fn run(
        &mut self,
        src: &ffmpeg_next::util::frame::Video,
        dst: &mut ffmpeg_next::util::frame::Video,
    ) -> Result<(), ffmpeg_next::Error> {
        // SAFETY: both frames wrap valid `AVFrame`s whose layouts match the
        // ones the context was created for; the library checks them.
        let rc =
            unsafe { ffmpeg_next::ffi::sws_scale_frame(self.ptr, dst.as_mut_ptr(), src.as_ptr()) };
        if rc < 0 {
            Err(ffmpeg_next::Error::from(rc))
        } else {
            Ok(())
        }
    }
}

/// A view of the region `x, y, width, height` of `src`, sharing its
/// buffers: a frame whose data pointers start at the region and whose
/// size is the region's, for scaling or copying a crop.
#[allow(unsafe_code)]
pub fn cropped(
    src: &ffmpeg_next::util::frame::Video,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<ffmpeg_next::util::frame::Video, ffmpeg_next::Error> {
    use ffmpeg_next::ffi;
    let mut out = ffmpeg_next::util::frame::Video::empty();
    // SAFETY: `out` is an empty frame that `av_frame_ref` fills with a
    // reference to `src`'s buffers (or a copy when they are not
    // reference-counted); the crop fields are set before the library
    // applies them, and the region lies inside the frame.
    unsafe {
        let rc = ffi::av_frame_ref(out.as_mut_ptr(), src.as_ptr());
        if rc < 0 {
            return Err(ffmpeg_next::Error::from(rc));
        }
        let f = &mut *out.as_mut_ptr();
        let (x, y, w, h) = (x as usize, y as usize, width as usize, height as usize);
        f.crop_left = x;
        f.crop_top = y;
        f.crop_right = (src.width() as usize).saturating_sub(x + w);
        f.crop_bottom = (src.height() as usize).saturating_sub(y + h);
        let rc =
            ffi::av_frame_apply_cropping(out.as_mut_ptr(), ffi::AV_FRAME_CROP_UNALIGNED as i32);
        if rc < 0 {
            return Err(ffmpeg_next::Error::from(rc));
        }
    }
    Ok(out)
}

impl Drop for ThreadedScaler {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: `ptr` was returned by `sws_alloc_context` and is freed
        // exactly once.
        unsafe { ffmpeg_next::ffi::sws_freeContext(self.ptr) };
    }
}

/// Makes `frame`'s storage ours to write: when an encoder still holds a
/// reference to it, the frame gets fresh storage, otherwise nothing
/// changes.
#[allow(unsafe_code)]
pub fn make_writable(
    frame: &mut ffmpeg_next::util::frame::Video,
) -> Result<(), ffmpeg_next::Error> {
    // SAFETY: `frame` is a valid, allocated frame for as long as the borrow.
    let rc = unsafe { ffmpeg_next::ffi::av_frame_make_writable(frame.as_mut_ptr()) };
    if rc < 0 {
        return Err(ffmpeg_next::Error::from(rc));
    }
    Ok(())
}

/// Static HDR10 metadata as the libraries lay it out: the mastering
/// display block (`AVMasteringDisplayMetadata`) and the content light
/// level block (`AVContentLightMetadata`), kept as bytes so a source's
/// can be carried to the output as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HdrMetadata {
    mastering: Option<Vec<u8>>,
    light: Option<Vec<u8>>,
}

impl HdrMetadata {
    /// Standard defaults for a PQ output without a source to copy from: a
    /// P3-D65 mastering display of 1000 nits (0.0001 nits black), MaxCLL
    /// 1000 and MaxFALL 400.
    pub fn defaults() -> Self {
        let rational = |num: i32, den: i32| {
            let mut v = num.to_ne_bytes().to_vec();
            v.extend_from_slice(&den.to_ne_bytes());
            v
        };
        let mut mastering = Vec::with_capacity(88);
        // Display primaries R, G, B and the white point, as x then y,
        // in fifty-thousandths (the SEI's unit).
        for (x, y) in [
            (34_000, 16_000),
            (13_250, 34_500),
            (7_500, 3_000),
            (15_635, 16_450),
        ] {
            mastering.extend(rational(x, 50_000));
            mastering.extend(rational(y, 50_000));
        }
        mastering.extend(rational(1, 10_000));
        mastering.extend(rational(1000, 1));
        mastering.extend(1i32.to_ne_bytes());
        mastering.extend(1i32.to_ne_bytes());
        let mut light = Vec::with_capacity(8);
        light.extend(1000u32.to_ne_bytes());
        light.extend(400u32.to_ne_bytes());
        Self {
            mastering: Some(mastering),
            light: Some(light),
        }
    }

    /// The peak the metadata declares: MaxCLL, else the mastering
    /// display's maximum luminance.
    pub fn peak_nits(&self) -> Option<f64> {
        if let Some(light) = &self.light {
            if light.len() >= 4 {
                let max_cll = u32::from_ne_bytes([light[0], light[1], light[2], light[3]]);
                if max_cll > 0 {
                    return Some(f64::from(max_cll));
                }
            }
        }
        let bytes = self.mastering.as_ref()?;
        if bytes.len() < 88 {
            return None;
        }
        let int = |at: usize| {
            i32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let (num, den, has_luminance) = (int(72), int(76), int(84));
        (has_luminance != 0 && den != 0 && num > 0).then(|| f64::from(num) / f64::from(den))
    }
}

/// The static HDR metadata a stream carries, if any block is present.
#[allow(unsafe_code)]
pub fn hdr_metadata(params: &Parameters) -> Option<HdrMetadata> {
    use ffmpeg_next::ffi::{AVPacketSideDataType, av_packet_side_data_get};
    // SAFETY: the parameters are valid for the borrow; the side data
    // array and count come from the same struct, and each entry's bytes
    // are read only within its declared size.
    let get = |kind: AVPacketSideDataType| unsafe {
        let raw = &*params.as_ptr();
        let sd = av_packet_side_data_get(raw.coded_side_data, raw.nb_coded_side_data, kind);
        if sd.is_null() {
            None
        } else {
            Some(std::slice::from_raw_parts((*sd).data, (*sd).size).to_vec())
        }
    };
    let meta = HdrMetadata {
        mastering: get(AVPacketSideDataType::AV_PKT_DATA_MASTERING_DISPLAY_METADATA),
        light: get(AVPacketSideDataType::AV_PKT_DATA_CONTENT_LIGHT_LEVEL),
    };
    (meta.mastering.is_some() || meta.light.is_some()).then_some(meta)
}

/// The display matrix a stream carries, as its 36 bytes, if any.
#[allow(unsafe_code)]
pub fn display_matrix(params: &Parameters) -> Option<Vec<u8>> {
    use ffmpeg_next::ffi::{AVPacketSideDataType, av_packet_side_data_get};
    // SAFETY: as in `hdr_metadata`: the side data array and count come
    // from the same valid struct, and the entry is read within its size.
    unsafe {
        let raw = &*params.as_ptr();
        let sd = av_packet_side_data_get(
            raw.coded_side_data,
            raw.nb_coded_side_data,
            AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
        );
        if sd.is_null() || (*sd).size < 36 {
            None
        } else {
            Some(std::slice::from_raw_parts((*sd).data, 36).to_vec())
        }
    }
}

/// The rotation a stream asks its player for, in degrees clockwise:
/// 0, 90, 180 or 270. Phones record portrait video as a landscape stream
/// with a 90 or 270 here. Read from the display matrix the way ffmpeg's
/// own tools do (`av_display_rotation_get`, negated and brought into
/// `[0, 360)`), and rounded to the nearest right angle.
pub fn display_rotation(params: &Parameters) -> u16 {
    let Some(bytes) = display_matrix(params) else {
        return 0;
    };
    let m: Vec<f64> = bytes
        .chunks_exact(4)
        .map(|c| f64::from(i32::from_ne_bytes([c[0], c[1], c[2], c[3]])) / 65536.0)
        .collect();
    let scale0 = m[0].hypot(m[3]);
    let scale1 = m[1].hypot(m[4]);
    if scale0 == 0.0 || scale1 == 0.0 {
        return 0;
    }
    // `av_display_rotation_get` returns the negation of this angle (the
    // counterclockwise rotation), and ffmpeg's tools negate it again to
    // get the clockwise rotation to apply for display.
    let theta = (m[1] / scale1).atan2(m[0] / scale0).to_degrees();
    let theta = theta - 360.0 * (theta / 360.0 + 0.9 / 360.0).floor();
    let quarter = (theta / 90.0).round() as i64;
    ((quarter.rem_euclid(4)) * 90) as u16
}

/// Attaches a display matrix to an output stream's parameters, so that
/// a copied stream keeps the rotation its source asked for.
#[allow(unsafe_code)]
pub fn attach_display_matrix(stream: &mut ffmpeg_next::format::stream::StreamMut, bytes: &[u8]) {
    use ffmpeg_next::ffi::{AVPacketSideDataType, av_malloc, av_packet_side_data_add};
    // SAFETY: as in `attach_hdr_metadata_to_stream`.
    unsafe {
        let par = (*stream.as_mut_ptr()).codecpar;
        let buf = av_malloc(bytes.len()).cast::<u8>();
        if buf.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
        let added = av_packet_side_data_add(
            &raw mut (*par).coded_side_data,
            &raw mut (*par).nb_coded_side_data,
            AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
            buf.cast(),
            bytes.len(),
            0,
        );
        if added.is_null() {
            ffmpeg_next::ffi::av_free(buf.cast());
        }
    }
}

/// The peak luminance in nits a stream's HDR metadata declares.
pub fn hdr_peak_nits(params: &Parameters) -> Option<f64> {
    hdr_metadata(params).and_then(|m| m.peak_nits())
}

/// Attaches the metadata to an output stream's parameters, for the
/// container to write (MP4's `mdcv` and `clli`, Matroska's mastering
/// element).
#[allow(unsafe_code)]
pub fn attach_hdr_metadata_to_stream(
    stream: &mut ffmpeg_next::format::stream::StreamMut,
    meta: &HdrMetadata,
) {
    use ffmpeg_next::ffi::{AVPacketSideDataType, av_malloc, av_packet_side_data_add};
    let blocks = [
        (
            AVPacketSideDataType::AV_PKT_DATA_MASTERING_DISPLAY_METADATA,
            &meta.mastering,
        ),
        (
            AVPacketSideDataType::AV_PKT_DATA_CONTENT_LIGHT_LEVEL,
            &meta.light,
        ),
    ];
    for (kind, bytes) in blocks {
        let Some(bytes) = bytes else { continue };
        // SAFETY: the stream is valid for the borrow; the buffer is
        // allocated with the library's allocator and handed to it, which
        // owns it from then on.
        unsafe {
            let par = (*stream.as_mut_ptr()).codecpar;
            let buf = av_malloc(bytes.len()).cast::<u8>();
            if buf.is_null() {
                continue;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
            let added = av_packet_side_data_add(
                &raw mut (*par).coded_side_data,
                &raw mut (*par).nb_coded_side_data,
                kind,
                buf.cast(),
                bytes.len(),
                0,
            );
            if added.is_null() {
                ffmpeg_next::ffi::av_free(buf.cast());
            }
        }
    }
}

/// Attaches the metadata to a frame, for encoders that write it into
/// the stream (HEVC's SEI messages, AV1's metadata OBUs).
#[allow(unsafe_code)]
pub fn attach_hdr_metadata_to_frame(
    frame: &mut ffmpeg_next::util::frame::Video,
    meta: &HdrMetadata,
) {
    use ffmpeg_next::ffi::{AVFrameSideDataType, av_frame_new_side_data};
    let blocks = [
        (
            AVFrameSideDataType::AV_FRAME_DATA_MASTERING_DISPLAY_METADATA,
            &meta.mastering,
        ),
        (
            AVFrameSideDataType::AV_FRAME_DATA_CONTENT_LIGHT_LEVEL,
            &meta.light,
        ),
    ];
    for (kind, bytes) in blocks {
        let Some(bytes) = bytes else { continue };
        // SAFETY: the frame is valid for the borrow; the library allocates
        // the side data buffer at the size asked for, and it is filled
        // within that size.
        unsafe {
            let sd = av_frame_new_side_data(frame.as_mut_ptr(), kind, bytes.len());
            if !sd.is_null() {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), (*sd).data, bytes.len());
            }
        }
    }
}
