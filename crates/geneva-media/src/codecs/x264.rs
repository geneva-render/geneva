//! Software H.264 through the system's x264 library.
//!
//! Nothing of x264 is built into geneva. When the distribution's x264
//! package is installed (`libx264.so.NNN` on Linux, Homebrew's dylib on
//! macOS) the library is loaded at run time and used through its C API,
//! ahead of the bundled OpenH264; without it nothing changes. The
//! `GENEVA_X264` environment variable names the library file to load, or
//! turns the lookup off when set to `off`.
//!
//! Only parts of the interface that have stayed the same across builds
//! are used: named parameters through `x264_param_parse`, the leading
//! fields of the parameter and picture structures, and the NAL array.
//! The build number is read from the library and checked against the
//! range this was written for; anything else is treated as not installed.
//!
//! This module is the second place in the crate that needs `unsafe`: the
//! calls into the loaded library, each with the invariant it relies on.
#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::OnceLock;

use ffmpeg_next::Rational;
use geneva_color::{Matrix, Primaries, Range, ResolvedTags, Transfer};

use crate::MediaError;
use crate::convert::{PlaneFormat, Planes};

/// Oldest x264 build whose structures this module knows (2017).
const OLDEST_BUILD: c_int = 155;
/// Newest build accepted; later builds may move the fields read here.
const NEWEST_BUILD: c_int = 175;

/// Room for `x264_param_t` of any build: the structure is under 2 KB.
const PARAM_BYTES: usize = 16384;
/// `x264_param_t` field offsets, unchanged since build 153.
const OFF_WIDTH: usize = 28;
const OFF_HEIGHT: usize = 32;
const OFF_CSP: usize = 36;
/// `X264_CSP_I420`.
const CSP_I420: c_int = 0x0002;
/// `NAL_SEI`.
const NAL_SEI: c_int = 6;

/// `x264_nal_t`.
#[repr(C)]
struct Nal {
    i_ref_idc: c_int,
    i_type: c_int,
    b_long_startcode: c_int,
    i_first_mb: c_int,
    i_last_mb: c_int,
    i_payload: c_int,
    p_payload: *mut u8,
    i_padding: c_int,
}

/// `x264_image_t`.
#[repr(C)]
struct Image {
    i_csp: c_int,
    i_plane: c_int,
    i_stride: [c_int; 4],
    plane: [*mut u8; 4],
}

/// The leading fields of `x264_picture_t`.
#[repr(C)]
struct PictureHead {
    i_type: c_int,
    i_qpplus1: c_int,
    i_pic_struct: c_int,
    b_keyframe: c_int,
    i_pts: i64,
    i_dts: i64,
    param: *mut c_void,
    img: Image,
}

/// `x264_picture_t` with room for the fields after the ones read here.
#[repr(C, align(16))]
struct Picture {
    head: PictureHead,
    _rest: [u8; 1024],
}

impl Picture {
    fn zeroed() -> Box<Self> {
        Box::new(Self {
            head: PictureHead {
                i_type: 0,
                i_qpplus1: 0,
                i_pic_struct: 0,
                b_keyframe: 0,
                i_pts: 0,
                i_dts: 0,
                param: std::ptr::null_mut(),
                img: Image {
                    i_csp: 0,
                    i_plane: 0,
                    i_stride: [0; 4],
                    plane: [std::ptr::null_mut(); 4],
                },
            },
            _rest: [0; 1024],
        })
    }
}

/// `x264_param_t` as opaque, suitably aligned storage.
#[repr(C, align(16))]
struct Params([u8; PARAM_BYTES]);

