use std::path::{Path, PathBuf};

use ffmpeg_next::codec;
use ffmpeg_next::software::resampling;
use ffmpeg_next::util::channel_layout::ChannelLayout;
use ffmpeg_next::util::format::{Pixel, Sample, sample};
use ffmpeg_next::util::frame;
use ffmpeg_next::{Dictionary, Error as FfError, Packet, Rational};
use geneva_color::ResolvedTags;
use geneva_render::Frame;
use geneva_timeline::Ratio;
use geneva_timeline::schema::{
    AudioCodec, Container, HardwarePolicy, VideoCodec, VideoProfile, VideoTune,
};

use super::subtitle_streams::{SubtitleSettings, SubtitleWriter};
use super::x264::{X264Encoder, X264Settings};
use super::{codec_error, ffi, h264, init, open_error, tags};
use crate::MediaError;
use crate::convert::{PlaneFormat, Planes, frame_to_planes};

/// What to write and how.
#[derive(Debug, Clone)]
pub struct EncodeSettings {
    /// Video track settings; `None` writes no video.
    pub video: Option<VideoSettings>,
    /// Container. Chosen from the output path's extension when `None`.
    pub container: Option<Container>,
    /// Audio track settings; `None` writes no audio.
    pub audio: Option<AudioSettings>,
    /// Subtitle tracks written as text streams.
    pub subtitles: Vec<SubtitleSettings>,
    /// Put the index of MP4, MOV and M4A files at the front (a second
    /// pass over the file when it is complete).
    pub fast_start: bool,
    /// Instead of `audio`, an audio stream with the parameters of this
    /// file's, whose packets come through [`Encoder::write_copied_audio`].
    pub copied_audio: Option<PathBuf>,
}

/// Video track settings.
#[derive(Debug, Clone)]
pub struct VideoSettings {
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Frame rate.
    pub fps: Ratio,
    /// Codec.
    pub codec: VideoCodec,
    /// Constant-quality level, when the codec supports one.
    pub crf: Option<u8>,
    /// Encoder speed preset name.
    pub preset: Option<String>,
    /// Whether to use a hardware encoder. Defaults to trying hardware first.
    pub hardware: HardwarePolicy,
    /// Output color tags; frames are converted to and tagged with these.
    pub color: ResolvedTags,
    /// Codec profile, for the codecs that have them.
    pub profile: Option<VideoProfile>,
    /// Seconds between keyframes; the encoder's own choice when `None`.
    pub keyframe_interval: Option<f64>,
    /// Bitrate ceiling in kb/s; constant quality below it.
    pub max_bitrate_kbps: Option<u32>,
    /// Average bitrate in kb/s: bitrate mode for the encoders that cannot
    /// hold constant quality under a ceiling (hardware); x264 ignores it.
    pub bitrate_kbps: Option<u32>,
    /// Write the track by stitching copied H.264 packets and encoded
    /// runs (smart cut); see [`StitchSettings`].
    pub stitch: Option<StitchSettings>,
    /// H.264/H.265 level such as "4.1", for the encoders that take one.
    pub level: Option<String>,
    /// What the picture is like, for the encoders with an equivalent.
    pub tune: Option<VideoTune>,
    /// Keyframes at the interval only, never at scene changes.
    pub fixed_keyframes: bool,
    /// Static HDR10 metadata to write with a PQ output: the source's,
    /// or standard defaults. Ignored for SDR and HLG outputs.
    pub hdr_metadata: Option<ffi::HdrMetadata>,
    /// Threads for the encoder; all the machine's when `None`. Set when
    /// several encoders share the machine.
    pub threads: Option<u32>,
}

/// A level string such as "4.1" as the integer code encoders use (41).
fn level_code(level: &str) -> Option<i64> {
    let mut parts = level.split('.');
    let major: i64 = parts.next()?.trim().parse().ok()?;
    let minor: i64 = parts.next().map_or(Some(0), |m| m.trim().parse().ok())?;
    Some(major * 10 + minor)
}

/// How a stitched H.264 track is written: the copied sources' parameter
/// sets go into the header next to the encoder's, which uses `sps_id`;
/// decode timestamps run `reorder` pictures ahead of display so that
/// copied and encoded pictures alike decode in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StitchSettings {
    /// The sources' `avcC` record.
    pub extradata: Vec<u8>,
    /// Parameter set id for the encoded runs.
    pub sps_id: u8,
    /// Decode-ahead in pictures needed by the copied sources.
    pub reorder: u32,
    /// Constant rate factor for the encoded runs.
    pub crf: u8,
}

/// Audio track settings.
#[derive(Debug, Clone)]
pub struct AudioSettings {
    /// Codec.
    pub codec: AudioCodec,
    /// Bitrate in kilobits per second.
    pub bitrate_kbps: u32,
    /// Sample rate in Hz of the samples pushed and of the output.
    pub sample_rate: u32,
    /// Channels written: 1 or 2. The samples pushed are always stereo;
    /// mono is their downmix.
    pub channels: u8,
    /// A loudness target for the mix, met by the mixer before the
    /// samples reach the encoder.
    pub loudness: Option<geneva_timeline::schema::Loudness>,
    /// The rumble high-pass and hum notches, applied by the mixer.
    pub hygiene: bool,
}

/// Picks the container from settings or the file extension.
pub fn container_for(path: &Path, requested: Option<Container>) -> Option<Container> {
    requested.or_else(
        || match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" => Some(Container::Mp4),
            "mov" => Some(Container::Mov),
            "mkv" => Some(Container::Mkv),
            "webm" => Some(Container::Webm),
            "mxf" => Some(Container::Mxf),
            "m4a" => Some(Container::M4a),
            "ogg" | "oga" | "opus" => Some(Container::Ogg),
            "flac" => Some(Container::Flac),
            "wav" => Some(Container::Wav),
            "mp3" => Some(Container::Mp3),
            "png" | "jpg" | "jpeg" => Some(Container::ImageSequence),
            _ => None,
        },
    )
}

/// Default codecs per container. For image sequences the video codec
/// follows the file extension (`default_image_codec`).
pub fn default_codecs(container: Container) -> (VideoCodec, AudioCodec) {
    match container {
        Container::Webm | Container::Ogg => (VideoCodec::Vp9, AudioCodec::Opus),
        Container::Mp4 | Container::Mov | Container::Mkv | Container::M4a => {
            (VideoCodec::H264, AudioCodec::Aac)
        }
        Container::Mxf => (VideoCodec::Dnxhd, AudioCodec::Pcm24),
        Container::Flac => (VideoCodec::H264, AudioCodec::Flac),
        Container::Wav => (VideoCodec::H264, AudioCodec::Pcm),
        Container::Mp3 => (VideoCodec::H264, AudioCodec::Mp3),
        Container::ImageSequence => (VideoCodec::Png, AudioCodec::Pcm),
    }
}

/// The image codec an image-sequence pattern asks for by extension.
pub fn default_image_codec(path: &Path) -> VideoCodec {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => VideoCodec::Mjpeg,
        _ => VideoCodec::Png,
    }
}

/// Muxer to select explicitly when the container's extension would map
/// to a different one; `None` lets the extension decide.
pub(super) fn muxer_name(container: Container) -> Option<&'static str> {
    match container {
        Container::M4a => Some("mp4"),
        Container::Ogg => Some("ogg"),
        Container::Mp4 | Container::Mov | Container::Mkv | Container::Webm => None,
        Container::Mxf => Some("mxf"),
        Container::Flac => Some("flac"),
        Container::Wav => Some("wav"),
        Container::Mp3 => Some("mp3"),
        Container::ImageSequence => Some("image2"),
    }
}

/// Whether a container can hold a video codec.
pub fn container_accepts_video(container: Container, codec: VideoCodec) -> bool {
    use VideoCodec::{Av1, Dnxhd, H264, H265, Mjpeg, Png, Prores, Vp9};
    match container {
        Container::Mp4 => matches!(codec, H264 | H265 | Vp9 | Av1 | Mjpeg),
        Container::Mov => true,
        Container::Mkv => !matches!(codec, Png),
        Container::Webm => matches!(codec, Vp9 | Av1),
        Container::Mxf => matches!(codec, Dnxhd | Prores | H264),
        Container::ImageSequence => matches!(codec, Png | Mjpeg),
        Container::M4a | Container::Ogg | Container::Flac | Container::Wav | Container::Mp3 => {
            false
        }
    }
}

