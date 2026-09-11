use std::path::{Path, PathBuf};

use ffmpeg_next::Error as FfError;
use ffmpeg_next::Packet;
use ffmpeg_next::codec;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::{resampling, scaling};
use ffmpeg_next::util::channel_layout::ChannelLayout;
use ffmpeg_next::util::format::{Pixel, Sample, sample};
use ffmpeg_next::util::frame;
use geneva_color::{ColorTags, ResolvedTags};
use geneva_render::Image;
use geneva_timeline::Ratio;

use super::probe::{ratio, ts_to_secs};
use super::{codec_error, init, open_error, tags};
use crate::MediaError;
use crate::convert::{Planes16, rgba8_into, ycbcr16_into};

/// Seeking more than this far ahead of the current position restarts from
/// the nearest keyframe instead of decoding every frame in between.
const FORWARD_DECODE_WINDOW_SECS: f64 = 2.0;

/// Reads packets of one stream and yields decoded frames, hiding the
/// send/receive dance and end-of-stream flushing.
struct StreamDecoder {
    path: PathBuf,
    ictx: ffmpeg_next::format::context::Input,
    stream_index: usize,
    time_base: ffmpeg_next::Rational,
    start_time: i64,
    eof: bool,
}

impl StreamDecoder {
    fn open(path: &Path, kind: Type) -> Result<(Self, codec::context::Context), MediaError> {
        init();
        let ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
        let stream = ictx
            .streams()
            .best(kind)
            .ok_or_else(|| MediaError::NoStream {
                path: path.to_owned(),
                kind: if kind == Type::Video {
                    "video"
                } else {
                    "audio"
                },
            })?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let start_time = stream.start_time().max(0);
        let mut ctx = codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| codec_error(format!("{}: decoder setup", path.display()), e))?;
        ctx.set_threading(codec::threading::Config::count(0));
        Ok((
            Self {
                path: path.to_owned(),
                ictx,
                stream_index,
                time_base,
                start_time,
                eof: false,
            },
            ctx,
        ))
    }

    /// Source time in seconds of a decoded frame.
    fn secs(&self, frame: &frame::Frame) -> Ratio {
        let ts = frame.timestamp().or(frame.pts()).unwrap_or(0);
        ts_to_secs(ts - self.start_time, self.time_base)
    }

    /// Seeks so that decoding resumes at or before source time `t`.
    fn seek(&mut self, t: Ratio, decoder: &mut codec::decoder::Opened) -> Result<(), MediaError> {
        let micros = (t.to_f64() * 1_000_000.0) as i64
            + ts_to_secs(self.start_time, self.time_base).to_f64() as i64 * 1_000_000;
        self.ictx
            .seek(micros, ..micros)
            .map_err(|e| codec_error(format!("{}: seeking to {t}s", self.path.display()), e))?;
        decoder.flush();
        self.eof = false;
        Ok(())
    }

    /// Pulls the next decoded frame, feeding packets as needed.
    fn next_frame(
        &mut self,
        decoder: &mut codec::decoder::Opened,
        out: &mut frame::Frame,
    ) -> Result<bool, MediaError> {
        loop {
            match decoder.receive_frame(out) {
                Ok(()) => return Ok(true),
                Err(FfError::Eof) => return Ok(false),
                Err(FfError::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {}
                Err(e) => {
                    return Err(codec_error(format!("{}: decoding", self.path.display()), e));
                }
            }
            if self.eof {
                return Ok(false);
            }
            let mut fed = false;
            let mut packet = Packet::empty();
            loop {
                match packet.read(&mut self.ictx) {
                    Ok(()) => {}
                    // A demuxer may have nothing ready yet without being at
                    // the end; only the end of the file ends the stream.
                    Err(FfError::Other { errno }) if errno == ffmpeg_next::error::EAGAIN => {
                        continue;
                    }
                    Err(FfError::Eof) => break,
                    Err(e) => {
                        return Err(codec_error(format!("{}: reading", self.path.display()), e));
                    }
                }
                if packet.stream() == self.stream_index {
                    decoder.send_packet(&packet).map_err(|e| {
                        codec_error(format!("{}: decoding", self.path.display()), e)
                    })?;
                    fed = true;
                    break;
                }
            }
            if !fed {
                self.eof = true;
                let _ = decoder.send_eof();
            }
        }
    }
}

