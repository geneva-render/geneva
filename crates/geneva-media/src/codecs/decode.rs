use std::path::{Path, PathBuf};

use ffmpeg_next::Error as FfError;
use ffmpeg_next::Packet;
use ffmpeg_next::codec;
use ffmpeg_next::media::Type;
use ffmpeg_next::software::{resampling, scaling};
use ffmpeg_next::util::channel_layout::ChannelLayout;
use ffmpeg_next::util::format::{Pixel, Sample, sample};
use ffmpeg_next::util::frame;
use geneva_color::{ColorTags, Matrix, ResolvedTags};
use geneva_render::Image;
use geneva_timeline::Ratio;

use super::probe::{ratio, ts_to_secs};
use super::{codec_error, ffi, init, open_error, tags};
use crate::MediaError;
use crate::convert::{Planes16, Planes420, rgba8_into, ycbcr16_into, yuv420p8_into};
use geneva_color::hdr::HdrToSdr;

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
    /// The file's time zero, in seconds of the stream's timestamps.
    zero: Ratio,
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
        // Time zero of a file is its first video frame; an audio track
        // keeps its offset from that, so the two stay in step however
        // each stream's own timestamps start. Audio-only files start at
        // their first sample.
        let zero_of = |s: &ffmpeg_next::format::stream::Stream| {
            let start = s.start_time();
            if start == i64::MIN {
                Ratio::ZERO
            } else {
                ts_to_secs(start, s.time_base())
            }
        };
        let zero = match ictx.streams().best(Type::Video) {
            Some(video) => zero_of(&video),
            None => zero_of(&stream).max(Ratio::ZERO),
        };
        let mut ctx = codec::context::Context::from_parameters(stream.parameters())
            .map_err(|e| codec_error(format!("{}: decoder setup", path.display()), e))?;
        // A share set for this thread, else the process's cap, else all.
        let threads = match DECODER_THREADS.with(std::cell::Cell::get) {
            0 => geneva_render::limits::threads_set().map_or(0, |n| n as u32),
            n => n,
        };
        ffi::set_threads(&mut ctx, threads);
        Ok((
            Self {
                path: path.to_owned(),
                ictx,
                stream_index,
                time_base,
                zero,
                eof: false,
            },
            ctx,
        ))
    }

    /// Source time in seconds of a decoded frame.
    fn secs(&self, frame: &frame::Frame) -> Ratio {
        let ts = frame.timestamp().or(frame.pts()).unwrap_or(0);
        ts_to_secs(ts, self.time_base) - self.zero
    }

    /// Seeks so that decoding resumes at or before source time `t`.
    fn seek(&mut self, t: Ratio, decoder: &mut codec::decoder::Opened) -> Result<(), MediaError> {
        super::probe::seek_before(
            &mut self.ictx,
            self.stream_index,
            t + self.zero,
            self.time_base,
        )
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

thread_local! {
    /// Threads for decoders opened on this thread; all the machine's by
    /// default.
    static DECODER_THREADS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Limits the decoders opened on the calling thread from now on to
/// `count` threads (`0` for all the machine's), so that several
/// pipelines sharing a machine do not each take all of it.
pub fn set_decoder_threads_for_this_thread(count: u32) {
    DECODER_THREADS.with(|c| c.set(count));
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
    /// The HDR-to-SDR conversion for the stream's tags, when they are HDR.
    hdr: Option<HdrToSdr>,
    /// The frame shown for the current time range.
    current: Option<Current>,
    /// The next decoded frame after `current`.
    pending: Option<(Ratio, frame::Video)>,
    /// Source time of the last frame pulled from the decoder.
    position: Option<Ratio>,
    /// An image buffer kept for the next conversion.
    spare: Option<Image>,
    /// Rotation the file asks for, in degrees clockwise, applied to the
    /// converted frames; raw frames keep the coded orientation.
    rotation: u16,
    /// How wide a stored pixel is shown. Converted frames are stretched
    /// to `shown` by it; raw frames keep the coded size.
    aspect: crate::PixelAspect,
    /// The coded picture at its display aspect, before the rotation:
    /// the size converted frames have, rotated.
    shown: (u32, u32),
    /// A buffer for the unrotated conversion when a rotation applies.
    unrotated: Option<Image>,
    /// How many times a frame request has seeked.
    seeks: u64,
    /// Conversions that shrink as they widen, by output size, each with
    /// the frame it writes, for [`frame_at_shrunk`](Self::frame_at_shrunk).
    shrinkers: Vec<Shrinker>,
    /// Shrunk images kept for the next conversions.
    spare_shrunk: Vec<Image>,
}

/// A conversion that widens and shrinks a frame to one size, and the
/// frame it writes: the scaler refuses a frame of any other size.
struct Shrinker {
    size: (u32, u32),
    context: scaling::Context,
    out: frame::Video,
}

/// A decoded frame, converted to the compositing format on first use.
struct Current {
    /// Start of the time range the frame covers.
    from: Ratio,
    raw: frame::Video,
    image: Option<Image>,
    /// The frame converted smaller, by size.
    shrunk: Vec<([u32; 2], Image)>,
}

/// How many shrunk sizes a reader keeps a conversion and an image for:
/// the sizes one source is drawn at in a composition, and the next steps
/// of a zoom.
const SHRUNK_KEPT: usize = 8;

impl VideoReader {
    /// Opens the first video stream of `path`. `overrides` are color tags
    /// from the timeline that win over what the file declares. HDR
    /// material is tone-mapped into the SDR working space.
    pub fn open(path: &Path, overrides: ColorTags) -> Result<Self, MediaError> {
        Self::open_with(path, overrides, true)
    }

    /// As [`open`](Self::open); with `tone_map` false, HDR material keeps
    /// its range (light past reference white), for an HDR output.
    pub fn open_with(
        path: &Path,
        overrides: ColorTags,
        tone_map: bool,
    ) -> Result<Self, MediaError> {
        let (inner, ctx) = StreamDecoder::open(path, Type::Video)?;
        let mut decoder = ctx.decoder();
        // With the packets' time base known, the decoder times its frames
        // itself (a stream's own timestamps sit ahead of the encoder's
        // priming, which it drops).
        decoder.set_packet_time_base(inner.time_base);
        let decoder = decoder
            .video()
            .map_err(|e| codec_error(format!("{}: opening video decoder", path.display()), e))?;
        let stream = inner
            .ictx
            .stream(inner.stream_index)
            .expect("stream exists");
        let fps = super::probe::frame_rate(ratio(stream.avg_frame_rate()), ratio(stream.rate()))
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
        // Non-square pixels are stretched to their display aspect as the
        // frame is widened, as a browser's `<video>` shows them.
        let aspect = super::ffi::sample_aspect_ratio(&stream);
        let shown = aspect.display_size(decoder.width(), decoder.height());
        let dst = if rgb { Pixel::RGBA } else { Pixel::YUV444P16LE };
        let scaler = scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            dst,
            shown.0,
            shown.1,
            scaling::Flags::BICUBIC,
        )
        .map_err(|e| codec_error(format!("{}: pixel format conversion", path.display()), e))?;
        // HDR material is tone-mapped on the way in, from the peak its
        // metadata declares.
        let (peak, rotation) = {
            let stream = inner
                .ictx
                .stream(inner.stream_index)
                .expect("stream exists");
            (
                ffi::hdr_peak_nits(&stream.parameters()),
                ffi::display_rotation(&stream.parameters()),
            )
        };
        let hdr = if tone_map {
            HdrToSdr::new(tags, peak)
        } else {
            None
        };
        Ok(Self {
            inner,
            decoder,
            frame_duration: fps.recip(),
            tags,
            rgb,
            scaler,
            scaled: frame::Video::empty(),
            hdr,
            current: None,
            pending: None,
            position: None,
            rotation,
            aspect,
            shown,
            unrotated: None,
            spare: None,
            seeks: 0,
            shrinkers: Vec::new(),
            spare_shrunk: Vec::new(),
        })
    }

    /// How many times a frame request has seeked in the file, the first
    /// request included. Reading frames in order seeks once; a seek per
    /// frame means every frame is decoded again from a keyframe.
    pub fn seeks(&self) -> u64 {
        self.seeks
    }

    /// Pixel format of the decoded frames.
    pub fn pixel_format(&self) -> Pixel {
        self.decoder.format()
    }

    /// The resolved color tags used to interpret the stream.
    pub fn tags(&self) -> ResolvedTags {
        self.tags
    }

    /// Width in pixels of the frames [`frame_at`](Self::frame_at) returns:
    /// as displayed, stretched by the pixel aspect and after the file's
    /// rotation.
    pub fn width(&self) -> u32 {
        if self.rotation % 180 == 90 {
            self.shown.1
        } else {
            self.shown.0
        }
    }

    /// Height in pixels of the displayed frames.
    pub fn height(&self) -> u32 {
        if self.rotation % 180 == 90 {
            self.shown.0
        } else {
            self.shown.1
        }
    }

    /// The size the picture is shown at, unrounded: [`width`](Self::width)
    /// and [`height`](Self::height) before the stretched side is rounded
    /// to whole pixels (853.33×480 for 720×480 at 32:27), for placing
    /// the frame at its exact aspect.
    pub fn display_size(&self) -> (f64, f64) {
        let (w, h) = (
            f64::from(self.decoder.width()),
            f64::from(self.decoder.height()),
        );
        let w = w * f64::from(self.aspect.num) / f64::from(self.aspect.den);
        if self.rotation % 180 == 90 {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// How wide a stored pixel is shown. [`frame_at`](Self::frame_at)
    /// applies it; [`raw_frame_at`](Self::raw_frame_at) does not.
    pub fn sample_aspect_ratio(&self) -> crate::PixelAspect {
        self.aspect
    }

    /// Rotation the file asks for, in degrees clockwise (0, 90, 180 or
    /// 270). [`frame_at`](Self::frame_at) applies it;
    /// [`raw_frame_at`](Self::raw_frame_at) does not.
    pub fn rotation(&self) -> u16 {
        self.rotation
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

    /// The frame at `t` made smaller to `size` (as displayed, after the
    /// file's rotation) while it is still Y'CbCr, then converted: for a
    /// picture drawn smaller than it is, which then converts only the
    /// pixels it can show. Shrinking in Y'CbCr averages gamma-encoded
    /// values, which dims fine bright detail a little against doing it
    /// in linear light. A size keeping more than three quarters of the
    /// frame's pixels is [`frame_at`](Self::frame_at): the shrink is a
    /// resize in the scaler, which the whole 8-bit 4:2:0 frame converts
    /// without, and it pays only once enough pixels are left out
    /// (measured on 1440p footage: fetched at 0.56 of its pixels, a
    /// 1080p job took 29% less time; at 0.88, 8% more).
    pub fn frame_at_shrunk(&mut self, t: Ratio, size: [u32; 2]) -> Result<&Image, MediaError> {
        let size = [
            size[0].clamp(1, self.width()),
            size[1].clamp(1, self.height()),
        ];
        let area = |[w, h]: [u32; 2]| u64::from(w) * u64::from(h);
        if area(size) * 4 > area([self.width(), self.height()]) * 3 {
            return self.frame_at(t);
        }
        self.advance_to(t)?;
        let mut current = self.current.take().expect("advance_to leaves a frame");
        let found = current.shrunk.iter().position(|(s, _)| *s == size);
        let index = match found {
            Some(i) => i,
            None => {
                let mut image = self.spare_shrunk.pop().unwrap_or_default();
                let converted = self.convert_shrunk(&current.raw, size, &mut image);
                if let Err(e) = converted {
                    self.current = Some(current);
                    return Err(e);
                }
                current.shrunk.push((size, image));
                current.shrunk.len() - 1
            }
        };
        let current = self.current.insert(current);
        Ok(&current.shrunk[index].1)
    }

    /// The frame at `t` as 8-bit 4:2:0 planes with the tags that convert
    /// them, when that is how the stream holds it and the frame is SDR
    /// and shown unrotated: the case [`frame_at`](Self::frame_at) converts
    /// straight to linear light, which a renderer with a device of its
    /// own converts there instead. `None` for every other stream, whose
    /// frames go through [`frame_at`](Self::frame_at).
    pub fn planes_at(
        &mut self,
        t: Ratio,
    ) -> Result<Option<(Planes420<'_>, u32, u32, ResolvedTags)>, MediaError> {
        self.advance_to(t)?;
        if self.rotation != 0
            || !self.aspect.is_square()
            || self.hdr.is_some()
            || self.tags.matrix == Matrix::Identity
        {
            return Ok(None);
        }
        let raw = &self
            .current
            .as_ref()
            .expect("advance_to leaves a frame")
            .raw;
        if !matches!(raw.format(), Pixel::YUV420P | Pixel::YUVJ420P) {
            return Ok(None);
        }
        let mut tags = self.tags;
        if raw.format() == Pixel::YUVJ420P {
            tags.range = geneva_color::Range::Full;
        }
        Ok(Some((
            Planes420 {
                y: raw.data(0),
                cb: raw.data(1),
                cr: raw.data(2),
                y_stride: raw.stride(0),
                c_stride: raw.stride(1),
            },
            raw.width(),
            raw.height(),
            tags,
        )))
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
        // A frame starting a hair after `t` is the frame for `t`:
        // containers that keep milliseconds put frames up to half a
        // millisecond off their grid, and nothing is a quarter of a
        // frame away from the one before it.
        let slack = self.frame_duration / Ratio::from_int(4);
        // The current frame is still the one for `t` by that same rule,
        // so asking again for the time it was taken for (two clips of
        // one source do, every frame) neither decodes nor seeks.
        let covered = match (&self.current, &self.pending) {
            (Some(cur), Some((next, _))) => cur.from <= t + slack && t + slack < *next,
            (Some(cur), None) => {
                // With no next frame read yet, only a frame taken by the
                // slack is known to still hold: the next one is a whole
                // frame later, past `t + slack`.
                (t < cur.from && cur.from <= t + slack) || (cur.from <= t && self.inner.eof)
            }
            _ => false,
        };
        if covered {
            return Ok(());
        }
        let backwards = self
            .current
            .as_ref()
            .is_some_and(|cur| t + slack < cur.from);
        let far_ahead = self
            .position
            .is_some_and(|p| (t - p).to_f64() > FORWARD_DECODE_WINDOW_SECS);
        if self.position.is_none() || backwards || far_ahead {
            self.inner.seek(t, &mut self.decoder)?;
            self.seeks += 1;
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
            if pts <= t + slack || self.current.is_none() {
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
                    shrunk: Vec::new(),
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
            self.spare_shrunk
                .extend(cur.shrunk.into_iter().map(|(_, image)| image));
            self.spare_shrunk.truncate(SHRUNK_KEPT);
        }
    }

    /// Nominal duration of one frame.
    pub fn frame_duration(&self) -> Ratio {
        self.frame_duration
    }

    fn convert(&mut self, raw: &frame::Video, into: &mut Image) -> Result<(), MediaError> {
        if self.rotation == 0 {
            return self.convert_unrotated(raw, into);
        }
        let mut flat = self.unrotated.take().unwrap_or_default();
        self.convert_unrotated(raw, &mut flat)?;
        rotate_into(&flat, self.rotation, into);
        self.unrotated = Some(flat);
        Ok(())
    }

    fn convert_unrotated(
        &mut self,
        raw: &frame::Video,
        into: &mut Image,
    ) -> Result<(), MediaError> {
        // The common layout goes straight to linear light, without the
        // widening pass; a full-range JPEG layout keeps its range here
        // since nothing compresses it on the way.
        if matches!(raw.format(), Pixel::YUV420P | Pixel::YUVJ420P)
            && self.tags.matrix != Matrix::Identity
            && self.hdr.is_none()
            && self.aspect.is_square()
        {
            let mut tags = self.tags;
            if raw.format() == Pixel::YUVJ420P {
                tags.range = geneva_color::Range::Full;
            }
            yuv420p8_into(
                &Planes420 {
                    y: raw.data(0),
                    cb: raw.data(1),
                    cr: raw.data(2),
                    y_stride: raw.stride(0),
                    c_stride: raw.stride(1),
                },
                raw.width(),
                raw.height(),
                tags,
                into,
            );
            return Ok(());
        }
        self.scaler.run(raw, &mut self.scaled).map_err(|e| {
            codec_error(
                format!("{}: pixel format conversion", self.inner.path.display()),
                e,
            )
        })?;
        self.linearize(&self.scaled, into);
        Ok(())
    }

    /// Converts a frame widened by one of the scalers (RGBA, or 16-bit
    /// 4:4:4 Y'CbCr) to linear light.
    fn linearize(&self, scaled: &frame::Video, into: &mut Image) {
        let (w, h) = (scaled.width(), scaled.height());
        if self.rgb {
            rgba8_into(
                scaled.data(0),
                scaled.stride(0),
                w,
                h,
                self.tags.transfer,
                into,
            );
        } else {
            ycbcr16_into(
                &Planes16 {
                    y: scaled.data(0),
                    cb: scaled.data(1),
                    cr: scaled.data(2),
                    stride: scaled.stride(0),
                },
                w,
                h,
                self.tags,
                self.hdr.as_ref(),
                into,
            );
        }
    }

    /// [`convert`](Self::convert), shrinking on the way: the scaler
    /// widens to the same layouts as the full-size one, so the samples
    /// mean the same, at `size` as displayed.
    fn convert_shrunk(
        &mut self,
        raw: &frame::Video,
        size: [u32; 2],
        into: &mut Image,
    ) -> Result<(), MediaError> {
        // `size` is as displayed; the scaler works on the coded frame.
        let size = if self.rotation % 180 == 90 {
            (size[1], size[0])
        } else {
            (size[0], size[1])
        };
        let found = self.shrinkers.iter().position(|s| s.size == size);
        let index = match found {
            Some(i) => i,
            None => {
                let dst = if self.rgb {
                    Pixel::RGBA
                } else {
                    Pixel::YUV444P16LE
                };
                let context = scaling::Context::get(
                    raw.format(),
                    raw.width(),
                    raw.height(),
                    dst,
                    size.0,
                    size.1,
                    scaling::Flags::BICUBIC,
                )
                .map_err(|e| {
                    codec_error(
                        format!("{}: shrinking conversion", self.inner.path.display()),
                        e,
                    )
                })?;
                if self.shrinkers.len() >= SHRUNK_KEPT {
                    self.shrinkers.remove(0);
                }
                self.shrinkers.push(Shrinker {
                    size,
                    context,
                    out: frame::Video::empty(),
                });
                self.shrinkers.len() - 1
            }
        };
        let shrinker = &mut self.shrinkers[index];
        let mut out = std::mem::replace(&mut shrinker.out, frame::Video::empty());
        let ran = shrinker.context.run(raw, &mut out);
        let result = ran
            .map_err(|e| {
                codec_error(
                    format!("{}: shrinking conversion", self.inner.path.display()),
                    e,
                )
            })
            .map(|()| {
                if self.rotation == 0 {
                    self.linearize(&out, into);
                } else {
                    let mut flat = Image::default();
                    self.linearize(&out, &mut flat);
                    rotate_into(&flat, self.rotation, into);
                }
            });
        self.shrinkers[index].out = out;
        result
    }
}

/// Writes `src` rotated clockwise by `degrees` (90, 180 or 270) into
/// `dst`, which takes the rotated size.
fn rotate_into(src: &Image, degrees: u16, dst: &mut Image) {
    let (w, h) = (src.width as usize, src.height as usize);
    let (dw, dh) = if degrees % 180 == 90 { (h, w) } else { (w, h) };
    dst.width = dw as u32;
    dst.height = dh as u32;
    dst.pixels.clear();
    dst.pixels.reserve(dw * dh);
    match degrees {
        90 => {
            for y in 0..dh {
                for x in 0..dw {
                    dst.pixels.push(src.pixels[(h - 1 - x) * w + y]);
                }
            }
        }
        180 => dst.pixels.extend(src.pixels.iter().rev().copied()),
        270 => {
            for y in 0..dh {
                for x in 0..dw {
                    dst.pixels.push(src.pixels[x * w + (w - 1 - y)]);
                }
            }
        }
        _ => dst.pixels.extend_from_slice(&src.pixels),
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
        let mut decoder = ctx.decoder();
        // As for video: the decoder moves the first frame's time past the
        // samples it drops (AAC priming, Opus pre-skip) only when it knows
        // the packets' time base.
        decoder.set_packet_time_base(inner.time_base);
        let decoder = decoder
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

impl AudioReader {
    /// Turns the reader into a stream that reads forward from `from`,
    /// resampled to `rate`, block by block.
    pub fn into_stream(mut self, from: Ratio, rate: u32) -> Result<AudioStream, MediaError> {
        self.inner.seek(from, &mut self.decoder)?;
        Ok(AudioStream {
            inner: self.inner,
            decoder: self.decoder,
            rate,
            from,
            resampler: None,
            pending: std::collections::VecDeque::new(),
            skip: 0,
            started: false,
            finished: false,
        })
    }
}

/// An audio stream read forward from a start time in blocks of any size,
/// as interleaved stereo `f32` at one rate. The resampler runs on across
/// blocks, so their boundaries are seamless; past the end of the file the
/// blocks are silence.
pub struct AudioStream {
    inner: StreamDecoder,
    decoder: codec::decoder::Audio,
    rate: u32,
    from: Ratio,
    resampler: Option<resampling::Context>,
    /// Resampled samples not yet handed out.
    pending: std::collections::VecDeque<f32>,
    /// Frames still to drop, when the first decoded frame began before
    /// `from`.
    skip: usize,
    started: bool,
    finished: bool,
}

impl AudioStream {
    /// The next `frames` frames.
    pub fn read(&mut self, frames: usize) -> Result<Vec<f32>, MediaError> {
        let want = frames * 2;
        let path = self.inner.path.clone();
        let mut raw = frame::Audio::empty();
        while self.pending.len() < want && !self.finished {
            if !self.inner.next_frame(&mut self.decoder, &mut raw)? {
                self.finished = true;
                let mut tail = frame::Audio::empty();
                if let Some(resampler) = self.resampler.as_mut() {
                    while resampler.flush(&mut tail).is_ok_and(|d| d.is_some())
                        || tail.samples() > 0
                    {
                        let n = tail.samples();
                        if n == 0 {
                            break;
                        }
                        Self::append(&mut self.pending, &mut self.skip, &tail);
                        tail = frame::Audio::empty();
                    }
                }
                break;
            }
            if raw.channel_layout().bits() == 0 {
                raw.set_channel_layout(ChannelLayout::default(i32::from(raw.channels())));
            }
            if !self.started {
                // Where the first frame lands relative to the start: silence
                // before it, or frames of it to drop.
                let secs = self.inner.secs(&raw);
                let offset = ((secs - self.from).to_f64() * f64::from(self.rate)).round() as i64;
                if offset > 0 {
                    self.pending
                        .extend(std::iter::repeat_n(0f32, offset as usize * 2));
                } else {
                    self.skip = (-offset) as usize;
                }
                self.started = true;
            }
            let converter = match self.resampler.as_mut() {
                Some(r) => r,
                None => self.resampler.insert(
                    resampling::Context::get(
                        raw.format(),
                        raw.channel_layout(),
                        raw.rate(),
                        Sample::F32(sample::Type::Packed),
                        ChannelLayout::STEREO,
                        self.rate,
                    )
                    .map_err(|e| codec_error(format!("{}: audio resampling", path.display()), e))?,
                ),
            };
            let mut resampled = frame::Audio::empty();
            converter
                .run(&raw, &mut resampled)
                .map_err(|e| codec_error(format!("{}: audio resampling", path.display()), e))?;
            Self::append(&mut self.pending, &mut self.skip, &resampled);
        }
        let n = want.min(self.pending.len());
        let mut out: Vec<f32> = self.pending.drain(..n).collect();
        out.resize(want, 0.0);
        Ok(out)
    }

    /// Appends a resampled frame's samples, dropping `skip` frames first.
    fn append(
        pending: &mut std::collections::VecDeque<f32>,
        skip: &mut usize,
        frame: &frame::Audio,
    ) {
        let n = frame.samples();
        if n == 0 {
            return;
        }
        let data = &frame.data(0)[..n * 2 * 4];
        let mut samples = data
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let drop = (*skip).min(n);
        *skip -= drop;
        for _ in 0..drop * 2 {
            samples.next();
        }
        pending.extend(samples);
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