/// Whether a container can hold an audio codec.
pub fn container_accepts_audio(container: Container, codec: AudioCodec) -> bool {
    use AudioCodec::{Aac, Ac3, Alac, Flac, Mp3, Opus, Pcm, Pcm24, Vorbis};
    match container {
        Container::Mp4 => matches!(codec, Aac | Mp3 | Alac | Ac3 | Opus | Flac),
        Container::Mov => matches!(codec, Aac | Mp3 | Alac | Ac3 | Pcm | Pcm24 | Flac),
        Container::Mkv => true,
        Container::Webm => matches!(codec, Opus | Vorbis),
        Container::Mxf | Container::Wav => matches!(codec, Pcm | Pcm24),
        Container::M4a => matches!(codec, Aac | Mp3 | Alac | Ac3),
        Container::Ogg => matches!(codec, Opus | Vorbis | Flac),
        Container::Flac => codec == Flac,
        Container::Mp3 => codec == Mp3,
        Container::ImageSequence => false,
    }
}

/// The sample rate to write for `wanted`: `wanted` itself when the codec
/// and the container take it, otherwise 48 kHz. Opus takes only its own
/// rates, AC-3 and MP3 a fixed set, AAC the standard ones, and MXF holds
/// 48 kHz audio only.
pub fn audio_sample_rate_for(codec: AudioCodec, container: Option<Container>, wanted: u32) -> u32 {
    let ok = match codec {
        AudioCodec::Opus => [8000, 12000, 16000, 24000, 48000].contains(&wanted),
        AudioCodec::Ac3 => [32000, 44100, 48000].contains(&wanted),
        AudioCodec::Mp3 => {
            [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000].contains(&wanted)
        }
        AudioCodec::Aac => [
            8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000,
        ]
        .contains(&wanted),
        AudioCodec::Pcm
        | AudioCodec::Pcm24
        | AudioCodec::Flac
        | AudioCodec::Alac
        | AudioCodec::Vorbis => (8000..=192_000).contains(&wanted),
    };
    if ok && container != Some(Container::Mxf) {
        wanted
    } else {
        48000
    }
}

/// The sample layout a codec (and profile) takes.
pub fn plane_format_for(
    codec: VideoCodec,
    profile: Option<VideoProfile>,
    hdr: bool,
) -> PlaneFormat {
    match codec {
        // H.264 through x264 is eight bits; HEVC, VP9 and AV1 carry ten
        // for an HDR output.
        VideoCodec::H265 | VideoCodec::Vp9 | VideoCodec::Av1 if hdr => PlaneFormat::Yuv420p10,
        VideoCodec::H264
        | VideoCodec::H265
        | VideoCodec::Vp9
        | VideoCodec::Av1
        | VideoCodec::Mjpeg => PlaneFormat::Yuv420p8,
        VideoCodec::Png => PlaneFormat::Rgba8,
        VideoCodec::Prores => match profile.unwrap_or(VideoProfile::Hq) {
            VideoProfile::P4444 | VideoProfile::P4444Xq => PlaneFormat::Yuv444p10,
            _ => PlaneFormat::Yuv422p10,
        },
        VideoCodec::Dnxhd => match profile.unwrap_or(VideoProfile::DnxhrHq) {
            VideoProfile::DnxhrHqx => PlaneFormat::Yuv422p10,
            VideoProfile::Dnxhr444 => PlaneFormat::Yuv444p10,
            _ => PlaneFormat::Yuv422p8,
        },
    }
}

/// The color tags an encoder actually writes: JPEG frames are full-range
/// BT.601 and PNG frames sRGB whatever the composition asked for.
pub fn output_tags_for(codec: VideoCodec, tags: ResolvedTags) -> ResolvedTags {
    match codec {
        // JPEG is full-range BT.601 by convention (JFIF).
        VideoCodec::Mjpeg => ResolvedTags {
            matrix: geneva_color::Matrix::Bt601,
            range: geneva_color::Range::Full,
            ..tags
        },
        // RGB frames are written with the sRGB curve, full range, no matrix.
        VideoCodec::Png => ResolvedTags {
            primaries: tags.primaries,
            transfer: geneva_color::Transfer::Srgb,
            matrix: geneva_color::Matrix::Identity,
            range: geneva_color::Range::Full,
        },
        _ => tags,
    }
}

/// The library's pixel format for a sample layout.
pub(super) fn pixel_of(format: PlaneFormat) -> Pixel {
    match format {
        PlaneFormat::Yuv420p8 => Pixel::YUV420P,
        PlaneFormat::Yuv420p10 => Pixel::YUV420P10LE,
        PlaneFormat::Yuv422p8 => Pixel::YUV422P,
        PlaneFormat::Yuv422p10 => Pixel::YUV422P10LE,
        PlaneFormat::Yuv444p10 => Pixel::YUV444P10LE,
        PlaneFormat::Rgba8 => Pixel::RGBA,
    }
}

/// The sample layout of a library pixel format, when it is one we pack.
pub(super) fn format_of(pixel: Pixel) -> Option<PlaneFormat> {
    match pixel {
        Pixel::YUV420P => Some(PlaneFormat::Yuv420p8),
        Pixel::YUV420P10LE => Some(PlaneFormat::Yuv420p10),
        Pixel::YUV422P => Some(PlaneFormat::Yuv422p8),
        Pixel::YUV422P10LE => Some(PlaneFormat::Yuv422p10),
        Pixel::YUV444P10LE => Some(PlaneFormat::Yuv444p10),
        Pixel::RGBA => Some(PlaneFormat::Rgba8),
        _ => None,
    }
}

/// Writes the container header; MP4-family files get their index moved
/// to the front when the file is finished if `fast_start` is set.
pub(super) fn write_header(
    octx: &mut ffmpeg_next::format::context::Output,
    container: Option<Container>,
    fast_start: bool,
    path: &Path,
) -> Result<(), MediaError> {
    let mp4_family = matches!(
        container,
        Some(Container::Mp4 | Container::Mov | Container::M4a)
    );
    if fast_start && mp4_family {
        let mut opts = Dictionary::new();
        opts.set("movflags", "+faststart");
        octx.write_header_with(opts)
            .map_err(|e| open_error(path, e))?;
    } else {
        octx.write_header().map_err(|e| open_error(path, e))?;
    }
    Ok(())
}

/// Hardware encoder implementations, most capable first.
fn hardware_encoder_names(codec: VideoCodec) -> &'static [&'static str] {
    match codec {
        VideoCodec::H264 => &["h264_videotoolbox", "h264_nvenc"],
        VideoCodec::H265 => &["hevc_videotoolbox", "hevc_nvenc"],
        _ => &[],
    }
}

/// Software encoder implementations.
fn software_encoder_names(codec: VideoCodec) -> &'static [&'static str] {
    match codec {
        VideoCodec::H264 => &["libopenh264"],
        VideoCodec::H265 => &[],
        VideoCodec::Vp9 => &["libvpx-vp9"],
        VideoCodec::Av1 => &["libsvtav1"],
        VideoCodec::Prores => &["prores_ks"],
        VideoCodec::Dnxhd => &["dnxhd"],
        VideoCodec::Png => &["png"],
        VideoCodec::Mjpeg => &["mjpeg"],
    }
}

/// Candidate encoders for a codec under a hardware policy, in the order
/// they are tried. Hardware encoders are compiled in whether or not the
/// machine has the device, so each candidate is opened to find out.
fn video_encoder_candidates(codec: VideoCodec, policy: HardwarePolicy) -> Vec<&'static str> {
    let hw = hardware_encoder_names(codec);
    let sw = software_encoder_names(codec);
    match policy {
        HardwarePolicy::Auto => hw.iter().chain(sw.iter()).copied().collect(),
        HardwarePolicy::Require if hw.is_empty() => sw.to_vec(),
        HardwarePolicy::Require => hw.to_vec(),
        HardwarePolicy::Never => sw.to_vec(),
    }
}

/// Maps the named speed presets onto SVT-AV1's numeric scale.
fn svt_preset(name: &str) -> String {
    match name {
        "ultrafast" => "12",
        "superfast" => "11",
        "veryfast" => "10",
        "faster" => "9",
        "fast" => "8",
        "medium" => "6",
        "slow" => "4",
        "slower" => "3",
        "veryslow" => "2",
        other => other,
    }
    .to_owned()
}

fn audio_encoder_names(codec: AudioCodec) -> &'static [&'static str] {
    match codec {
        AudioCodec::Aac => &["aac"],
        AudioCodec::Opus => &["libopus"],
        AudioCodec::Flac => &["flac"],
        AudioCodec::Pcm => &["pcm_s16le"],
        AudioCodec::Pcm24 => &["pcm_s24le"],
        AudioCodec::Mp3 => &["libmp3lame"],
        AudioCodec::Vorbis => &["libvorbis"],
        AudioCodec::Alac => &["alac"],
        AudioCodec::Ac3 => &["ac3"],
    }
}