type ParamDefaultPreset = unsafe extern "C" fn(*mut Params, *const c_char, *const c_char) -> c_int;
type ParamParse = unsafe extern "C" fn(*mut Params, *const c_char, *const c_char) -> c_int;
type ParamApplyProfile = unsafe extern "C" fn(*mut Params, *const c_char) -> c_int;
type EncoderOpen = unsafe extern "C" fn(*mut Params) -> *mut c_void;
type EncoderHeaders = unsafe extern "C" fn(*mut c_void, *mut *mut Nal, *mut c_int) -> c_int;
type EncoderEncode = unsafe extern "C" fn(
    *mut c_void,
    *mut *mut Nal,
    *mut c_int,
    *mut Picture,
    *mut Picture,
) -> c_int;
type EncoderDelayedFrames = unsafe extern "C" fn(*mut c_void) -> c_int;
type EncoderClose = unsafe extern "C" fn(*mut c_void);
type PictureInit = unsafe extern "C" fn(*mut Picture);

/// A loaded x264 library.
pub struct X264Lib {
    /// Keeps the library mapped for as long as the function pointers live.
    _library: libloading::Library,
    /// The library's `X264_BUILD`.
    pub build: c_int,
    /// Where it was loaded from.
    pub path: String,
    param_default_preset: ParamDefaultPreset,
    param_parse: ParamParse,
    param_apply_profile: ParamApplyProfile,
    encoder_open: EncoderOpen,
    encoder_headers: EncoderHeaders,
    encoder_encode: EncoderEncode,
    encoder_delayed_frames: EncoderDelayedFrames,
    encoder_close: EncoderClose,
    picture_init: PictureInit,
}

/// The system's x264, loaded on first use, or `None` when it is not
/// installed, turned off, or of a build this module does not know.
pub fn library() -> Option<&'static X264Lib> {
    loaded().as_ref().ok()
}

/// Why the system's x264 could not be used although a library file was
/// found: a build outside the known range, or a missing symbol. `None`
/// when it loaded or when no library file exists.
pub fn load_error() -> Option<&'static str> {
    match loaded() {
        Err(Some(e)) => Some(e.as_str()),
        _ => None,
    }
}

fn loaded() -> &'static Result<X264Lib, Option<String>> {
    static LIB: OnceLock<Result<X264Lib, Option<String>>> = OnceLock::new();
    LIB.get_or_init(load)
}

/// Library names to try, most specific first.
fn candidates() -> Vec<String> {
    if let Ok(v) = std::env::var("GENEVA_X264") {
        if v.is_empty() || v == "0" || v.eq_ignore_ascii_case("off") {
            return Vec::new();
        }
        return vec![v];
    }
    let mut names = Vec::new();
    if cfg!(target_os = "macos") {
        for dir in ["/opt/homebrew/lib", "/usr/local/lib"] {
            names.push(format!("{dir}/libx264.dylib"));
        }
        names.push("libx264.dylib".to_owned());
    } else {
        names.push("libx264.so".to_owned());
        for build in (OLDEST_BUILD..=NEWEST_BUILD).rev() {
            names.push(format!("libx264.so.{build}"));
        }
    }
    names
}

/// `Err(None)` when no library file was found; `Err(Some(why))` when one
/// was found but could not be used.
fn load() -> Result<X264Lib, Option<String>> {
    let mut problem = None;
    for name in candidates() {
        // SAFETY: loading a shared library runs its initializers; x264's
        // do nothing beyond setting up its own tables.
        let Ok(library) = (unsafe { libloading::Library::new(&name) }) else {
            continue;
        };
        match bind(library, name.clone()) {
            Ok(lib) => return Ok(lib),
            Err(e) => problem = Some(format!("{name}: {e}")),
        }
    }
    Err(problem)
}

