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
use geneva_timeline::schema::{AudioCodec, Container, HardwarePolicy, VideoCodec, VideoProfile};

use super::subtitle_streams::{SubtitleSettings, SubtitleWriter};
use super::{codec_error, ffi, init, open_error, tags};
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

/// The sample layout a codec (and profile) takes.
pub fn plane_format_for(codec: VideoCodec, profile: Option<VideoProfile>) -> PlaneFormat {
    match codec {
        VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Vp9 | VideoCodec::Av1 => {
            PlaneFormat::Yuv420p8
        }
        VideoCodec::Mjpeg => PlaneFormat::Yuv420p8,
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
        Pixel::YUV422P => Some(PlaneFormat::Yuv422p8),
        Pixel::YUV422P10LE => Some(PlaneFormat::Yuv422p10),
        Pixel::YUV444P10LE => Some(PlaneFormat::Yuv444p10),
        Pixel::RGBA => Some(PlaneFormat::Rgba8),
        _ => None,
    }
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
) -> Result<codec::encoder::video::Encoder, MediaError> {
    let name = vcodec.name().to_owned();
    let mut vctx = codec::context::Context::new_with_codec(vcodec);
    if global_header {
        vctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }
    // Software encoders spread work over all cores; the count is theirs to
    // pick from the machine.
    ffi::use_all_threads(&mut vctx);
    let mut venc = vctx
        .encoder()
        .video()
        .map_err(|e| codec_error(format!("{name}: encoder setup"), e))?;
    venc.set_width(settings.width);
    venc.set_height(settings.height);
    venc.set_format(pixel_of(plane_format_for(settings.codec, settings.profile)));
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
        }
        "libvpx-vp9" => {
            opts.set("crf", &settings.crf.unwrap_or(31).to_string());
            opts.set("b", "0");
            opts.set("row-mt", "1");
        }
        "libsvtav1" => {
            opts.set("crf", &settings.crf.unwrap_or(30).to_string());
            opts.set(
                "preset",
                &svt_preset(settings.preset.as_deref().unwrap_or("medium")),
            );
        }
        "prores_ks" => {
            opts.set(
                "profile",
                settings.profile.unwrap_or(VideoProfile::Hq).as_str(),
            );
            opts.set("vendor", "apl0");
        }
        "dnxhd" => {
            opts.set(
                "profile",
                settings.profile.unwrap_or(VideoProfile::DnxhrHq).as_str(),
            );
        }
        "mjpeg" => {
            // Quality 1..=31 with 1 best; the 0..=51 scale halves onto it.
            let q = (1 + i32::from(settings.crf.unwrap_or(8)) / 2).clamp(1, 31);
            venc.set_qmin(q);
            venc.set_qmax(q);
        }
        n if n.ends_with("_nvenc") => {
            opts.set("rc", "constqp");
            opts.set("qp", &quality.to_string());
            opts.set("preset", "p4");
        }
        n if n.ends_with("_videotoolbox") => {
            // Quality is 0..=1 with 1 best; invert the 0..=51 scale.
            let q = 1.0 - f64::from(quality) / 51.0;
            opts.set("q:v", &format!("{}", (q * 100.0).round() as i64));
            opts.set("realtime", "0");
        }
        _ => {}
    }
    venc.open_with(opts)
        .map_err(|e| codec_error(format!("opening {name} encoder"), e))
}

/// Opens the first video encoder candidate that accepts the settings and
/// adds its stream to the output.
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
    let format = plane_format_for(settings.codec, settings.profile);
    let fps = Rational::new(settings.fps.numer() as i32, settings.fps.denom() as i32);
    let time_base = Rational::new(fps.denominator(), fps.numerator());
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
    for name in &candidates {
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
            Ok(enc) => {
                opened = Some(enc);
                break;
            }
            Err(e) => last_error = Some(e),
        }
    }
    let encoder = match (opened, last_error) {
        (Some(v), _) => v,
        (None, Some(e)) => return Err(e),
        (None, None) => {
            return Err(MediaError::MissingEncoder {
                name: candidates.join(" or "),
            });
        }
    };
    let vcodec = encoder.codec().expect("opened encoder has a codec");
    let mut stream = octx.add_stream(vcodec).map_err(|e| open_error(path, e))?;
    let stream_index = stream.index();
    stream.set_time_base(time_base);
    stream.set_avg_frame_rate(fps);
    stream.set_rate(fps);
    stream.set_parameters(&encoder);
    Ok(VideoTrack {
        encoder,
        stream_index,
        time_base,
        settings,
        format,
    })
}

struct AudioTrack {
    encoder: codec::encoder::audio::Encoder,
    stream_index: usize,
    time_base: Rational,
    resampler: resampling::Context,
    frame_size: usize,
    /// Interleaved stereo samples waiting to fill a frame.
    pending: Vec<f32>,
    /// Sample position of the next frame to send.
    next_pts: i64,
}

struct VideoTrack {
    encoder: codec::encoder::video::Encoder,
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
    audio: Option<AudioTrack>,
    subtitles: SubtitleWriter,
    image_sequence: bool,
    finished: bool,
}