/// The sample format each audio encoder takes, and whether it is a
/// bitrate-driven codec.
fn audio_sample_format(codec: AudioCodec) -> (Sample, bool) {
    match codec {
        AudioCodec::Aac | AudioCodec::Mp3 | AudioCodec::Vorbis | AudioCodec::Ac3 => {
            (Sample::F32(sample::Type::Planar), true)
        }
        AudioCodec::Opus => (Sample::F32(sample::Type::Packed), true),
        AudioCodec::Flac | AudioCodec::Pcm => (Sample::I16(sample::Type::Packed), false),
        AudioCodec::Alac => (Sample::I16(sample::Type::Planar), false),
        AudioCodec::Pcm24 => (Sample::I32(sample::Type::Packed), false),
    }
}

fn find_encoder(names: &[&str]) -> Result<ffmpeg_next::Codec, MediaError> {
    names
        .iter()
        .find_map(|n| ffmpeg_next::encoder::find_by_name(n))
        .ok_or_else(|| MediaError::MissingEncoder {
            name: names.join(" or "),
        })
}

/// Configures and opens one video encoder implementation.
fn open_video_encoder(
    vcodec: ffmpeg_next::Codec,
    settings: &VideoSettings,
    video_time_base: Rational,
    fps: Rational,
    global_header: bool,
) -> Result<(codec::encoder::video::Encoder, Vec<String>), MediaError> {
    let name = vcodec.name().to_owned();
    let mut notes = Vec::new();
    // Keyframes per interval, when one is set.
    let gop_frames = settings.keyframe_interval.map(|secs| {
        (secs * f64::from(fps.numerator()) / f64::from(fps.denominator()))
            .round()
            .max(1.0) as u32
    });
    let tune = settings.tune;
    let fixed = settings.fixed_keyframes;
    let no_tune = |notes: &mut Vec<String>| {
        if let Some(t) = tune {
            notes.push(format!(
                "tune \"{}\" is not applied: {name} has no equivalent",
                t.as_str()
            ));
        }
    };
    let no_fixed = |notes: &mut Vec<String>| {
        if fixed {
            notes.push(format!(
                "fixed keyframes are not applied: {name} also places keyframes at scene changes"
            ));
        }
    };
    let mut vctx = codec::context::Context::new_with_codec(vcodec);
    if global_header {
        vctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    // Software encoders spread work over all cores unless told a share;
    // the count is theirs to pick from the machine.
    ffi::set_threads(&mut vctx, settings.threads.unwrap_or(0));
    let mut venc = vctx
        .encoder()
        .video()
        .map_err(|e| codec_error(format!("{name}: encoder setup"), e))?;
    venc.set_width(settings.width);
    venc.set_height(settings.height);
    let format = plane_format_for(settings.codec, settings.profile, settings.color.is_hdr());
    // Hardware encoders take 10-bit 4:2:0 as P010 (chroma interleaved,
    // samples in the high bits); the planes are packed so on the way in.
    let hardware_10bit = format == PlaneFormat::Yuv420p10
        && (name.ends_with("_videotoolbox") || name.ends_with("_nvenc"));
    venc.set_format(if hardware_10bit {
        Pixel::P010LE
    } else {
        pixel_of(format)
    });
    venc.set_time_base(video_time_base);
    venc.set_frame_rate(Some(fps));
    let (space, range, primaries, transfer) = tags::to_codec_tags(settings.color);
    venc.set_colorspace(space);
    venc.set_color_range(range);
    venc.set_color_primaries(primaries);
    venc.set_color_transfer_characteristic(transfer);
    let quality = i32::from(settings.crf.unwrap_or(23).clamp(1, 51));
    let mut opts = Dictionary::new();
    match name.as_str() {
        "libopenh264" => {
            // Quality-mode rate control with the quantizer pinned gives a
            // constant quantizer; the quality level maps onto the same
            // 0..=51 scale the other encoders use.
            opts.set("rc_mode", "quality");
            opts.set("profile", "high");
            opts.set("coder", "cabac");
            venc.set_qmin(quality);
            venc.set_qmax(quality);
            venc.set_bit_rate(50_000_000);
            venc.set_gop(u32::try_from(settings.fps.round().max(1) * 2).unwrap_or(60));
            no_tune(&mut notes);
            no_fixed(&mut notes);
        }
        "libvpx-vp9" => {
            opts.set("crf", &settings.crf.unwrap_or(31).to_string());
            // libvpx takes a ceiling as the bitrate of its constrained
            // quality mode, and refuses maxrate without one; 0 is constant
            // quality with no cap.
            let cap = settings.max_bitrate_kbps.map_or(0, |k| u64::from(k) * 1000);
            opts.set("b", &cap.to_string());
            opts.set("row-mt", "1");
            if format.bits() > 8 {
                // Profile 2 carries 10-bit 4:2:0.
                opts.set("profile", "2");
            }
            match tune {
                Some(VideoTune::Film) => opts.set("tune-content", "film"),
                Some(_) => no_tune(&mut notes),
                None => {}
            }
            // Keyframes at a fixed distance: the shortest and longest the
            // same, which turns the automatic placement off.
            if let (true, Some(frames)) = (fixed, gop_frames) {
                opts.set("keyint_min", &frames.to_string());
            }
        }
        "libsvtav1" => {
            opts.set("crf", &settings.crf.unwrap_or(30).to_string());
            opts.set(
                "preset",
                &svt_preset(settings.preset.as_deref().unwrap_or("medium")),
            );
            let mut params = Vec::new();
            match tune {
                Some(VideoTune::FastDecode) => params.push("fast-decode=1"),
                Some(_) => no_tune(&mut notes),
                None => {}
            }
            if fixed {
                params.push("scd=0");
            }
            if !params.is_empty() {
                opts.set("svtav1-params", &params.join(":"));
            }
        }
        "prores_ks" => {
            opts.set(
                "profile",
                settings.profile.unwrap_or(VideoProfile::Hq).as_str(),
            );
            opts.set("vendor", "apl0");
            no_tune(&mut notes);
        }
        "dnxhd" => {
            opts.set(
                "profile",
                settings.profile.unwrap_or(VideoProfile::DnxhrHq).as_str(),
            );
            no_tune(&mut notes);
        }
        "mjpeg" => {
            // Quality 1..=31 with 1 best; the 0..=51 scale halves onto it.
            let q = (1 + i32::from(settings.crf.unwrap_or(8)) / 2).clamp(1, 31);
            venc.set_qmin(q);
            venc.set_qmax(q);
            no_tune(&mut notes);
        }
        "png" => no_tune(&mut notes),
        n if n.ends_with("_nvenc") => {
            opts.set("rc", "constqp");
            opts.set("qp", &quality.to_string());
            opts.set("preset", "p4");
            match tune {
                Some(VideoTune::ZeroLatency) => opts.set("tune", "ll"),
                Some(_) => no_tune(&mut notes),
                None => {}
            }
            if fixed {
                opts.set("no-scenecut", "1");
            }
        }
        n if n.ends_with("_videotoolbox") => {
            // VideoToolbox places keyframes at the interval only, so fixed
            // keyframes need nothing from it.
            match tune {
                Some(VideoTune::ZeroLatency) => opts.set("realtime", "1"),
                Some(_) => no_tune(&mut notes),
                None => {}
            }
            if let Some(kbps) = settings.bitrate_kbps {
                // Bitrate mode: the only way VideoToolbox holds a size.
                venc.set_bit_rate(kbps as usize * 1000);
            } else {
                // Constant quality is 1..=100 with 100 best; invert the
                // 0..=51 scale. The encoder reads it from the context's
                // global quality with the qscale flag set, the way
                // `ffmpeg -q:v` stores it (in lambda units, FF_QP2LAMBDA =
                // 118).
                let q = 1.0 - f64::from(quality) / 51.0;
                let q = ((q * 100.0).round() as i64).clamp(1, 100);
                opts.set("global_quality", &(q * 118).to_string());
                opts.set("flags", "+qscale");
            }
            if tune != Some(VideoTune::ZeroLatency) {
                opts.set("realtime", "0");
            }
        }
        _ => {
            if let Some(kbps) = settings.bitrate_kbps {
                venc.set_bit_rate(kbps as usize * 1000);
            }
            no_tune(&mut notes);
            no_fixed(&mut notes);
        }
    }
    if let Some(frames) = gop_frames {
        venc.set_gop(frames);
    }
    // A ceiling on top of constant quality is a software encoder's trick:
    // VideoToolbox given a data rate limit in quality mode writes files
    // twice the size, so it takes the ceiling only in bitrate mode.
    let ceiling_applies = name != "libvpx-vp9"
        && (!name.ends_with("_videotoolbox") || settings.bitrate_kbps.is_some());
    if let (Some(kbps), true) = (settings.max_bitrate_kbps, ceiling_applies) {
        let bps = u64::from(kbps) * 1000;
        opts.set("maxrate", &bps.to_string());
        opts.set("bufsize", &(bps * 2).to_string());
    }
    if let Some(code) = settings.level.as_deref().and_then(level_code) {
        if name.ends_with("_videotoolbox") {
            opts.set("level", &code.to_string());
        }
    }
    let venc = venc
        .open_with(opts)
        .map_err(|e| codec_error(format!("opening {name} encoder"), e))?;
    Ok((venc, notes))
}

/// Packs 10-bit 4:2:0 planes into a P010 frame: 16-bit words with the
/// ten bits at the top, chroma interleaved Cb, Cr.
fn pack_p010(planes: &Planes, out: &mut frame::Video) {
    let [y, cb, cr] = &planes.planes[..] else {
        unreachable!("4:2:0 has three planes");
    };
    let stride = out.stride(0);
    let dst = out.data_mut(0);
    for row in 0..y.height {
        let src = &y.data[row * y.stride..row * y.stride + y.width * 2];
        let line = &mut dst[row * stride..row * stride + y.width * 2];
        for (d, s) in line.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
            let v = u16::from_le_bytes([s[0], s[1]]) << 6;
            d.copy_from_slice(&v.to_le_bytes());
        }
    }
    let stride = out.stride(1);
    let dst = out.data_mut(1);
    for row in 0..cb.height {
        let b = &cb.data[row * cb.stride..row * cb.stride + cb.width * 2];
        let r = &cr.data[row * cr.stride..row * cr.stride + cr.width * 2];
        let line = &mut dst[row * stride..row * stride + cb.width * 4];
        for ((d, b), r) in line
            .chunks_exact_mut(4)
            .zip(b.chunks_exact(2))
            .zip(r.chunks_exact(2))
        {
            let cb = u16::from_le_bytes([b[0], b[1]]) << 6;
            let cr = u16::from_le_bytes([r[0], r[1]]) << 6;
            d[..2].copy_from_slice(&cb.to_le_bytes());
            d[2..].copy_from_slice(&cr.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod p010_tests {
    use super::*;

    #[test]
    fn ten_bit_planes_pack_into_p010_words_and_interleaved_chroma() {
        let mut planes = Planes::new(PlaneFormat::Yuv420p10, 4, 2);
        let put = |plane: &mut crate::convert::Plane, i: usize, v: u16| {
            plane.data[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
        };
        for i in 0..8 {
            put(&mut planes.planes[0], i, 100 + i as u16);
        }
        for i in 0..2 {
            put(&mut planes.planes[1], i, 500 + i as u16);
            put(&mut planes.planes[2], i, 700 + i as u16);
        }
        let mut out = frame::Video::new(Pixel::P010LE, 4, 2);
        pack_p010(&planes, &mut out);
        let word = |data: &[u8], i: usize| u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]);
        // Luma in the top ten bits of each word, row by row.
        assert_eq!(word(out.data(0), 0), 100 << 6);
        assert_eq!(word(&out.data(0)[out.stride(0)..], 0), 104 << 6);
        // Chroma interleaved Cb, Cr.
        assert_eq!(word(out.data(1), 0), 500 << 6);
        assert_eq!(word(out.data(1), 1), 700 << 6);
        assert_eq!(word(out.data(1), 2), 501 << 6);
        assert_eq!(word(out.data(1), 3), 701 << 6);
    }
}

/// Opens the first video encoder candidate that accepts the settings and
/// adds its stream to the output.
/// Adds an audio stream to `octx` and opens its encoder. Shared by the
/// encoding path and by a copy that mixes its own audio.
pub(super) fn open_audio_track(
    octx: &mut ffmpeg_next::format::context::Output,
    path: &Path,
    a: &AudioSettings,
    global_header: bool,
) -> Result<AudioEncoder, MediaError> {
    let acodec = find_encoder(audio_encoder_names(a.codec))?;
    let mut astream = octx.add_stream(acodec).map_err(|e| open_error(path, e))?;
    let stream_index = astream.index();
    let time_base = Rational::new(1, a.sample_rate as i32);
    astream.set_time_base(time_base);
    let mut actx = codec::context::Context::new_with_codec(acodec);
    if global_header {
        actx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    let mut aenc = actx
        .encoder()
        .audio()
        .map_err(|e| codec_error("audio encoder setup", e))?;
    let (format, bitrate_driven) = audio_sample_format(a.codec);
    let layout = if a.channels == 1 {
        ChannelLayout::MONO
    } else {
        ChannelLayout::STEREO
    };
    aenc.set_rate(a.sample_rate as i32);
    aenc.set_format(format);
    aenc.set_channel_layout(layout);
    aenc.set_time_base(time_base);
    if bitrate_driven {
        aenc.set_bit_rate(a.bitrate_kbps as usize * 1000);
    }
    let encoder = aenc
        .open()
        .map_err(|e| codec_error("opening audio encoder", e))?;
    let frame_size = match encoder.frame_size() {
        0 => 1024,
        n => n as usize,
    };
    octx.stream_mut(stream_index)
        .expect("stream added")
        .set_parameters(&encoder);
    let resampler = resampling::Context::get(
        Sample::F32(sample::Type::Packed),
        ChannelLayout::STEREO,
        a.sample_rate,
        format,
        layout,
        a.sample_rate,
    )
    .map_err(|e| codec_error("audio sample format conversion", e))?;
    Ok(AudioEncoder {
        settings: a.clone(),
        encoder,
        stream_index,
        time_base,
        stream_time_base: time_base,
        resampler,
        frame_size,
        pending: Vec::new(),
        next_pts: 0,
    })
}

fn open_video_track(
    octx: &mut ffmpeg_next::format::context::Output,
    path: &Path,
    mut settings: VideoSettings,
    global_header: bool,
) -> Result<VideoTrack, MediaError> {
    settings.color = output_tags_for(settings.codec, settings.color);
    if settings.codec == VideoCodec::Dnxhd && (settings.width < 256 || settings.height < 120) {
        return Err(MediaError::Codec {
            context: "encoder setup".to_owned(),
            reason: format!(
                "DNxHR needs a picture of at least 256×120; the output is {}×{}",
                settings.width, settings.height
            ),
        });
    }
    let format = plane_format_for(settings.codec, settings.profile, settings.color.is_hdr());
    if settings.color.is_hdr() && format.bits() < 10 {
        return Err(MediaError::Codec {
            context: "encoder setup".to_owned(),
            reason: format!(
                "an HDR output needs a ten-bit codec (h265, av1, vp9 or prores), not {:?}",
                settings.codec
            ),
        });
    }
    let fps = Rational::new(settings.fps.numer() as i32, settings.fps.denom() as i32);
    let time_base = Rational::new(fps.denominator(), fps.numerator());
    if let Some(stitch) = settings.stitch.clone() {
        return open_stitch_track(octx, path, settings, &stitch, fps, time_base, format);
    }
    let candidates = video_encoder_candidates(settings.codec, settings.hardware);
    if candidates.is_empty() {
        return Err(MediaError::MissingEncoder {
            name: format!(
                "{:?} ({:?} hardware policy)",
                settings.codec, settings.hardware
            ),
        });
    }
    let mut opened = None;
    let mut last_error = None;
    let mut x264_tried = false;
    let mut x264_error = None;
    let mut hw_skipped: Option<(String, String)> = None;
    for name in &candidates {
        // The system's x264, when installed, comes before the bundled
        // software encoder and after any hardware encoder the policy
        // allows.
        if settings.codec == VideoCodec::H264 && name.starts_with("lib") && !x264_tried {
            x264_tried = true;
            let x264 = X264Settings {
                width: settings.width,
                height: settings.height,
                fps,
                crf: settings.crf.unwrap_or(23).clamp(0, 51),
                preset: settings.preset.as_deref().unwrap_or("medium"),
                color: settings.color,
                global_header,
                keyframe_interval: settings.keyframe_interval,
                max_bitrate_kbps: settings.max_bitrate_kbps,
                level: settings.level.clone(),
                annexb: true,
                sps_id: None,
                bframes: None,
                tune: settings.tune.map(VideoTune::as_str),
                fixed_keyframes: settings.fixed_keyframes,
                threads: settings.threads,
            };
            match X264Encoder::open(&x264) {
                Ok(enc) => {
                    opened = Some((VideoBackend::X264(enc), "libx264".to_owned(), Vec::new()));
                    break;
                }
                Err(MediaError::MissingEncoder { .. }) => {}
                Err(e) => x264_error = Some(e.to_string()),
            }
        }
        let Some(vcodec) = ffmpeg_next::encoder::find_by_name(name) else {
            continue;
        };
        // A hardware encoder that is compiled in but has no device logs its
        // failure before returning an error; with a software fallback still
        // to try, that log line is noise.
        let quiet = settings.hardware == HardwarePolicy::Auto && !name.starts_with("lib");
        if quiet {
            ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Quiet);
        }
        let result = open_video_encoder(vcodec, &settings, time_base, fps, global_header);
        if quiet {
            ffmpeg_next::log::set_level(ffmpeg_next::log::Level::Error);
        }
        match result {
            Ok((enc, notes)) => {
                opened = Some((VideoBackend::Lavc(enc), (*name).to_owned(), notes));
                break;
            }
            Err(e) => {
                if !name.starts_with("lib") && hw_skipped.is_none() {
                    hw_skipped = Some(((*name).to_owned(), e.to_string()));
                }
                last_error = Some(e);
            }
        }
    }
    let (backend, name, mut notes) = match (opened, last_error) {
        (Some(v), _) => v,
        // A codec with hardware encoders only: say so, rather than the
        // last driver's complaint.
        (None, Some(e)) if software_encoder_names(settings.codec).is_empty() => {
            return Err(MediaError::Codec {
                context: "encoder setup".to_owned(),
                reason: format!(
                    "{:?} needs a hardware encoder (VideoToolbox or NVENC) and none could be opened here ({e}); for ten bits without one, use av1 or vp9",
                    settings.codec
                ),
            });
        }
        (None, Some(e)) => return Err(e),
        (None, None) => {
            return Err(MediaError::MissingEncoder {
                name: candidates.join(" or "),
            });
        }
    };
    // A hardware encoder that is compiled in but has no usable device is
    // skipped without a word. That is right on a machine that has no such
    // device, and wrong on one that does: there the run is slower than the
    // machine can go and nothing says why. So say it, but only where the
    // device looks present, or every laptop without a GPU would hear about
    // NVENC on every render.
    if let Some((hw, reason)) = hw_skipped {
        if hardware_device_present(&hw) && hw != name {
            notes.push(skipped_note(&hw, &reason, &name));
        }
    }
    let mut stream = match &backend {
        VideoBackend::Lavc(encoder) => {
            let vcodec = encoder.codec().expect("opened encoder has a codec");
            let mut stream = octx.add_stream(vcodec).map_err(|e| open_error(path, e))?;
            stream.set_parameters(encoder);
            stream
        }
        VideoBackend::X264(encoder) => {
            let ctx = ffi::h264_context(
                settings.width,
                settings.height,
                encoder.extradata(),
                tags::to_codec_tags(settings.color),
            );
            octx.add_stream_with(&ctx)
                .map_err(|e| open_error(path, e))?
        }
        VideoBackend::Stitch(_) => unreachable!("stitched tracks are opened above"),
    };
    let stream_index = stream.index();
    stream.set_time_base(time_base);
    stream.set_avg_frame_rate(fps);
    stream.set_rate(fps);
    if let Some(meta) = &settings.hdr_metadata {
        if settings.color.transfer == geneva_color::Transfer::Pq {
            ffi::attach_hdr_metadata_to_stream(&mut stream, meta);
        }
    }
    Ok(VideoTrack {
        backend,
        scratch: frame::Video::empty(),
        name,
        notes,
        x264_error,
        stream_index,
        time_base,
        settings,
        format,
    })
}

/// The x264 settings of a stitched track's encoded runs.
struct StitchX264 {
    width: u32,
    height: u32,
    fps: Rational,
    crf: u8,
    color: ResolvedTags,
    sps_id: u8,
}

impl StitchX264 {
    fn settings(&self) -> X264Settings<'static> {
        X264Settings {
            width: self.width,
            height: self.height,
            fps: self.fps,
            crf: self.crf,
            preset: "medium",
            color: self.color,
            global_header: true,
            keyframe_interval: None,
            max_bitrate_kbps: None,
            level: None,
            annexb: false,
            sps_id: Some(self.sps_id),
            bframes: Some(STITCH_BFRAMES),
            tune: None,
            fixed_keyframes: false,
            threads: None,
        }
    }
}

/// Consecutive B-frames of the encoded runs; with x264's pyramid the
/// pictures decode at most this many ahead of display.
const STITCH_BFRAMES: u8 = 3;

/// A stitched H.264 track: copied packets and encoded runs in one stream.
struct StitchTrack {
    x264: StitchX264,
    /// The encoder of the current run, opened at its first picture and
    /// flushed when the run ends.
    current: Option<X264Encoder>,
    /// Packets written so far: the decode timestamp counter.
    packets: i64,
    /// Decode-ahead in pictures.
    reorder: i64,
    build: i32,
}

/// Opens a stitched track: the header carries the sources' parameter
/// sets and those of the encoder's runs.
fn open_stitch_track(
    octx: &mut ffmpeg_next::format::context::Output,
    path: &Path,
    settings: VideoSettings,
    stitch: &StitchSettings,
    fps: Rational,
    time_base: Rational,
    format: PlaneFormat,
) -> Result<VideoTrack, MediaError> {
    if settings.codec != VideoCodec::H264 {
        return Err(MediaError::Codec {
            context: "encoder setup".to_owned(),
            reason: "a stitched track must be H.264".to_owned(),
        });
    }
    let x264 = StitchX264 {
        width: settings.width,
        height: settings.height,
        fps,
        crf: stitch.crf,
        color: settings.color,
        sps_id: stitch.sps_id,
    };
    // A throwaway encoder yields the runs' parameter sets for the header.
    let probe = X264Encoder::open(&x264.settings())?;
    let build = probe.build();
    let mut sets = h264::parse_avcc(&stitch.extradata).ok_or_else(|| MediaError::Codec {
        context: "encoder setup".to_owned(),
        reason: "the source's H.264 parameter sets could not be read".to_owned(),
    })?;
    for nal in h264::nal_units(probe.extradata(), 4) {
        match h264::nal_type(nal) {
            7 => sets.sps.push(nal.to_vec()),
            8 => sets.pps.push(nal.to_vec()),
            _ => {}
        }
    }
    drop(probe);
    let extradata = h264::build_avcc(&sets);
    let ctx = ffi::h264_context(
        settings.width,
        settings.height,
        &extradata,
        tags::to_codec_tags(settings.color),
    );
    let mut stream = octx
        .add_stream_with(&ctx)
        .map_err(|e| open_error(path, e))?;
    let stream_index = stream.index();
    stream.set_time_base(time_base);
    stream.set_avg_frame_rate(fps);
    stream.set_rate(fps);
    Ok(VideoTrack {
        backend: VideoBackend::Stitch(StitchTrack {
            x264,
            current: None,
            packets: 0,
            reorder: i64::from(stitch.reorder.max(u32::from(STITCH_BFRAMES))),
            build,
        }),
        scratch: frame::Video::empty(),
        name: "libx264".to_owned(),
        notes: Vec::new(),
        x264_error: None,
        stream_index,
        time_base,
        settings,
        format,
    })
}

/// The audio half of an [`Encoder`]. It can be detached with
/// [`Encoder::take_audio_encoder`] and run on another thread while the
/// video is encoded; its packets go back through
/// [`Encoder::write_audio_packets`].
pub struct AudioEncoder {
    settings: AudioSettings,
    encoder: codec::encoder::audio::Encoder,
    stream_index: usize,
    time_base: Rational,
    /// The muxer's time base for the stream, known once the header is
    /// written.
    stream_time_base: Rational,
    resampler: resampling::Context,
    frame_size: usize,
    /// Interleaved stereo samples waiting to fill a frame.
    pending: Vec<f32>,
    /// Sample position of the next frame to send.
    next_pts: i64,
}

impl AudioEncoder {
    /// The settings the track was opened with.
    #[must_use]
    pub fn settings(&self) -> &AudioSettings {
        &self.settings
    }

    /// Encodes interleaved stereo samples at the configured rate and
    /// returns the packets ready for the muxer. Samples that do not fill
    /// a whole encoder frame wait for the next call.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<Packet>, MediaError> {
        let chunk_len = self.frame_size * 2;
        let mut out = Vec::new();
        // Whole encoder frames go straight from the caller's slice; only the
        // remainder is buffered, so the cost stays linear in the audio length.
        let mut rest = samples;
        if !self.pending.is_empty() {
            let need = (chunk_len - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..need]);
            rest = &rest[need..];
            if self.pending.len() == chunk_len {
                let chunk = std::mem::take(&mut self.pending);
                self.send_chunk(&chunk, &mut out)?;
            }
        }
        let mut chunks = rest.chunks_exact(chunk_len);
        for chunk in &mut chunks {
            self.send_chunk(chunk, &mut out)?;
        }
        self.pending.extend_from_slice(chunks.remainder());
        Ok(out)
    }

    /// Takes the time base the muxer chose for the stream, which is
    /// known only once the header is written.
    pub(super) fn follow_muxer(&mut self, octx: &ffmpeg_next::format::context::Output) {
        self.stream_time_base = octx
            .stream(self.stream_index)
            .expect("stream added")
            .time_base();
    }

    /// Time reached by the frames sent so far.
    pub fn time(&self) -> Ratio {
        Ratio::new(self.next_pts, i64::from(self.encoder.rate()).max(1))
    }

    /// Pads the last frame, flushes the encoder and returns its final
    /// packets.
    pub fn finish(mut self) -> Result<Vec<Packet>, MediaError> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let mut chunk = std::mem::take(&mut self.pending);
            chunk.resize(self.frame_size * 2, 0.0);
            self.send_chunk(&chunk, &mut out)?;
        }
        self.encoder
            .send_eof()
            .map_err(|e| codec_error("flushing audio", e))?;
        let duration = self.frame_size as i64;
        self.collect(duration, &mut out)?;
        Ok(out)
    }

    fn send_chunk(&mut self, chunk: &[f32], out: &mut Vec<Packet>) -> Result<(), MediaError> {
        let n = chunk.len() / 2;
        let mut packed =
            frame::Audio::new(Sample::F32(sample::Type::Packed), n, ChannelLayout::STEREO);
        packed.set_rate(self.encoder.rate());
        let data = packed.data_mut(0);
        for (dst, v) in data.chunks_exact_mut(4).zip(chunk) {
            dst.copy_from_slice(&v.to_le_bytes());
        }
        let mut converted = frame::Audio::empty();
        self.resampler
            .run(&packed, &mut converted)
            .map_err(|e| codec_error("audio sample format conversion", e))?;
        converted.set_pts(Some(self.next_pts));
        self.next_pts += n as i64;
        self.encoder
            .send_frame(&converted)
            .map_err(|e| codec_error("encoding audio", e))?;
        self.collect(n as i64, out)
    }

    /// Moves the encoder's ready packets into `out`, stamped for the
    /// muxer.
    fn collect(&mut self, duration: i64, out: &mut Vec<Packet>) -> Result<(), MediaError> {
        loop {
            let mut packet = Packet::empty();
            match self.encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.set_stream(self.stream_index);
                    if packet.duration() == 0 {
                        packet.set_duration(duration);
                    }
                    packet.rescale_ts(self.time_base, self.stream_time_base);
                    out.push(packet);
                }
                Err(FfError::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
                    return Ok(());
                }
                Err(FfError::Eof) => return Ok(()),
                Err(e) => return Err(codec_error("encoding audio", e)),
            }
        }
    }
}