fn bind(library: libloading::Library, path: String) -> Result<X264Lib, String> {
    // The build number is not exported as data by every distribution's
    // library, but `x264_encoder_open` carries it in its name.
    // SAFETY: every symbol is looked up by the name x264.h declares it
    // under and read with the declared signature.
    unsafe {
        let mut found = None;
        for build in (OLDEST_BUILD..=NEWEST_BUILD).rev() {
            let name = format!("x264_encoder_open_{build}\0");
            if let Ok(sym) = library.get::<EncoderOpen>(name.as_bytes()) {
                found = Some((build, *sym));
                break;
            }
        }
        let Some((build, encoder_open)) = found else {
            return Err(format!(
                "no x264_encoder_open for a build in {OLDEST_BUILD}..={NEWEST_BUILD}, the range this version of geneva knows"
            ));
        };
        Ok(X264Lib {
            build,
            path,
            param_default_preset: *library
                .get(b"x264_param_default_preset\0")
                .map_err(|e| e.to_string())?,
            param_parse: *library
                .get(b"x264_param_parse\0")
                .map_err(|e| e.to_string())?,
            param_apply_profile: *library
                .get(b"x264_param_apply_profile\0")
                .map_err(|e| e.to_string())?,
            encoder_open,
            encoder_headers: *library
                .get(b"x264_encoder_headers\0")
                .map_err(|e| e.to_string())?,
            encoder_encode: *library
                .get(b"x264_encoder_encode\0")
                .map_err(|e| e.to_string())?,
            encoder_delayed_frames: *library
                .get(b"x264_encoder_delayed_frames\0")
                .map_err(|e| e.to_string())?,
            encoder_close: *library
                .get(b"x264_encoder_close\0")
                .map_err(|e| e.to_string())?,
            picture_init: *library
                .get(b"x264_picture_init\0")
                .map_err(|e| e.to_string())?,
            _library: library,
        })
    }
}

/// One encoded picture in coded order.
pub struct EncodedFrame {
    /// Annex B NAL units (start codes included).
    pub data: Vec<u8>,
    /// Presentation time in frames.
    pub pts: i64,
    /// Decode time in frames; the first few can be negative.
    pub dts: i64,
    /// Whether the picture is an IDR or recovery point.
    pub keyframe: bool,
}

/// What the encoder needs to know about the picture.
pub struct X264Settings<'a> {
    /// Picture size.
    pub width: u32,
    /// Picture size.
    pub height: u32,
    /// Frame rate.
    pub fps: Rational,
    /// Constant rate factor, 0..=51.
    pub crf: u8,
    /// x264 preset name.
    pub preset: &'a str,
    /// Color tags written into the stream.
    pub color: ResolvedTags,
    /// Whether parameter sets go into the container header (and not into
    /// the stream) as MP4, MOV and Matroska expect.
    pub global_header: bool,
}

/// An open x264 encoder taking 8-bit 4:2:0 pictures.
pub struct X264Encoder {
    lib: &'static X264Lib,
    handle: *mut c_void,
    extradata: Vec<u8>,
    width: u32,
    height: u32,
    pic_in: Box<Picture>,
    pic_out: Box<Picture>,
}

// SAFETY: the handle is used from one thread at a time through `&mut`;
// x264 has no thread affinity for its encoder handles.
unsafe impl Send for X264Encoder {}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn setup_error(reason: impl Into<String>) -> MediaError {
    MediaError::Codec {
        context: "x264 setup".to_owned(),
        reason: reason.into(),
    }
}

