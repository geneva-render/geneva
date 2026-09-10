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
use geneva_timeline::schema::{AudioCodec, Container, HardwarePolicy, VideoCodec};

use super::{codec_error, init, open_error, tags};
use crate::MediaError;
use crate::convert::{Yuv420p, frame_to_yuv420p};

/// What to write and how.
#[derive(Debug, Clone)]
pub struct EncodeSettings {
    /// Video track settings; `None` writes no video.
    pub video: Option<VideoSettings>,
    /// Container. Chosen from the output path's extension when `None`.
    pub container: Option<Container>,
    /// Audio track settings; `None` writes no audio.
    pub audio: Option<AudioSettings>,
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
            "m4a" => Some(Container::M4a),
            "ogg" | "oga" | "opus" => Some(Container::Ogg),
            "flac" => Some(Container::Flac),
            "wav" => Some(Container::Wav),
            _ => None,
        },
    )
}

/// Default codecs per container.
pub fn default_codecs(container: Container) -> (VideoCodec, AudioCodec) {
    match container {
        Container::Webm | Container::Ogg => (VideoCodec::Vp9, AudioCodec::Opus),
        Container::Mp4 | Container::Mov | Container::Mkv | Container::M4a => {
            (VideoCodec::H264, AudioCodec::Aac)
        }
        Container::Flac => (VideoCodec::H264, AudioCodec::Flac),
        Container::Wav => (VideoCodec::H264, AudioCodec::Pcm),
    }
}

/// Muxer to select explicitly when the container's extension would map
/// to a different one; `None` lets the extension decide.
pub(super) fn muxer_name(container: Container) -> Option<&'static str> {
    match container {
        Container::M4a => Some("mp4"),
        Container::Ogg => Some("ogg"),
        Container::Mp4 | Container::Mov | Container::Mkv | Container::Webm => None,
        Container::Flac => Some("flac"),
        Container::Wav => Some("wav"),
    }
}

/// Hardware encoder implementations, most capable first.
fn hardware_encoder_names(codec: VideoCodec) -> &'static [&'static str] {
    match codec {
        VideoCodec::H264 => &["h264_videotoolbox", "h264_nvenc"],
        VideoCodec::H265 => &["hevc_videotoolbox", "hevc_nvenc"],
        VideoCodec::Vp9 | VideoCodec::Av1 => &[],
    }
}

/// Software encoder implementations.
fn software_encoder_names(codec: VideoCodec) -> &'static [&'static str] {
    match codec {
        VideoCodec::H264 => &["libopenh264"],
        VideoCodec::H265 => &[],
        VideoCodec::Vp9 => &["libvpx-vp9"],
        VideoCodec::Av1 => &["libsvtav1"],
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
    vctx.set_threading(codec::threading::Config::count(0));
    let mut venc = vctx
        .encoder()
        .video()
        .map_err(|e| codec_error(format!("{name}: encoder setup"), e))?;
    venc.set_width(settings.width);
    venc.set_height(settings.height);
    venc.set_format(Pixel::YUV420P);
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
    settings: VideoSettings,
    global_header: bool,
) -> Result<VideoTrack, MediaError> {
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
}

/// Writes video and/or audio to a file.
pub struct Encoder {
    path: PathBuf,
    octx: ffmpeg_next::format::context::Output,
    video: Option<VideoTrack>,
    frame_index: i64,
    audio: Option<AudioTrack>,
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
                let format = match a.codec {
                    AudioCodec::Aac => Sample::F32(sample::Type::Planar),
                    AudioCodec::Opus => Sample::F32(sample::Type::Packed),
                    AudioCodec::Flac | AudioCodec::Pcm => Sample::I16(sample::Type::Packed),
                };
                aenc.set_rate(a.sample_rate as i32);
                aenc.set_format(format);
                aenc.set_channel_layout(ChannelLayout::STEREO);
                aenc.set_time_base(time_base);
                if matches!(a.codec, AudioCodec::Aac | AudioCodec::Opus) {
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

        octx.write_header().map_err(|e| open_error(path, e))?;
        Ok(Self {
            path: path.to_owned(),
            octx,
            video,
            frame_index: 0,
            audio,
            finished: false,
        })
    }

    /// Encodes one frame. Frames must be pushed in order; each is shown
    /// for exactly one frame period.
    pub fn push_frame(&mut self, frame: &Frame) -> Result<(), MediaError> {
        let color = self.video_color()?;
        let yuv = frame_to_yuv420p(frame, color);
        self.push_yuv420p(&yuv)
    }

    /// Output color tags of the video track, which frames converted ahead
    /// of [`push_yuv420p`](Self::push_yuv420p) must use.
    pub fn video_color(&self) -> Result<ResolvedTags, MediaError> {
        self.video
            .as_ref()
            .map(|t| t.settings.color)
            .ok_or_else(|| MediaError::Codec {
                context: "encoding video".to_owned(),
                reason: "the output has no video track".to_owned(),
            })
    }

    /// Encodes one frame already converted to the output's color tags.
    pub fn push_yuv420p(&mut self, yuv: &Yuv420p) -> Result<(), MediaError> {
        let Some(track) = self.video.as_mut() else {
            return Err(MediaError::Codec {
                context: "encoding video".to_owned(),
                reason: "the output has no video track".to_owned(),
            });
        };
        let mut out = frame::Video::new(Pixel::YUV420P, yuv.width, yuv.height);
        let strides = [out.stride(0), out.stride(1), out.stride(2)];
        copy_plane(
            out.data_mut(0),
            strides[0],
            &yuv.y,
            yuv.width as usize,
            yuv.height as usize,
        );
        let (cw, ch) = (yuv.chroma_width() as usize, yuv.chroma_height() as usize);
        copy_plane(out.data_mut(1), strides[1], &yuv.cb, cw, ch);
        copy_plane(out.data_mut(2), strides[2], &yuv.cr, cw, ch);
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
        )
    }

    /// Queues interleaved stereo samples at the configured sample rate.
    pub fn push_audio(&mut self, samples: &[f32]) -> Result<(), MediaError> {
        let Some(track) = self.audio.as_mut() else {
            return Ok(());
        };
        track.pending.extend_from_slice(samples);
        while track.pending.len() >= track.frame_size * 2 {
            let chunk: Vec<f32> = track.pending.drain(..track.frame_size * 2).collect();
            Self::send_audio_chunk(&mut self.octx, track, &chunk)?;
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
        let bytes: Vec<u8> = chunk.iter().flat_map(|v| v.to_le_bytes()).collect();
        packed.data_mut(0)[..bytes.len()].copy_from_slice(&bytes);
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
        self.octx
            .write_trailer()
            .map_err(|e| open_error(&self.path, e))?;
        self.finished = true;
        Ok(())
    }

    /// Number of frames pushed so far.
    pub fn frames_written(&self) -> u64 {
        self.frame_index as u64
    }
}

fn copy_plane(dst: &mut [u8], stride: usize, src: &[u8], width: usize, height: usize) {
    for row in 0..height {
        dst[row * stride..row * stride + width]
            .copy_from_slice(&src[row * width..(row + 1) * width]);
    }
}