/// What encodes the video track.
enum VideoBackend {
    /// An encoder of the bundled libraries.
    Lavc(codec::encoder::video::Encoder),
    /// The system's x264, loaded at run time.
    X264(X264Encoder),
    /// Copied packets and x264 runs in one stream.
    Stitch(StitchTrack),
}

struct VideoTrack {
    backend: VideoBackend,
    /// The picture handed to a bundled encoder, reused from one frame to
    /// the next.
    scratch: frame::Video,
    /// Encoder implementation name, for the notes.
    name: String,
    /// Settings the encoder had no equivalent for.
    notes: Vec<String>,
    /// Why the system's x264 was found but not used, if that happened.
    x264_error: Option<String>,
    stream_index: usize,
    time_base: Rational,
    settings: VideoSettings,
    format: PlaneFormat,
}

/// Writes video and/or audio to a file.
pub struct Encoder {
    path: PathBuf,
    octx: ffmpeg_next::format::context::Output,
    video: Option<VideoTrack>,
    frame_index: i64,
    audio: Option<AudioEncoder>,
    /// Stream index and source time base of a copied audio stream.
    copied_audio: Option<(usize, Rational)>,
    subtitles: SubtitleWriter,
    image_sequence: bool,
    finished: bool,
}

impl Encoder {
    /// Creates the output file and writes its header.
    pub fn new(path: &Path, settings: EncodeSettings) -> Result<Self, MediaError> {
        init();
        if settings.video.is_none() && settings.audio.is_none() && settings.copied_audio.is_none() {
            return Err(MediaError::Codec {
                context: "encoder setup".to_owned(),
                reason: "no video or audio track requested".to_owned(),
            });
        }
        let container = container_for(path, settings.container);
        if let Some(c) = container {
            if let Some(v) = &settings.video {
                if !container_accepts_video(c, v.codec) {
                    return Err(MediaError::Codec {
                        context: "encoder setup".to_owned(),
                        reason: format!("the {c:?} container cannot hold {:?} video", v.codec),
                    });
                }
            }
            if let Some(a) = &settings.audio {
                if !container_accepts_audio(c, a.codec) {
                    return Err(MediaError::Codec {
                        context: "encoder setup".to_owned(),
                        reason: format!("the {c:?} container cannot hold {:?} audio", a.codec),
                    });
                }
            }
        }
        let mut octx = match container.and_then(muxer_name) {
            Some(name) => ffmpeg_next::format::output_as(path, name),
            None => ffmpeg_next::format::output(path),
        }
        .map_err(|e| open_error(path, e))?;
        let global_header = octx
            .format()
            .flags()
            .contains(ffmpeg_next::format::Flags::GLOBAL_HEADER);

        let video = match settings.video {
            None => None,
            Some(v) => Some(open_video_track(&mut octx, path, v, global_header)?),
        };

        // Audio stream.
        let audio = match &settings.audio {
            None => None,
            Some(a) => Some(open_audio_track(&mut octx, path, a, global_header)?),
        };

        let subtitles =
            SubtitleWriter::add_streams(&mut octx, container, &settings.subtitles, path)?;
        // A copied audio stream takes its parameters from the template.
        let copied_audio = match &settings.copied_audio {
            None => None,
            Some(template) => {
                let ictx =
                    ffmpeg_next::format::input(template).map_err(|e| open_error(template, e))?;
                let stream = ictx
                    .streams()
                    .best(ffmpeg_next::media::Type::Audio)
                    .ok_or_else(|| MediaError::NoStream {
                        path: template.clone(),
                        kind: "audio",
                    })?;
                let index = super::copy::add_copied_stream(&mut octx, &stream, path)?;
                Some((index, stream.time_base()))
            }
        };
        write_header(&mut octx, container, settings.fast_start, path)?;
        // The muxer may have chosen another time base for the stream.
        let mut audio = audio;
        if let Some(a) = audio.as_mut() {
            a.follow_muxer(&octx);
        }
        Ok(Self {
            path: path.to_owned(),
            octx,
            video,
            frame_index: 0,
            audio,
            copied_audio,
            subtitles,
            image_sequence: container == Some(Container::ImageSequence),
            finished: false,
        })
    }