impl X264Encoder {
    /// Opens the system's x264, or fails when it is not available.
    pub fn open(settings: &X264Settings<'_>) -> Result<Self, MediaError> {
        let lib = match (library(), load_error()) {
            (Some(lib), _) => lib,
            (None, Some(why)) => return Err(setup_error(why)),
            (None, None) => {
                return Err(MediaError::MissingEncoder {
                    name: "libx264".to_owned(),
                });
            }
        };
        let mut params = Box::new(Params([0; PARAM_BYTES]));
        let p: *mut Params = &raw mut *params;
        let preset = cstr(settings.preset);
        // SAFETY: `params` is writable storage larger than any build's
        // `x264_param_t`; the strings are NUL-terminated.
        let rc = unsafe { (lib.param_default_preset)(p, preset.as_ptr(), std::ptr::null()) };
        if rc != 0 {
            return Err(setup_error(format!("unknown preset {:?}", settings.preset)));
        }
        let (prim, transfer, matrix) = color_names(settings.color);
        let full_range = if settings.color.range == Range::Full {
            "on"
        } else {
            "off"
        };
        let repeat_headers = if settings.global_header { "0" } else { "1" };
        let fps = format!(
            "{}/{}",
            settings.fps.numerator(),
            settings.fps.denominator()
        );
        let crf = settings.crf.to_string();
        let options: [(&str, &str); 10] = [
            ("crf", &crf),
            ("fps", &fps),
            // Timestamps are frame indices; rate control goes by the frame rate.
            ("force-cfr", "1"),
            ("repeat-headers", repeat_headers),
            ("log", "0"),
            ("colorprim", prim),
            ("transfer", transfer),
            ("colormatrix", matrix),
            ("fullrange", full_range),
            ("annexb", "1"),
        ];
        for (name, value) in options {
            let (n, v) = (cstr(name), cstr(value));
            // SAFETY: as above.
            let rc = unsafe { (lib.param_parse)(p, n.as_ptr(), v.as_ptr()) };
            if rc != 0 {
                return Err(setup_error(format!("x264 rejected {name}={value}")));
            }
        }
        // SAFETY: the offsets are those of `i_width`, `i_height` and
        // `i_csp` in every build in the accepted range; `params` is
        // aligned to 16 and the offsets are multiples of 4.
        #[allow(clippy::cast_ptr_alignment)]
        unsafe {
            let base = params.0.as_mut_ptr();
            base.add(OFF_WIDTH)
                .cast::<c_int>()
                .write(settings.width as c_int);
            base.add(OFF_HEIGHT)
                .cast::<c_int>()
                .write(settings.height as c_int);
            base.add(OFF_CSP).cast::<c_int>().write(CSP_I420);
        }
        let profile = cstr("high");
        // SAFETY: as above.
        let rc = unsafe { (lib.param_apply_profile)(p, profile.as_ptr()) };
        if rc != 0 {
            return Err(setup_error("the high profile was refused"));
        }
        // SAFETY: `params` is fully initialized by the calls above; x264
        // copies what it needs.
        let handle = unsafe { (lib.encoder_open)(p) };
        if handle.is_null() {
            return Err(setup_error("x264_encoder_open failed"));
        }
        let mut enc = Self {
            lib,
            handle,
            extradata: Vec::new(),
            width: settings.width,
            height: settings.height,
            pic_in: Picture::zeroed(),
            pic_out: Picture::zeroed(),
        };
        if settings.global_header {
            enc.extradata = enc.headers()?;
        }
        Ok(enc)
    }

    /// The build number of the library in use.
    pub fn build(&self) -> i32 {
        self.lib.build
    }

    /// Parameter sets (SPS and PPS, Annex B) for the container header;
    /// empty unless `global_header` was requested.
    pub fn extradata(&self) -> &[u8] {
        &self.extradata
    }

    fn headers(&mut self) -> Result<Vec<u8>, MediaError> {
        let mut nals: *mut Nal = std::ptr::null_mut();
        let mut count: c_int = 0;
        // SAFETY: `handle` is an open encoder; x264 fills the NAL array,
        // which stays valid until the next call on the handle.
        let size =
            unsafe { (self.lib.encoder_headers)(self.handle, &raw mut nals, &raw mut count) };
        if size < 0 {
            return Err(setup_error("x264_encoder_headers failed"));
        }
        let mut out = Vec::with_capacity(size as usize);
        for i in 0..count as usize {
            // SAFETY: `count` NALs were returned, each with a payload of
            // `i_payload` bytes.
            let nal = unsafe { &*nals.add(i) };
            if nal.i_type == NAL_SEI {
                continue;
            }
            // SAFETY: as above.
            let payload =
                unsafe { std::slice::from_raw_parts(nal.p_payload, nal.i_payload as usize) };
            out.extend_from_slice(payload);
        }
        Ok(out)
    }