/// Sequential access to the frames of a video stream, with random access
/// by seeking to the preceding keyframe.
pub struct VideoReader {
    inner: StreamDecoder,
    decoder: codec::decoder::Video,
    frame_duration: Ratio,
    tags: ResolvedTags,
    rgb: bool,
    scaler: scaling::Context,
    scaled: frame::Video,
    /// The frame shown for the current time range.
    current: Option<Current>,
    /// The next decoded frame after `current`.
    pending: Option<(Ratio, frame::Video)>,
    /// Source time of the last frame pulled from the decoder.
    position: Option<Ratio>,
    /// An image buffer kept for the next conversion.
    spare: Option<Image>,
}

/// A decoded frame, converted to the compositing format on first use.
struct Current {
    /// Start of the time range the frame covers.
    from: Ratio,
    raw: frame::Video,
    image: Option<Image>,
}

impl VideoReader {
    /// Opens the first video stream of `path`. `overrides` are color tags
    /// from the timeline that win over what the file declares.
    pub fn open(path: &Path, overrides: ColorTags) -> Result<Self, MediaError> {
        let (inner, ctx) = StreamDecoder::open(path, Type::Video)?;
        let decoder = ctx
            .decoder()
            .video()
            .map_err(|e| codec_error(format!("{}: opening video decoder", path.display()), e))?;
        let stream = inner
            .ictx
            .stream(inner.stream_index)
            .expect("stream exists");
        let fps = ratio(stream.avg_frame_rate())
            .or_else(|| ratio(stream.rate()))
            .unwrap_or(Ratio::from_int(25));
        let file_tags = tags::from_codec_tags(
            decoder.color_space(),
            decoder.color_range(),
            decoder.color_primaries(),
            decoder.color_transfer_characteristic(),
        );
        let rgb = is_rgb(decoder.format());
        let mut merged = ColorTags {
            primaries: overrides.primaries.or(file_tags.primaries),
            transfer: overrides.transfer.or(file_tags.transfer),
            matrix: overrides.matrix.or(file_tags.matrix),
            range: overrides.range.or(file_tags.range),
        };
        if rgb {
            merged.matrix = Some(geneva_color::Matrix::Identity);
        }
        let (mut tags, _) = geneva_color::infer(merged, decoder.width(), decoder.height());
        // The widening conversion below compresses the full-range JPEG
        // layouts to limited range on the way, so the samples it hands
        // over are limited whatever the file says.
        if matches!(
            decoder.format(),
            Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P | Pixel::YUVJ440P
        ) {
            tags.range = geneva_color::Range::Limited;
        }
        let dst = if rgb { Pixel::RGBA } else { Pixel::YUV444P16LE };
        let scaler = scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            dst,
            decoder.width(),
            decoder.height(),
            scaling::Flags::BICUBIC,
        )
        .map_err(|e| codec_error(format!("{}: pixel format conversion", path.display()), e))?;
        Ok(Self {
            inner,
            decoder,
            frame_duration: fps.recip(),
            tags,
            rgb,
            scaler,
            scaled: frame::Video::empty(),
            current: None,
            pending: None,
            position: None,
            spare: None,
        })
    }

    /// Pixel format of the decoded frames.
    pub fn pixel_format(&self) -> Pixel {
        self.decoder.format()
    }

    /// The resolved color tags used to interpret the stream.
    pub fn tags(&self) -> ResolvedTags {
        self.tags
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.decoder.width()
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.decoder.height()
    }

    /// Returns the frame displayed at source time `t`: the last frame whose
    /// presentation time is at or before `t`. Before the first frame the
    /// first frame is returned; after the last, the last.
    pub fn frame_at(&mut self, t: Ratio) -> Result<&Image, MediaError> {
        self.advance_to(t)?;
        let mut current = self.current.take().expect("advance_to leaves a frame");
        if current.image.is_none() {
            let mut image = self.spare.take().unwrap_or_default();
            self.convert(&current.raw, &mut image)?;
            current.image = Some(image);
        }
        let current = self.current.insert(current);
        Ok(current.image.as_ref().expect("converted above"))
    }

    /// Like [`frame_at`](Self::frame_at), but returns the decoded frame in
    /// the stream's own pixel format, without conversion.
    pub fn raw_frame_at(&mut self, t: Ratio) -> Result<&frame::Video, MediaError> {
        self.advance_to(t)?;
        Ok(&self
            .current
            .as_ref()
            .expect("advance_to leaves a frame")
            .raw)
    }

    /// Makes `current` the frame displayed at source time `t`.
    fn advance_to(&mut self, t: Ratio) -> Result<(), MediaError> {
        let t = t.max(Ratio::ZERO);
        let covered = match (&self.current, &self.pending) {
            (Some(cur), Some((next, _))) => cur.from <= t && t < *next,
            (Some(cur), None) => cur.from <= t && self.inner.eof,
            _ => false,
        };
        if covered {
            return Ok(());
        }
        let backwards = self.current.as_ref().is_some_and(|cur| t < cur.from);
        let far_ahead = self
            .position
            .is_some_and(|p| (t - p).to_f64() > FORWARD_DECODE_WINDOW_SECS);
        if self.position.is_none() || backwards || far_ahead {
            self.inner.seek(t, &mut self.decoder)?;
            self.retire_current();
            self.pending = None;
            self.position = None;
        }
        loop {
            let (pts, raw) = match self.pending.take() {
                Some(p) => p,
                None => {
                    let mut raw = frame::Video::empty();
                    if !self.inner.next_frame(&mut self.decoder, &mut raw)? {
                        break;
                    }
                    let pts = self.inner.secs(&raw);
                    self.position = Some(pts);
                    (pts, raw)
                }
            };
            if pts <= t || self.current.is_none() {
                // The first frame after a seek is shown even for times before
                // its timestamp, so `from` covers everything up to it.
                let from = if self.current.is_none() {
                    Ratio::ZERO.min(pts)
                } else {
                    pts
                };
                self.retire_current();
                self.current = Some(Current {
                    from,
                    raw,
                    image: None,
                });
                if pts > t {
                    break;
                }
            } else {
                self.pending = Some((pts, raw));
                break;
            }
        }
        if self.current.is_none() {
            return Err(MediaError::Codec {
                context: format!("{}: decoding", self.inner.path.display()),
                reason: "no frames could be decoded".to_owned(),
            });
        }
        Ok(())
    }

    /// Drops the current frame, keeping its image buffer for reuse.
    fn retire_current(&mut self) {
        if let Some(cur) = self.current.take() {
            if cur.image.is_some() {
                self.spare = cur.image;
            }
        }
    }

    /// Nominal duration of one frame.
    pub fn frame_duration(&self) -> Ratio {
        self.frame_duration
    }

    fn convert(&mut self, raw: &frame::Video, into: &mut Image) -> Result<(), MediaError> {
        self.scaler.run(raw, &mut self.scaled).map_err(|e| {
            codec_error(
                format!("{}: pixel format conversion", self.inner.path.display()),
                e,
            )
        })?;
        let (w, h) = (self.scaled.width(), self.scaled.height());
        if self.rgb {
            rgba8_into(
                self.scaled.data(0),
                self.scaled.stride(0),
                w,
                h,
                self.tags.transfer,
                into,
            );
        } else {
            ycbcr16_into(
                &Planes16 {
                    y: self.scaled.data(0),
                    cb: self.scaled.data(1),
                    cr: self.scaled.data(2),
                    stride: self.scaled.stride(0),
                },
                w,
                h,
                self.tags,
                into,
            );
        }
        Ok(())
    }
}