    /// Encodes one frame. Frames must be pushed in order; each is shown
    /// for exactly one frame period.
    pub fn push_frame(&mut self, frame: &Frame) -> Result<(), MediaError> {
        let (color, format) = self.video_format()?;
        let planes = frame_to_planes(frame, color, format);
        self.push_planes(&planes)
    }

    /// Output color tags and sample layout of the video track, which
    /// frames converted ahead of [`push_planes`](Self::push_planes) must
    /// use.
    pub fn video_format(&self) -> Result<(ResolvedTags, PlaneFormat), MediaError> {
        self.video
            .as_ref()
            .map(|t| (t.settings.color, t.format))
            .ok_or_else(|| MediaError::Codec {
                context: "encoding video".to_owned(),
                reason: "the output has no video track".to_owned(),
            })
    }

    /// Encodes one frame already packed in the track's sample layout.
    pub fn push_planes(&mut self, planes: &Planes) -> Result<(), MediaError> {
        let Some(track) = self.video.as_mut() else {
            return Err(MediaError::Codec {
                context: "encoding video".to_owned(),
                reason: "the output has no video track".to_owned(),
            });
        };
        if planes.format != track.format {
            return Err(MediaError::Codec {
                context: "encoding video".to_owned(),
                reason: format!(
                    "frame is {} but the track takes {}",
                    planes.format.name(),
                    track.format.name()
                ),
            });
        }
        let pts = self.frame_index;
        self.frame_index += 1;
        match &mut track.backend {
            VideoBackend::Lavc(encoder) => {
                // The frame takes the encoder's own layout, which for a
                // hardware encoder fed ten bits is P010, not the planes'.
                let pixel = encoder.format();
                let out = &mut track.scratch;
                if out.format() != pixel
                    || out.width() != planes.width
                    || out.height() != planes.height
                {
                    *out = frame::Video::new(pixel, planes.width, planes.height);
                } else {
                    // The encoder may still hold the last picture; then the
                    // library gives this one fresh storage, otherwise the
                    // same storage is written again.
                    ffi::make_writable(out).map_err(|e| codec_error("encoding video", e))?;
                }
                if out.format() == Pixel::P010LE {
                    pack_p010(planes, out);
                } else {
                    for (i, plane) in planes.planes.iter().enumerate() {
                        let stride = out.stride(i);
                        let row_bytes = plane.width * planes.format.bytes_per_sample();
                        let dst = out.data_mut(i);
                        for row in 0..plane.height {
                            dst[row * stride..row * stride + row_bytes].copy_from_slice(
                                &plane.data[row * plane.stride..row * plane.stride + row_bytes],
                            );
                        }
                    }
                }
                if let Some(meta) = &track.settings.hdr_metadata {
                    if track.settings.color.transfer == geneva_color::Transfer::Pq {
                        ffi::attach_hdr_metadata_to_frame(out, meta);
                    }
                }
                let (space, range, primaries, transfer) = tags::to_codec_tags(track.settings.color);
                out.set_color_space(space);
                out.set_color_range(range);
                out.set_color_primaries(primaries);
                out.set_color_transfer_characteristic(transfer);
                out.set_pts(Some(pts));
                encoder
                    .send_frame(out)
                    .map_err(|e| codec_error("encoding video", e))?;
                Self::drain(
                    &mut self.octx,
                    encoder,
                    track.stream_index,
                    track.time_base,
                    1,
                )?;
            }
            VideoBackend::X264(encoder) => {
                if let Some(frame) = encoder.encode(planes, pts)? {
                    Self::write_x264_frame(
                        &mut self.octx,
                        track.stream_index,
                        track.time_base,
                        &frame,
                    )?;
                }
            }
            VideoBackend::Stitch(stitch) => {
                let encoder = match stitch.current.as_mut() {
                    Some(e) => e,
                    None => stitch
                        .current
                        .insert(X264Encoder::open(&stitch.x264.settings())?),
                };
                if let Some(frame) = encoder.encode(planes, pts)? {
                    let dts = stitch.packets - stitch.reorder;
                    stitch.packets += 1;
                    Self::write_stitched(
                        &mut self.octx,
                        track.stream_index,
                        track.time_base,
                        &frame.data,
                        frame.pts,
                        dts,
                        frame.keyframe,
                    )?;
                }
            }
        }
        let time = Ratio::from_int(self.frame_index) / track.settings.fps;
        self.subtitles.write_due(&mut self.octx, time)
    }