    /// Encodes one picture. Nothing may come out for the first few
    /// pictures while the encoder looks ahead.
    pub fn encode(
        &mut self,
        planes: &Planes,
        pts: i64,
    ) -> Result<Option<EncodedFrame>, MediaError> {
        if planes.format != PlaneFormat::Yuv420p8
            || planes.width != self.width
            || planes.height != self.height
        {
            return Err(MediaError::Codec {
                context: "encoding video".to_owned(),
                reason: format!(
                    "x264 was opened for {}×{} 8-bit 4:2:0 and got {}×{} {}",
                    self.width,
                    self.height,
                    planes.width,
                    planes.height,
                    planes.format.name()
                ),
            });
        }
        // SAFETY: `pic_in` is writable storage larger than `x264_picture_t`.
        unsafe { (self.lib.picture_init)(&raw mut *self.pic_in) };
        let head = &mut self.pic_in.head;
        head.i_pts = pts;
        head.img.i_csp = CSP_I420;
        head.img.i_plane = 3;
        for (i, plane) in planes.planes.iter().enumerate().take(3) {
            head.img.i_stride[i] = plane.stride as c_int;
            head.img.plane[i] = plane.data.as_ptr().cast_mut();
        }
        self.run(true)
    }

    /// Takes one delayed picture out of the encoder after the last input;
    /// `None` once it is empty.
    pub fn flush(&mut self) -> Result<Option<EncodedFrame>, MediaError> {
        // A call can consume a queued frame without producing one yet, so
        // keep asking while frames remain (as FFmpeg's wrapper does).
        loop {
            // SAFETY: `handle` is an open encoder.
            if unsafe { (self.lib.encoder_delayed_frames)(self.handle) } <= 0 {
                return Ok(None);
            }
            if let Some(frame) = self.run(false)? {
                return Ok(Some(frame));
            }
        }
    }

    fn run(&mut self, with_input: bool) -> Result<Option<EncodedFrame>, MediaError> {
        let mut nals: *mut Nal = std::ptr::null_mut();
        let mut count: c_int = 0;
        let pic_in: *mut Picture = if with_input {
            &raw mut *self.pic_in
        } else {
            std::ptr::null_mut()
        };
        // SAFETY: `handle` is open; `pic_in` points at planes that outlive
        // the call (x264 copies them before returning); `pic_out` is
        // writable storage larger than `x264_picture_t`. The returned
        // NALs' payloads are consecutive in memory and add up to the
        // returned size.
        let size = unsafe {
            (self.lib.encoder_encode)(
                self.handle,
                &raw mut nals,
                &raw mut count,
                pic_in,
                &raw mut *self.pic_out,
            )
        };
        if size < 0 {
            return Err(MediaError::Codec {
                context: "encoding video".to_owned(),
                reason: "x264_encoder_encode failed".to_owned(),
            });
        }
        if size == 0 || count == 0 {
            return Ok(None);
        }
        // SAFETY: as above.
        let data = unsafe { std::slice::from_raw_parts((*nals).p_payload, size as usize) }.to_vec();
        let head = &self.pic_out.head;
        Ok(Some(EncodedFrame {
            data,
            pts: head.i_pts,
            dts: head.i_dts,
            keyframe: head.b_keyframe != 0,
        }))
    }
}

impl Drop for X264Encoder {
    fn drop(&mut self) {
        // SAFETY: `handle` was returned by `x264_encoder_open` and is
        // closed exactly once.
        unsafe { (self.lib.encoder_close)(self.handle) };
    }
}

/// x264's names for the color tags.
fn color_names(tags: ResolvedTags) -> (&'static str, &'static str, &'static str) {
    let prim = match tags.primaries {
        Primaries::Bt709 => "bt709",
        Primaries::Bt601_625 => "bt470bg",
        Primaries::Bt601_525 => "smpte170m",
        Primaries::Bt2020 => "bt2020",
    };
    let transfer = match tags.transfer {
        Transfer::Bt709 => "bt709",
        Transfer::Srgb => "iec61966-2-1",
        Transfer::Linear => "linear",
        Transfer::Pq => "smpte2084",
        Transfer::Hlg => "arib-std-b67",
    };
    let matrix = match tags.matrix {
        Matrix::Bt709 => "bt709",
        Matrix::Bt601 => "smpte170m",
        Matrix::Bt2020Ncl => "bt2020nc",
        Matrix::Identity => "GBR",
    };
    (prim, transfer, matrix)
}