impl Encoder {
    /// Creates the output file and writes its header.
    pub fn new(path: &Path, settings: EncodeSettings) -> Result<Self, MediaError> {
        init();
        if settings.video.is_none() && settings.audio.is_none() {
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
            Some(a) => {
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
                aenc.set_rate(a.sample_rate as i32);
                aenc.set_format(format);
                aenc.set_channel_layout(ChannelLayout::STEREO);
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
                    ChannelLayout::STEREO,
                    a.sample_rate,
                )
                .map_err(|e| codec_error("audio sample format conversion", e))?;
                Some(AudioTrack {
                    encoder,
                    stream_index,
                    time_base,
                    resampler,
                    frame_size,
                    pending: Vec::new(),
                    next_pts: 0,
                })
            }
        };

        let subtitles =
            SubtitleWriter::add_streams(&mut octx, container, &settings.subtitles, path)?;
        octx.write_header().map_err(|e| open_error(path, e))?;
        Ok(Self {
            path: path.to_owned(),
            octx,
            video,
            frame_index: 0,
            audio,
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
        let mut out = frame::Video::new(pixel_of(track.format), planes.width, planes.height);
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
        let (space, range, primaries, transfer) = tags::to_codec_tags(track.settings.color);
        out.set_color_space(space);
        out.set_color_range(range);
        out.set_color_primaries(primaries);
        out.set_color_transfer_characteristic(transfer);
        out.set_pts(Some(self.frame_index));
        self.frame_index += 1;
        track
            .encoder
            .send_frame(&out)
            .map_err(|e| codec_error("encoding video", e))?;
        Self::drain(
            &mut self.octx,
            &mut track.encoder,
            track.stream_index,
            track.time_base,
            1,
        )?;
        let time = Ratio::from_int(self.frame_index) / track.settings.fps;
        self.subtitles.write_due(&mut self.octx, time)
    }

    /// Queues interleaved stereo samples at the configured sample rate.
    pub fn push_audio(&mut self, samples: &[f32]) -> Result<(), MediaError> {
        let Some(track) = self.audio.as_mut() else {
            return Ok(());
        };
        let chunk_len = track.frame_size * 2;
        // Whole encoder frames go straight from the caller's slice; only the
        // remainder is buffered, so the cost stays linear in the audio length.
        let mut rest = samples;
        if !track.pending.is_empty() {
            let need = (chunk_len - track.pending.len()).min(rest.len());
            track.pending.extend_from_slice(&rest[..need]);
            rest = &rest[need..];
            if track.pending.len() == chunk_len {
                let chunk = std::mem::take(&mut track.pending);
                Self::send_audio_chunk(&mut self.octx, track, &chunk)?;
            }
        }
        let mut chunks = rest.chunks_exact(chunk_len);
        for chunk in &mut chunks {
            Self::send_audio_chunk(&mut self.octx, track, chunk)?;
        }
        track.pending.extend_from_slice(chunks.remainder());
        if self.video.is_none() {
            let time = Ratio::new(track.next_pts, i64::from(track.encoder.rate()).max(1));
            self.subtitles.write_due(&mut self.octx, time)?;
        }
        Ok(())
    }

    fn send_audio_chunk(
        octx: &mut ffmpeg_next::format::context::Output,
        track: &mut AudioTrack,
        chunk: &[f32],
    ) -> Result<(), MediaError> {
        let n = chunk.len() / 2;
        let mut packed =
            frame::Audio::new(Sample::F32(sample::Type::Packed), n, ChannelLayout::STEREO);
        packed.set_rate(track.encoder.rate());
        let data = packed.data_mut(0);
        for (dst, v) in data.chunks_exact_mut(4).zip(chunk) {
            dst.copy_from_slice(&v.to_le_bytes());
        }
        let mut converted = frame::Audio::empty();
        track
            .resampler
            .run(&packed, &mut converted)
            .map_err(|e| codec_error("audio sample format conversion", e))?;
        converted.set_pts(Some(track.next_pts));
        track.next_pts += n as i64;
        track
            .encoder
            .send_frame(&converted)
            .map_err(|e| codec_error("encoding audio", e))?;
        Self::drain(
            octx,
            &mut track.encoder,
            track.stream_index,
            track.time_base,
            n as i64,
        )
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
        if let Some(mut track) = self.audio.take() {
            if !track.pending.is_empty() {
                let mut chunk = std::mem::take(&mut track.pending);
                chunk.resize(track.frame_size * 2, 0.0);
                Self::send_audio_chunk(&mut self.octx, &mut track, &chunk)?;
            }
            track
                .encoder
                .send_eof()
                .map_err(|e| codec_error("flushing audio", e))?;
            Self::drain(
                &mut self.octx,
                &mut track.encoder,
                track.stream_index,
                track.time_base,
                track.frame_size as i64,
            )?;
        }
        if let Some(mut track) = self.video.take() {
            track
                .encoder
                .send_eof()
                .map_err(|e| codec_error("flushing video", e))?;
            Self::drain(
                &mut self.octx,
                &mut track.encoder,
                track.stream_index,
                track.time_base,
                1,
            )?;
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