    /// Writes one copied packet of a stitched track: the picture shown at
    /// output frame `frame`. Copied packets come in their decode order;
    /// each one takes the frame slot after the previous push.
    pub fn push_copied(
        &mut self,
        data: &[u8],
        frame: i64,
        keyframe: bool,
    ) -> Result<(), MediaError> {
        let Some(track) = self.video.as_mut() else {
            return Err(MediaError::Codec {
                context: "copying video".to_owned(),
                reason: "the output has no video track".to_owned(),
            });
        };
        let VideoBackend::Stitch(stitch) = &mut track.backend else {
            return Err(MediaError::Codec {
                context: "copying video".to_owned(),
                reason: "the track is not stitched".to_owned(),
            });
        };
        if stitch.current.is_some() {
            return Err(MediaError::Codec {
                context: "copying video".to_owned(),
                reason: "an encoded run is still open".to_owned(),
            });
        }
        let dts = stitch.packets - stitch.reorder;
        stitch.packets += 1;
        self.frame_index += 1;
        Self::write_stitched(
            &mut self.octx,
            track.stream_index,
            track.time_base,
            data,
            frame,
            dts,
            keyframe,
        )?;
        let time = Ratio::from_int(self.frame_index) / track.settings.fps;
        self.subtitles.write_due(&mut self.octx, time)
    }