fn is_rgb(fmt: Pixel) -> bool {
    matches!(
        fmt,
        Pixel::RGB24
            | Pixel::BGR24
            | Pixel::RGBA
            | Pixel::BGRA
            | Pixel::ARGB
            | Pixel::ABGR
            | Pixel::RGB48LE
            | Pixel::RGBA64LE
            | Pixel::GBRP
            | Pixel::GBRAP
            | Pixel::GBRP10LE
            | Pixel::GBRP12LE
            | Pixel::GBRAP10LE
            | Pixel::GBRAP12LE
            | Pixel::GBRAP16LE
            | Pixel::PAL8
    )
}

/// Reads a range of an audio stream as interleaved stereo `f32` samples at
/// a chosen rate.
pub struct AudioReader {
    inner: StreamDecoder,
    decoder: codec::decoder::Audio,
}

impl AudioReader {
    /// Opens the first audio stream of `path`.
    pub fn open(path: &Path) -> Result<Self, MediaError> {
        let (inner, ctx) = StreamDecoder::open(path, Type::Audio)?;
        let decoder = ctx
            .decoder()
            .audio()
            .map_err(|e| codec_error(format!("{}: opening audio decoder", path.display()), e))?;
        Ok(Self { inner, decoder })
    }

    /// Sample rate of the stream.
    pub fn sample_rate(&self) -> u32 {
        self.decoder.rate()
    }