    /// Ends the current encoded run of a stitched track, flushing its
    /// encoder, so that copied packets can follow. A no-op otherwise.
    pub fn end_segment(&mut self) -> Result<(), MediaError> {
        let Some(track) = self.video.as_mut() else {
            return Ok(());
        };
        let VideoBackend::Stitch(stitch) = &mut track.backend else {
            return Ok(());
        };
        let Some(mut encoder) = stitch.current.take() else {
            return Ok(());
        };
        while let Some(frame) = encoder.flush()? {
            let dts = stitch.packets - stitch.reorder;
            stitch.packets += 1;
            Self::write_stitched(
                &mut self.octx,
                track.stream_index,
                track.time_base,
                &frame.data,
                frame.pts,
                dts,
                frame.keyframe,
            )?;
        }
        Ok(())
    }

    fn write_stitched(
        octx: &mut ffmpeg_next::format::context::Output,
        stream_index: usize,
        time_base: Rational,
        data: &[u8],
        pts: i64,
        dts: i64,
        keyframe: bool,
    ) -> Result<(), MediaError> {
        if dts > pts {
            return Err(MediaError::Codec {
                context: "writing packet".to_owned(),
                reason: format!("picture {pts} would decode after it is shown (dts {dts})"),
            });
        }
        let mut packet = Packet::copy(data);
        packet.set_stream(stream_index);
        packet.set_pts(Some(pts));
        packet.set_dts(Some(dts));
        packet.set_duration(1);
        if keyframe {
            packet.set_flags(codec::packet::Flags::KEY);
        }
        let stream_tb = octx
            .stream(stream_index)
            .expect("stream exists")
            .time_base();
        packet.rescale_ts(time_base, stream_tb);
        packet
            .write_interleaved(octx)
            .map_err(|e| codec_error("writing packet", e))
    }

    /// Queues interleaved stereo samples at the configured sample rate.
    pub fn push_audio(&mut self, samples: &[f32]) -> Result<(), MediaError> {
        let Some(enc) = self.audio.as_mut() else {
            return Ok(());
        };
        let packets = enc.push(samples)?;
        let time = enc.time();
        self.write_audio_packets(packets, time)
    }

    /// Detaches the audio encoder so that it can run on another thread.
    /// Afterwards [`push_audio`](Self::push_audio) does nothing and
    /// [`finish`](Self::finish) does not flush audio: the caller feeds the
    /// detached encoder and passes everything it returns, including the
    /// packets from its own `finish`, to
    /// [`write_audio_packets`](Self::write_audio_packets) before finishing.
    pub fn take_audio_encoder(&mut self) -> Option<AudioEncoder> {
        self.audio.take()
    }

    /// Writes packets from the audio encoder, interleaved with the video.
    /// `time` is the audio time they reach, which paces subtitle cues on
    /// audio-only outputs.
    pub fn write_audio_packets(
        &mut self,
        packets: Vec<Packet>,
        time: Ratio,
    ) -> Result<(), MediaError> {
        for packet in packets {
            packet
                .write_interleaved(&mut self.octx)
                .map_err(|e| codec_error("writing packet", e))?;
        }
        if self.video.is_none() {
            self.subtitles.write_due(&mut self.octx, time)?;
        }
        Ok(())
    }

    /// Writes packets of the copied audio stream, timed in the source
    /// stream's time base.
    pub fn write_copied_audio(&mut self, packets: Vec<Packet>) -> Result<(), MediaError> {
        let Some((index, source_tb)) = self.copied_audio else {
            return Err(MediaError::Codec {
                context: "copying audio".to_owned(),
                reason: "the output has no copied audio stream".to_owned(),
            });
        };
        let stream_tb = self.octx.stream(index).expect("stream exists").time_base();
        for mut packet in packets {
            packet.set_stream(index);
            packet.rescale_ts(source_tb, stream_tb);
            packet
                .write_interleaved(&mut self.octx)
                .map_err(|e| codec_error("writing copied packet", e))?;
        }
        Ok(())
    }

    /// Writes one picture from the system's x264.
    fn write_x264_frame(
        octx: &mut ffmpeg_next::format::context::Output,
        stream_index: usize,
        time_base: Rational,
        frame: &super::x264::EncodedFrame,
    ) -> Result<(), MediaError> {
        let mut packet = Packet::copy(&frame.data);
        packet.set_stream(stream_index);
        packet.set_pts(Some(frame.pts));
        packet.set_dts(Some(frame.dts));
        packet.set_duration(1);
        if frame.keyframe {
            packet.set_flags(codec::packet::Flags::KEY);
        }
        let stream_tb = octx
            .stream(stream_index)
            .expect("stream exists")
            .time_base();
        packet.rescale_ts(time_base, stream_tb);
        packet
            .write_interleaved(octx)
            .map_err(|e| codec_error("writing packet", e))
    }

    /// Lines about video settings the encoder in use had no equivalent
    /// for (a tune, fixed keyframes), each saying so.
    pub fn video_setting_notes(&self) -> Vec<String> {
        self.video
            .as_ref()
            .map(|t| t.notes.clone())
            .unwrap_or_default()
    }

    /// A line about the video encoder in use, when it is worth telling:
    /// which software H.264 encoder was picked, and that the system's
    /// x264 would be preferred when installed.
    pub fn video_encoder_note(&self) -> Option<String> {
        let track = self.video.as_ref()?;
        match &track.backend {
            VideoBackend::X264(enc) => Some(format!(
                "H.264 encoded with the system's x264 (build {})",
                enc.build()
            )),
            VideoBackend::Stitch(stitch) => Some(format!(
                "H.264 runs encoded with the system's x264 (build {}) at CRF {}",
                stitch.build, stitch.x264.crf
            )),
            VideoBackend::Lavc(_)
                if track.name.ends_with("_videotoolbox")
                    && track.settings.max_bitrate_kbps.is_some()
                    && track.settings.bitrate_kbps.is_none() =>
            {
                Some(format!(
                    "VideoToolbox encodes at constant quality; the {} kb/s ceiling is not applied by hardware encoders (a --budget switches them to bitrate mode)",
                    track.settings.max_bitrate_kbps.unwrap_or(0)
                ))
            }
            VideoBackend::Lavc(_)
                if track.name.ends_with("_nvenc") && track.settings.max_bitrate_kbps.is_some() =>
            {
                Some(format!(
                    "NVENC encodes at a constant quantizer; the {} kb/s ceiling is not applied by it",
                    track.settings.max_bitrate_kbps.unwrap_or(0)
                ))
            }
            VideoBackend::Lavc(_) if track.name == "libopenh264" => Some(match &track.x264_error {
                Some(e) => format!(
                    "H.264 encoded with the bundled OpenH264; the system's x264 was found but not used: {e}"
                ),
                None => "H.264 encoded with the bundled OpenH264; the system's x264 is used instead when its library is installed (see the README)".to_owned(),
            }),
            VideoBackend::Lavc(_) => None,
        }
    }

    fn drain(
        octx: &mut ffmpeg_next::format::context::Output,
        encoder: &mut codec::encoder::Encoder,
        stream_index: usize,
        time_base: Rational,
        duration: i64,
    ) -> Result<(), MediaError> {
        let mut packet = Packet::empty();
        loop {
            match encoder.receive_packet(&mut packet) {
                Ok(()) => {
                    packet.set_stream(stream_index);
                    if packet.duration() == 0 {
                        packet.set_duration(duration);
                    }
                    let stream_tb = octx
                        .stream(stream_index)
                        .expect("stream exists")
                        .time_base();
                    packet.rescale_ts(time_base, stream_tb);
                    packet
                        .write_interleaved(octx)
                        .map_err(|e| codec_error("writing packet", e))?;
                }
                Err(FfError::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
                    return Ok(());
                }
                Err(FfError::Eof) => return Ok(()),
                Err(e) => return Err(codec_error("encoding", e)),
            }
        }
    }

    /// Flushes the encoders and writes the trailer.
    pub fn finish(mut self) -> Result<(), MediaError> {
        if let Some(enc) = self.audio.take() {
            let time = enc.time();
            let packets = enc.finish()?;
            self.write_audio_packets(packets, time)?;
        }
        if let Some(mut track) = self.video.take() {
            match &mut track.backend {
                VideoBackend::Lavc(encoder) => {
                    encoder
                        .send_eof()
                        .map_err(|e| codec_error("flushing video", e))?;
                    Self::drain(
                        &mut self.octx,
                        encoder,
                        track.stream_index,
                        track.time_base,
                        1,
                    )?;
                }
                VideoBackend::X264(encoder) => {
                    while let Some(frame) = encoder.flush()? {
                        Self::write_x264_frame(
                            &mut self.octx,
                            track.stream_index,
                            track.time_base,
                            &frame,
                        )?;
                    }
                }
                VideoBackend::Stitch(stitch) => {
                    if let Some(mut encoder) = stitch.current.take() {
                        while let Some(frame) = encoder.flush()? {
                            let dts = stitch.packets - stitch.reorder;
                            stitch.packets += 1;
                            Self::write_stitched(
                                &mut self.octx,
                                track.stream_index,
                                track.time_base,
                                &frame.data,
                                frame.pts,
                                dts,
                                frame.keyframe,
                            )?;
                        }
                    }
                }
            }
        }
        self.subtitles.finish(&mut self.octx)?;
        self.octx
            .write_trailer()
            .map_err(|e| open_error(&self.path, e))?;
        self.finished = true;
        if self.image_sequence {
            // Opening the output created an empty file named after the
            // pattern itself; the frames went to the numbered files.
            if std::fs::metadata(&self.path).is_ok_and(|m| m.len() == 0) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
        Ok(())
    }

    /// Number of frames pushed so far.
    pub fn frames_written(&self) -> u64 {
        self.frame_index as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_rates_fall_back_where_the_codec_or_container_needs_it() {
        assert_eq!(
            audio_sample_rate_for(AudioCodec::Aac, Some(Container::Mp4), 44100),
            44100
        );
        assert_eq!(
            audio_sample_rate_for(AudioCodec::Opus, Some(Container::Webm), 44100),
            48000
        );
        assert_eq!(
            audio_sample_rate_for(AudioCodec::Opus, Some(Container::Webm), 24000),
            24000
        );
        assert_eq!(
            audio_sample_rate_for(AudioCodec::Pcm24, Some(Container::Mxf), 44100),
            48000
        );
        assert_eq!(
            audio_sample_rate_for(AudioCodec::Pcm, Some(Container::Wav), 22050),
            22050
        );
        assert_eq!(
            audio_sample_rate_for(AudioCodec::Ac3, Some(Container::Mp4), 22050),
            48000
        );
    }
}

/// Whether the device a hardware encoder needs looks present, so that a
/// failure to open it is worth reporting.
///
/// This checks what can be checked without opening anything: every Mac
/// has VideoToolbox, and an NVIDIA driver has a control device. It says
/// nothing about whether the encoder would have worked.
fn hardware_device_present(encoder: &str) -> bool {
    if encoder.contains("videotoolbox") {
        return cfg!(target_os = "macos");
    }
    if encoder.contains("nvenc") {
        return cfg!(target_os = "linux") && std::path::Path::new("/dev/nvidiactl").exists();
    }
    false
}

/// What to say when the machine has the device and the encoder for it
/// still would not open.
fn skipped_note(hardware: &str, reason: &str, used: &str) -> String {
    let reason = reason.trim_end_matches('.');
    format!(
        "{hardware} was not usable on this machine ({reason}), so the video was encoded with {used}"
    )
}

#[cfg(test)]
mod hardware_note_tests {
    use super::{hardware_device_present, skipped_note};

    /// Only the two hardware encoders geneva knows are claimed, and only
    /// on the platform that could have them. An unknown name is never
    /// claimed, so a new encoder cannot start reporting by accident.
    #[test]
    fn a_device_is_only_claimed_where_it_could_be() {
        assert_eq!(
            hardware_device_present("h264_videotoolbox"),
            cfg!(target_os = "macos")
        );
        assert!(!hardware_device_present("libx264"));
        assert!(!hardware_device_present("h264_something_else"));
    }

    /// NVENC is claimed only on Linux, and only when the driver's control
    /// device is there. This machine is one or the other, and either way
    /// the answer must agree with the file.
    #[test]
    fn nvenc_follows_the_driver_device() {
        let has = cfg!(target_os = "linux") && std::path::Path::new("/dev/nvidiactl").exists();
        assert_eq!(hardware_device_present("h264_nvenc"), has);
    }

    /// The note names the encoder that would not open, why, and what ran
    /// instead, so the reader can tell a missing driver from a busy one.
    #[test]
    fn the_note_names_all_three_things() {
        let n = skipped_note("h264_nvenc", "No capable devices found.", "libx264");
        assert!(n.contains("h264_nvenc"), "{n}");
        assert!(n.contains("No capable devices found"), "{n}");
        assert!(n.contains("libx264"), "{n}");
        assert!(!n.contains("found.)"), "the trailing stop is dropped: {n}");
    }
}