    /// Decodes `duration` seconds starting at source time `from`, resampled
    /// to `rate` Hz stereo. The result always has exactly
    /// `round(duration × rate)` frames; missing audio is silence.
    pub fn read(
        &mut self,
        from: Ratio,
        duration: Ratio,
        rate: u32,
    ) -> Result<Vec<f32>, MediaError> {
        let frames_wanted = (duration.to_f64() * f64::from(rate)).round().max(0.0) as usize;
        let mut out = vec![0f32; frames_wanted * 2];
        if frames_wanted == 0 {
            return Ok(out);
        }
        let path = self.inner.path.clone();
        // The converter is built from the first decoded frame rather than
        // from the decoder, and frames whose channel order is unspecified
        // (plain WAV, for one) are given the default order for their
        // channel count so every frame matches the converter's input.
        let normalize = |raw: &mut frame::Audio| {
            if raw.channel_layout().bits() == 0 {
                raw.set_channel_layout(ChannelLayout::default(i32::from(raw.channels())));
            }
        };
        let make_resampler = |raw: &frame::Audio| {
            resampling::Context::get(
                raw.format(),
                raw.channel_layout(),
                raw.rate(),
                Sample::F32(sample::Type::Packed),
                ChannelLayout::STEREO,
                rate,
            )
            .map_err(|e| codec_error(format!("{}: audio resampling", path.display()), e))
        };

        self.inner.seek(from, &mut self.decoder)?;
        let mut write_pos: Option<i64> = None;
        let mut resampler = None;
        let mut raw = frame::Audio::empty();
        loop {
            if !self.inner.next_frame(&mut self.decoder, &mut raw)? {
                break;
            }
            // A fresh output frame each time: the converter sizes it to the
            // input, and decoders such as Opus and Vorbis start with a short
            // frame that would otherwise cap every later one.
            let mut resampled = frame::Audio::empty();
            let secs = self.inner.secs(&raw);
            if write_pos.is_none() {
                write_pos = Some(((secs - from).to_f64() * f64::from(rate)).round() as i64);
            }
            normalize(&mut raw);
            let converter = match resampler.as_mut() {
                Some(r) => r,
                None => resampler.insert(make_resampler(&raw)?),
            };
            converter
                .run(&raw, &mut resampled)
                .map_err(|e| codec_error(format!("{}: audio resampling", path.display()), e))?;
            let pos = write_pos.as_mut().expect("set above");
            copy_samples(&resampled, pos, &mut out);
            if *pos >= frames_wanted as i64 {
                break;
            }
        }
        let mut tail = frame::Audio::empty();
        if let (Some(pos), Some(resampler)) = (write_pos.as_mut(), resampler.as_mut()) {
            while resampler.flush(&mut tail).is_ok_and(|d| d.is_some()) || tail.samples() > 0 {
                copy_samples(&tail, pos, &mut out);
                if tail.samples() == 0 {
                    break;
                }
                tail = frame::Audio::empty();
            }
        }
        Ok(out)
    }
}

/// Copies packed stereo f32 samples into `out` at frame position `pos`,
/// clipping to the buffer, and advances `pos`.
fn copy_samples(frame: &frame::Audio, pos: &mut i64, out: &mut [f32]) {
    let n = frame.samples();
    if n == 0 {
        return;
    }
    let data = frame.data(0);
    let samples: Vec<f32> = data[..n * 2 * 4]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let frames_out = out.len() / 2;
    for (i, pair) in samples.chunks_exact(2).enumerate() {
        let f = *pos + i as i64;
        if f >= 0 && (f as usize) < frames_out {
            out[f as usize * 2] = pair[0];
            out[f as usize * 2 + 1] = pair[1];
        }
    }
    *pos += n as i64;
}
