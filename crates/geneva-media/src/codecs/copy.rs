//! Stream copying: writing a file from the coded packets of others,
//! without decoding, for cuts and joins that need no re-encoding.
//!
//! Cuts can only happen at keyframes of the source, so the copied range is
//! widened to the keyframe at or before the requested start; the report
//! says exactly which times were used.

use std::path::{Path, PathBuf};

use ffmpeg_next::codec;
use ffmpeg_next::media::Type;
use ffmpeg_next::{Packet, Rational};
use geneva_color::ResolvedTags;
use geneva_timeline::schema::{Container, Fit, VideoCodec};
use geneva_timeline::{Composition, Ratio, ResolvedSource};

use super::decode::VideoReader;
use super::ffi;
use super::probe::ts_to_secs;
use super::subtitle_streams::{SubtitleSettings, SubtitleWriter};
use super::{init, open_error};
use crate::MediaError;

/// One piece of an input file to copy.
#[derive(Debug, Clone, PartialEq)]
pub struct CopySegment {
    /// Source file.
    pub path: PathBuf,
    /// Requested start in the source, in seconds.
    pub from: Ratio,
    /// Requested end in the source, in seconds, if bounded.
    pub to: Option<Ratio>,
}

/// A decision that the composition can be produced by copying packets.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyPlan {
    /// Video segments in output order; empty for audio-only outputs.
    pub segments: Vec<CopySegment>,
    /// Audio segments in output order; empty when the output has no audio.
    /// They may come from other files than the video.
    pub audio: Vec<CopySegment>,
    /// Why copying is possible, for the user.
    pub reason: String,
}

/// Requested start, actual start (a keyframe) and actual end of one
/// copied segment, in source seconds.
pub type SegmentReport = (Ratio, Ratio, Ratio);

/// What a copy actually did.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyReport {
    /// One entry per copied segment.
    pub segments: Vec<SegmentReport>,
    /// Duration of the output in seconds.
    pub duration: Ratio,
    /// Number of video packets written.
    pub video_packets: u64,
}

impl CopyReport {
    /// Human-readable notes about cuts that moved to keyframes.
    pub fn notes(&self) -> Vec<String> {
        self.segments
            .iter()
            .filter(|(req, actual, _)| req != actual)
            .map(|(req, actual, _)| format!("cut at {req}s moved to the keyframe at {actual}s"))
            .collect()
    }
}

/// Decides whether `comp` can be written by copying packets from its
/// sources, and returns the plan when it can.
///
/// Copying applies when the visual part is one layer of video clips shown
/// as they are (natural size at the frame center, full opacity, no
/// rotation, no transitions, nothing else on top) with output size, rate
/// and codec matching the sources and compatible coded parameters across
/// them; and the audio is absent, the video clips' own audio, or a single
/// untouched audio track that spans the output. Audio-only outputs copy
/// when the audio track is one untouched clip and the source codec fits
/// the container.
pub fn plan_stream_copy(
    comp: &Composition,
    root: &Path,
    container: Container,
    requested_codec: Option<VideoCodec>,
) -> Result<Option<CopyPlan>, MediaError> {
    Ok(plan_stream_copy_explained(comp, root, container, requested_codec)?.ok())
}

/// Why the sources' streams cannot be copied into the output, when the
/// composition itself shows them as they are: a human-readable sentence
/// for the render's notes. `None` when the composition changes the
/// picture or the sound, which needs no explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyRefusal(pub Option<String>);

impl CopyRefusal {
    fn because(reason: impl Into<String>) -> Self {
        Self(Some(reason.into()))
    }
}

/// A color encoding in a few words, for a note.
fn describe_tags(t: ResolvedTags) -> String {
    let kind = if t.is_hdr() { "HDR" } else { "SDR" };
    format!(
        "{kind} ({} {}, {} matrix, {} range)",
        lowercase(t.transfer),
        lowercase(t.primaries),
        lowercase(t.matrix),
        lowercase(t.range)
    )
}

/// The JSON spelling of a container or codec name, for a note.
fn lowercase(v: impl std::fmt::Debug) -> String {
    format!("{v:?}").to_lowercase()
}

/// [`plan_stream_copy`], saying why when the streams cannot be copied.
pub fn plan_stream_copy_explained(
    comp: &Composition,
    root: &Path,
    container: Container,
    requested_codec: Option<VideoCodec>,
) -> Result<Result<CopyPlan, CopyRefusal>, MediaError> {
    init();
    let refuse = |reason: String| Ok(Err(CopyRefusal::because(reason)));
    if comp.audio.len() > 1 {
        return Ok(Err(CopyRefusal(None)));
    }
    // A loudness target, hygiene, or another audio codec, bitrate, rate
    // or channel count change the sound, which only the mix can do. That
    // says nothing about the picture: the plan still describes what can
    // be copied, and a caller that changes the audio drops `audio` and
    // encodes its own with `stream_copy_mixing_audio`.
    let Some(untouched) = untouched_video_clips(comp, root)? else {
        return Ok(Err(CopyRefusal(None)));
    };
    if !untouched.is_empty() && container.is_audio_only() {
        return Ok(Err(CopyRefusal(None)));
    }
    let mut video_segments = Vec::new();
    let mut own_audio: Option<bool> = None;
    let mut reference: Option<(StreamShape, PathBuf)> = None;
    for clip in &untouched {
        let shape = StreamShape::read(&clip.path)?;
        let takes_audio = clip.audio && shape.has_audio;
        match own_audio {
            None => own_audio = Some(takes_audio),
            Some(a) if a != takes_audio => {
                return refuse(format!(
                    "{} {} audio and the others do not, so they cannot be joined as they are",
                    clip.path.file_name().map_or_else(
                        || clip.path.display().to_string(),
                        |n| n.to_string_lossy().into_owned()
                    ),
                    if takes_audio { "has" } else { "has no" }
                ));
            }
            _ => {}
        }
        let name = clip.path.file_name().map_or_else(
            || clip.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let coded = shape.codec_id.name();
        let holder = lowercase(container);
        if let Some(code) = requested_codec {
            if shape.codec != Some(code) {
                return refuse(format!(
                    "{name} is {coded}, and the output asks for {}",
                    lowercase(code)
                ));
            }
        }
        if !container_accepts(container, shape.codec) {
            return refuse(format!("{name} is {coded}, which {holder} cannot hold"));
        }
        // A silent source is copied without audio; the clip's `audio`
        // only says to take what the file has.
        if clip.audio && shape.has_audio {
            if let Some(id) = shape.audio_id {
                if !container_accepts_audio(container, id) {
                    return refuse(format!(
                        "the audio of {name} is {}, which {holder} cannot hold",
                        id.name()
                    ));
                }
            }
        }
        // The copied pictures keep their color encoding, so it must be
        // the output's: an HDR source into an SDR output is tone-mapped,
        // not copied.
        let overrides = comp
            .assets
            .get(&clip.asset)
            .map(|a| a.color)
            .unwrap_or_default();
        let source_tags = VideoReader::open(&clip.path, overrides)?.tags();
        if source_tags != comp.color {
            let hint = if source_tags.is_hdr() == comp.color.is_hdr() {
                "; set output.color to the source's tags to copy it"
            } else {
                ""
            };
            return refuse(format!(
                "{name} is {}, and the output is {}{hint}",
                describe_tags(source_tags),
                describe_tags(comp.color)
            ));
        }
        match &reference {
            None => reference = Some((shape, clip.path.clone())),
            Some((r, first)) if !r.compatible(&shape) => {
                let first = first.file_name().map_or_else(
                    || first.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                return refuse(format!(
                    "{first} and {name} differ in {}, so they cannot be joined as they are",
                    r.difference(&shape)
                ));
            }
            _ => {}
        }
        video_segments.push(CopySegment {
            path: clip.path.clone(),
            from: clip.in_,
            to: Some(clip.in_ + (clip.end - clip.start)),
        });
    }

    // Audio: the clips' own audio, one untouched separate track, or none.
    let mut audio_segments = Vec::new();
    match (own_audio, comp.audio.first()) {
        (Some(true), None) => {
            audio_segments.clone_from(&video_segments);
        }
        (Some(true), Some(_)) => return Ok(Err(CopyRefusal(None))),
        (own, Some(track)) => {
            if own == Some(true) || track.clips.len() != 1 {
                return Ok(Err(CopyRefusal(None)));
            }
            let clip = &track.clips[0];
            let untouched = clip.gain_db.is_constant()
                && clip.speed == Ratio::ONE
                && clip.gain_db.sample(0.0) == 0.0
                && clip.fade_in.is_zero()
                && clip.fade_out.is_zero()
                && clip.start.is_zero()
                && about_equal(clip.end, comp.duration);
            if !untouched {
                return Ok(Err(CopyRefusal(None)));
            }
            let Some(asset_info) = comp.assets.get(&clip.asset) else {
                return Ok(Err(CopyRefusal(None)));
            };
            let path = root.join(&asset_info.src);
            let audio_shape = AudioShape::read(&path)?;
            if !container_accepts_audio(container, audio_shape.codec_id) {
                return refuse(format!(
                    "the audio of {} is {}, which {} cannot hold",
                    path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned()
                    ),
                    audio_shape.codec_id.name(),
                    lowercase(container)
                ));
            }
            audio_segments.push(CopySegment {
                path,
                from: clip.in_,
                to: Some(clip.in_ + (clip.end - clip.start)),
            });
        }
        (Some(false) | None, None) => {}
    }
    if video_segments.is_empty() && audio_segments.is_empty() {
        return Ok(Err(CopyRefusal(None)));
    }
    let reason = match (video_segments.len(), audio_segments.is_empty()) {
        (0, _) => "the audio stream is used as is, so it is copied without re-encoding".to_owned(),
        (1, _) => "the video stream is used as is, so it is copied without re-encoding".to_owned(),
        (n, _) => format!(
            "all {n} sources share the same stream parameters, so they are joined without re-encoding"
        ),
    };
    Ok(Ok(CopyPlan {
        segments: video_segments,
        audio: audio_segments,
        reason,
    }))
}

/// A video clip shown exactly as decoded: natural size at the frame
/// center, full opacity, no rotation or transition, normal blending, in a
/// composition whose output size and rate match the source's. When scaling
/// is allowed the clip may instead be fitted to the whole frame.
pub(super) struct UntouchedClip {
    pub path: PathBuf,
    pub asset: String,
    pub start: Ratio,
    pub end: Ratio,
    pub in_: Ratio,
    pub audio: bool,
    /// Source picture size.
    pub width: u32,
    pub height: u32,
    /// How the picture is placed when it is not simply scaled to the
    /// frame; `None` when it covers the frame.
    pub place: Option<Place>,
}

/// A picture's place in the frame when scaling alone does not give it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    /// The picture sits at `[x, y, width, height]` in the frame, the rest
    /// being the composition's background (a `contain` fit with bars).
    Bars([u32; 4]),
    /// The region `[x, y, width, height]` of the source is scaled to the
    /// whole frame (a `cover` fit, cropping the edges).
    Crop([u32; 4]),
    /// The region `src` of the source is scaled to `dst` in the frame,
    /// the rest being the background (a cropped clip with bars).
    CropBars { src: [u32; 4], dst: [u32; 4] },
}

/// The composition's single video layer as a contiguous list of untouched
/// clips covering the whole output, or `None` when anything would change
/// the picture. An empty list means the composition has no video layer.
pub(super) fn untouched_video_clips(
    comp: &Composition,
    root: &Path,
) -> Result<Option<Vec<UntouchedClip>>, MediaError> {
    video_layer_clips(comp, root, false, false)
}

/// Like [`untouched_video_clips`], but also accepts clips that are only
/// scaled: fitted so that the picture fills the whole output frame (to
/// within the rounding of an even output size), with nothing else changed.
pub(super) fn filling_video_clips(
    comp: &Composition,
    root: &Path,
) -> Result<Option<Vec<UntouchedClip>>, MediaError> {
    video_layer_clips(comp, root, true, false)
}

/// Like [`filling_video_clips`], for a composition whose further layers
/// hold overlays that composite normally: the first layer's clips are
/// returned and the layers above are left to the caller.
pub(super) fn base_video_clips(
    comp: &Composition,
    root: &Path,
) -> Result<Option<Vec<UntouchedClip>>, MediaError> {
    video_layer_clips(comp, root, true, true)
}

fn video_layer_clips(
    comp: &Composition,
    root: &Path,
    allow_scale: bool,
    allow_overlays: bool,
) -> Result<Option<Vec<UntouchedClip>>, MediaError> {
    if comp.layers.len() > 1 {
        let plain = comp
            .layers
            .iter()
            .skip(1)
            .flat_map(|l| l.clips.iter())
            .all(|c| c.blend == geneva_timeline::schema::BlendMode::Normal && c.effects.is_empty());
        if !allow_overlays || !plain {
            return Ok(None);
        }
    }
    let Some(layer) = comp.layers.first() else {
        return Ok(Some(Vec::new()));
    };
    let mut clips = Vec::new();
    let mut own_audio: Option<bool> = None;
    let mut expected_end = Ratio::ZERO;
    let out_w = f64::from(comp.width);
    let out_h = f64::from(comp.height);
    let center = [out_w / 2.0, out_h / 2.0];
    for clip in &layer.clips {
        let ResolvedSource::Video { asset, in_, audio } = &clip.source else {
            return Ok(None);
        };
        if clip.start != expected_end
            || clip.transition_in.is_some()
            || clip.transition_out.is_some()
        {
            return Ok(None);
        }
        if !clip.opacity.is_constant() || clip.opacity.sample(0.0) < 1.0 {
            return Ok(None);
        }
        if !clip.scale.is_constant() || clip.scale.sample(0.0) != [1.0, 1.0] {
            return Ok(None);
        }
        if !clip.rotation.is_constant() || clip.rotation.sample(0.0) != 0.0 {
            return Ok(None);
        }
        if clip.blend != geneva_timeline::schema::BlendMode::Normal
            || !clip.effects.is_empty()
            || clip.mask.is_some()
            || clip.speed != Ratio::ONE
        {
            return Ok(None);
        }
        if !clip.position.is_constant() || clip.position.sample(0.0) != center {
            return Ok(None);
        }
        if clip.anchor.to_px(out_w, out_h) != center {
            return Ok(None);
        }
        match own_audio {
            None => own_audio = Some(*audio),
            Some(a) if a != *audio => return Ok(None),
            _ => {}
        }
        let Some(asset_info) = comp.assets.get(asset) else {
            return Ok(None);
        };
        let path = root.join(&asset_info.src);
        let shape = StreamShape::read(&path)?;
        if shape.fps != comp.fps {
            return Ok(None);
        }
        // A crop shows one region of the source, rounded to even pixels
        // so that subsampled chroma lines up; it is a scaling.
        let region = match clip.crop {
            None => None,
            Some(crop) => {
                if !allow_scale {
                    return Ok(None);
                }
                let Some([x, y, w, h]) =
                    crop.to_px(f64::from(shape.width), f64::from(shape.height))
                else {
                    return Ok(None);
                };
                let even_floor = |v: f64| ((v / 2.0).floor() * 2.0) as u32;
                let even = |v: f64| ((v / 2.0).round() * 2.0) as u32;
                let (rx, ry) = (even_floor(x), even_floor(y));
                let (rw, rh) = (even(w).max(2), even(h).max(2));
                if rx + rw > shape.width || ry + rh > shape.height {
                    return Ok(None);
                }
                if [rx, ry, rw, rh] == [0, 0, shape.width, shape.height] {
                    None
                } else {
                    Some([rx, ry, rw, rh])
                }
            }
        };
        let (pic_w, pic_h) = region.map_or((shape.width, shape.height), |r| (r[2], r[3]));
        let same_size = region.is_none() && pic_w == comp.width && pic_h == comp.height;
        let fills = allow_scale && fills_frame(clip.fit, pic_w, pic_h, comp);
        let place = if same_size || !allow_scale {
            None
        } else if fills {
            region.map(Place::Crop)
        } else {
            placement(clip.fit, region, shape.width, shape.height, comp)
        };
        if !(same_size || fills || place.is_some()) {
            return Ok(None);
        }
        clips.push(UntouchedClip {
            path,
            asset: asset.clone(),
            start: clip.start,
            end: clip.end,
            in_: *in_,
            audio: *audio,
            width: shape.width,
            height: shape.height,
            place,
        });
        expected_end = clip.end;
    }
    if clips.is_empty() || !about_equal(expected_end, comp.duration) {
        return Ok(None);
    }
    Ok(Some(clips))
}

/// Whether two times agree to within a millisecond: the slack that a
/// duration printed with six decimals leaves against the exact length of
/// a clip at a fractional frame rate (2123 frames at 29.97 fps last
/// 70.837433… s), where copying the clip whole is still what was asked.
fn about_equal(a: Ratio, b: Ratio) -> bool {
    let diff = if a > b { a - b } else { b - a };
    diff <= geneva_timeline::END_SLACK
}

/// Whether a `w`×`h` picture fitted into the output covers the whole frame
/// to within two pixels on each axis: the slack that rounding an output
/// size to even numbers leaves (854×480 at height 360 is 640.5 wide,
/// written as 642), where the renderer would show at most a one-pixel
/// edge and a scaler stretches by a fraction of a percent instead.
fn fills_frame(fit: Fit, w: u32, h: u32, comp: &Composition) -> bool {
    let (w, h) = (f64::from(w), f64::from(h));
    let (out_w, out_h) = (f64::from(comp.width), f64::from(comp.height));
    let scale = match fit {
        Fit::None => [1.0, 1.0],
        Fit::Contain => {
            let s = (out_w / w).min(out_h / h);
            [s, s]
        }
        Fit::Cover => {
            let s = (out_w / w).max(out_h / h);
            [s, s]
        }
        Fit::Fill => [out_w / w, out_h / h],
    };
    (w * scale[0] - out_w).abs() <= 2.0 && (h * scale[1] - out_h).abs() <= 2.0
}

/// How a picture whose scaling does not cover the frame is placed: the
/// picture is the `w`×`h` source, or its `region` when cropped. A
/// `contain` fit scaled to touch two edges, or an unfitted picture at
/// its own size, sits centered on the background, which must be opaque;
/// a `cover` fit has the centered region of the picture that the frame
/// shows scaled to the whole frame. Positions and sizes are rounded to
/// even pixels so that subsampled chroma lines up. `None` for a stretch
/// or an unfitted picture that overflows the frame.
fn placement(
    fit: Fit,
    region: Option<[u32; 4]>,
    w: u32,
    h: u32,
    comp: &Composition,
) -> Option<Place> {
    let [rx, ry, w, h] = region.unwrap_or([0, 0, w, h]);
    let (out_w, out_h) = (f64::from(comp.width), f64::from(comp.height));
    let (fw, fh) = (f64::from(w), f64::from(h));
    let even = |v: f64| ((v / 2.0).round() * 2.0) as u32;
    if fit == Fit::Cover {
        let scale = (out_w / fw).max(out_h / fh);
        let rw = even(out_w / scale).min(w);
        let rh = even(out_h / scale).min(h);
        if rw < 2 || rh < 2 {
            return None;
        }
        let x = rx + even((fw - f64::from(rw)) / 2.0);
        let y = ry + even((fh - f64::from(rh)) / 2.0);
        return Some(Place::Crop([x, y, rw, rh]));
    }
    if comp.background.a < 1.0 {
        return None;
    }
    let scale = match fit {
        Fit::Contain => (out_w / fw).min(out_h / fh),
        Fit::None if fw <= out_w && fh <= out_h => 1.0,
        Fit::None | Fit::Cover | Fit::Fill => return None,
    };
    let rw = even(fw * scale).min(comp.width);
    let rh = even(fh * scale).min(comp.height);
    if rw < 2 || rh < 2 {
        return None;
    }
    let x = even((out_w - f64::from(rw)) / 2.0);
    let y = even((out_h - f64::from(rh)) / 2.0);
    let dst = [x, y, rw, rh];
    Some(match region {
        None => Place::Bars(dst),
        Some(src) => Place::CropBars { src, dst },
    })
}

/// Audio codecs each container can hold without re-encoding.
pub(super) fn container_accepts_audio(container: Container, codec: codec::Id) -> bool {
    use codec::Id;
    match container {
        Container::Mp4 | Container::M4a => {
            matches!(
                codec,
                Id::AAC | Id::MP3 | Id::ALAC | Id::AC3 | Id::EAC3 | Id::OPUS | Id::FLAC
            )
        }
        Container::Mov => matches!(
            codec,
            Id::AAC
                | Id::MP3
                | Id::ALAC
                | Id::AC3
                | Id::EAC3
                | Id::OPUS
                | Id::FLAC
                | Id::PCM_S16LE
                | Id::PCM_S24LE
                | Id::PCM_F32LE
        ),
        Container::Mkv => true,
        Container::Webm => matches!(codec, Id::OPUS | Id::VORBIS),
        Container::Ogg => matches!(codec, Id::OPUS | Id::VORBIS | Id::FLAC),
        Container::Flac => codec == Id::FLAC,
        Container::Mp3 => codec == Id::MP3,
        Container::Wav | Container::Mxf => {
            matches!(codec, Id::PCM_S16LE | Id::PCM_S24LE | Id::PCM_F32LE)
        }
        Container::ImageSequence => false,
    }
}

/// Coded parameters of an audio stream, for copy decisions.
struct AudioShape {
    codec_id: codec::Id,
}

impl AudioShape {
    fn read(path: &Path) -> Result<Self, MediaError> {
        let ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
        let audio = ictx
            .streams()
            .best(Type::Audio)
            .ok_or_else(|| MediaError::NoStream {
                path: path.to_owned(),
                kind: "audio",
            })?;
        Ok(Self {
            codec_id: audio.parameters().id(),
        })
    }
}

fn container_accepts(container: Container, codec: Option<VideoCodec>) -> bool {
    match codec {
        None => false,
        // Image sequences are written frame by frame, never copied.
        Some(_) if container == Container::ImageSequence => false,
        Some(c) => super::encode::container_accepts_video(container, c),
    }
}

/// The coded parameters that must agree between joined sources.
#[derive(Debug, Clone, PartialEq)]
struct StreamShape {
    codec: Option<VideoCodec>,
    codec_id: codec::Id,
    width: u32,
    height: u32,
    /// How wide a stored pixel is shown; `width` already counts it.
    aspect: crate::PixelAspect,
    fps: Ratio,
    format: ffmpeg_next::util::format::Pixel,
    extradata: Vec<u8>,
    has_audio: bool,
    audio_id: Option<codec::Id>,
    audio_extradata: Vec<u8>,
}

impl StreamShape {
    fn read(path: &Path) -> Result<Self, MediaError> {
        let mut ictx = ffmpeg_next::format::input(path).map_err(|e| open_error(path, e))?;
        // The same reconciliation as the probe, so that a file whose
        // timestamps jitter (a phone recording) compares equal to the
        // rate the composition was built at.
        let fps = super::probe::video_frame_rate(&mut ictx).unwrap_or(Ratio::from_int(25));
        let video = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| MediaError::NoStream {
                path: path.to_owned(),
                kind: "video",
            })?;
        let params = video.parameters();
        let ctx = codec::context::Context::from_parameters(params.clone()).map_err(|e| {
            super::codec_error(format!("{}: reading stream parameters", path.display()), e)
        })?;
        let decoder = ctx.decoder().video().map_err(|e| {
            super::codec_error(format!("{}: reading stream parameters", path.display()), e)
        })?;
        // The picture as displayed: a rotated source is compared to the
        // composition at its upright size, and copied with its matrix; one
        // with non-square pixels at its display size, and copied with its
        // pixel aspect (the codec parameters carry it).
        let rotation = ffi::display_rotation(&params);
        let aspect = ffi::sample_aspect_ratio(&video);
        let (shown_w, shown_h) = aspect.display_size(decoder.width(), decoder.height());
        let (width, height) = if rotation % 180 == 90 {
            (shown_h, shown_w)
        } else {
            (shown_w, shown_h)
        };
        let audio = ictx.streams().best(Type::Audio);
        Ok(Self {
            codec: match params.id() {
                codec::Id::H264 => Some(VideoCodec::H264),
                codec::Id::HEVC => Some(VideoCodec::H265),
                codec::Id::VP9 => Some(VideoCodec::Vp9),
                codec::Id::AV1 => Some(VideoCodec::Av1),
                codec::Id::PRORES => Some(VideoCodec::Prores),
                codec::Id::DNXHD => Some(VideoCodec::Dnxhd),
                codec::Id::MJPEG => Some(VideoCodec::Mjpeg),
                codec::Id::PNG => Some(VideoCodec::Png),
                _ => None,
            },
            codec_id: params.id(),
            width,
            height,
            aspect,
            fps,
            format: decoder.format(),
            extradata: ffi::extradata(&params),
            has_audio: audio.is_some(),
            audio_id: audio.as_ref().map(|a| a.parameters().id()),
            audio_extradata: audio
                .as_ref()
                .map(|a| ffi::extradata(&a.parameters()))
                .unwrap_or_default(),
        })
    }

    fn compatible(&self, other: &Self) -> bool {
        self.codec_id == other.codec_id
            && self.width == other.width
            && self.height == other.height
            && self.aspect == other.aspect
            && self.format == other.format
            && self.extradata == other.extradata
            && self.audio_id == other.audio_id
            && self.audio_extradata == other.audio_extradata
    }

    /// The first thing [`Self::compatible`] finds different, named.
    fn difference(&self, other: &Self) -> &'static str {
        if self.codec_id != other.codec_id {
            "video codec"
        } else if self.width != other.width || self.height != other.height {
            "picture size"
        } else if self.aspect != other.aspect {
            "pixel aspect"
        } else if self.format != other.format {
            "pixel format"
        } else if self.extradata != other.extradata {
            "encoder settings (the coded parameter sets)"
        } else if self.audio_id != other.audio_id {
            "audio codec"
        } else {
            "audio settings"
        }
    }
}

/// Writes `output` by copying the planned segments' packets.
pub fn stream_copy(
    plan: &CopyPlan,
    output: &Path,
    subtitles: &[SubtitleSettings],
    fast_start: bool,
) -> Result<CopyReport, MediaError> {
    init();
    let container = super::encode::container_for(output, None);
    let mut octx = match container.and_then(super::encode::muxer_name) {
        Some(name) => {
            ffmpeg_next::format::output_as(output, name).map_err(|e| open_error(output, e))?
        }
        None => ffmpeg_next::format::output(output).map_err(|e| open_error(output, e))?,
    };

    // Output streams take their parameters from the first source of each kind.
    let out_video = match plan.segments.first() {
        Some(first) => {
            let template =
                ffmpeg_next::format::input(&first.path).map_err(|e| open_error(&first.path, e))?;
            let video_in =
                template
                    .streams()
                    .best(Type::Video)
                    .ok_or_else(|| MediaError::NoStream {
                        path: first.path.clone(),
                        kind: "video",
                    })?;
            Some(add_copied_stream(&mut octx, &video_in, output)?)
        }
        None => None,
    };
    let out_audio = match plan.audio.first() {
        Some(first) => {
            let template =
                ffmpeg_next::format::input(&first.path).map_err(|e| open_error(&first.path, e))?;
            let audio_in =
                template
                    .streams()
                    .best(Type::Audio)
                    .ok_or_else(|| MediaError::NoStream {
                        path: first.path.clone(),
                        kind: "audio",
                    })?;
            Some(add_copied_stream(&mut octx, &audio_in, output)?)
        }
        None => None,
    };
    if out_video.is_none() && out_audio.is_none() {
        return Err(MediaError::Codec {
            context: "stream copy".to_owned(),
            reason: "nothing to copy".to_owned(),
        });
    }
    let mut cues = SubtitleWriter::add_streams(&mut octx, container, subtitles, output)?;
    super::encode::write_header(&mut octx, container, fast_start, output)?;

    let mut report_segments = Vec::new();
    let mut video_packets = 0u64;
    let mut duration = Ratio::ZERO;
    // MP4 and QuickTime hold a packet that starts before time zero (an
    // audio lead-in) in an edit list; other containers would shift every
    // stream to make room for it.
    let lead_in_ok = matches!(container, Some(Container::Mp4 | Container::Mov));
    if let Some(out_idx) = out_video {
        let (segs, total) = copy_track(
            &mut octx,
            &plan.segments,
            Type::Video,
            out_idx,
            &mut video_packets,
            &mut cues,
            lead_in_ok,
            &mut |_, _| Ok(()),
        )?;
        report_segments = segs;
        duration = total;
    }
    // The clips' own audio is cut where the picture actually starts: a
    // cut that moved to a keyframe takes the sound with it.
    let mut audio_plan = plan.audio.clone();
    if plan.audio == plan.segments {
        for (seg, (_, actual, _)) in audio_plan.iter_mut().zip(&report_segments) {
            seg.from = *actual;
        }
    }
    if let Some(out_idx) = out_audio {
        let mut ignored = 0u64;
        let (segs, total) = copy_track(
            &mut octx,
            &audio_plan,
            Type::Audio,
            out_idx,
            &mut ignored,
            &mut cues,
            lead_in_ok,
            &mut |_, _| Ok(()),
        )?;
        if out_video.is_none() {
            report_segments = segs;
            duration = total;
        }
    }
    cues.finish(&mut octx)?;
    octx.write_trailer().map_err(|e| open_error(output, e))?;
    Ok(CopyReport {
        segments: report_segments,
        duration,
        video_packets,
    })
}

/// Writes `output` with the planned video copied packet for packet and
/// the audio encoded from `blocks`, which yield the treated mix in
/// interleaved stereo at the settings' rate.
///
/// This is what an output whose sound is brought to a loudness or
/// cleaned takes when its picture needs nothing: the treatment is a
/// change no copied audio track can carry, but the picture is untouched
/// either way, so only the sound is encoded. `plan.audio` is ignored.
///
/// The two tracks are written together, the audio caught up after each
/// video packet, so the muxer interleaves them as it goes instead of
/// holding a whole track in memory.
pub fn stream_copy_mixing_audio(
    plan: &CopyPlan,
    output: &Path,
    subtitles: &[SubtitleSettings],
    fast_start: bool,
    audio: &super::encode::AudioSettings,
    blocks: &mut dyn FnMut(usize) -> Result<Option<Vec<f32>>, MediaError>,
) -> Result<CopyReport, MediaError> {
    init();
    let container = super::encode::container_for(output, None);
    let mut octx = match container.and_then(super::encode::muxer_name) {
        Some(name) => {
            ffmpeg_next::format::output_as(output, name).map_err(|e| open_error(output, e))?
        }
        None => ffmpeg_next::format::output(output).map_err(|e| open_error(output, e))?,
    };
    let first = plan.segments.first().ok_or_else(|| MediaError::Codec {
        context: "stream copy".to_owned(),
        reason: "no video to copy".to_owned(),
    })?;
    let out_video = {
        let template =
            ffmpeg_next::format::input(&first.path).map_err(|e| open_error(&first.path, e))?;
        let video_in =
            template
                .streams()
                .best(Type::Video)
                .ok_or_else(|| MediaError::NoStream {
                    path: first.path.clone(),
                    kind: "video",
                })?;
        add_copied_stream(&mut octx, &video_in, output)?
    };
    let global_header = octx
        .format()
        .flags()
        .contains(ffmpeg_next::format::Flags::GLOBAL_HEADER);
    let mut encoder = super::encode::open_audio_track(&mut octx, output, audio, global_header)?;
    let mut cues = SubtitleWriter::add_streams(&mut octx, container, subtitles, output)?;
    super::encode::write_header(&mut octx, container, fast_start, output)?;
    encoder.follow_muxer(&octx);

    // A tenth of a second at a time: short enough that the audio stays
    // beside the picture in the file, long enough not to be all overhead.
    let block = (audio.sample_rate as usize / 10).max(1);
    let mut drained = false;
    let mut pump =
        |octx: &mut ffmpeg_next::format::context::Output, upto: Ratio| -> Result<(), MediaError> {
            while !drained && encoder.time() < upto {
                match blocks(block)? {
                    Some(chunk) => {
                        for packet in encoder.push(&chunk)? {
                            packet
                                .write_interleaved(octx)
                                .map_err(|e| super::codec_error("writing audio packet", e))?;
                        }
                    }
                    None => drained = true,
                }
            }
            Ok(())
        };

    let mut video_packets = 0u64;
    let (report_segments, duration) = copy_track(
        &mut octx,
        &plan.segments,
        Type::Video,
        out_video,
        &mut video_packets,
        &mut cues,
        matches!(container, Some(Container::Mp4 | Container::Mov)),
        &mut pump,
    )?;
    // The mix can outlast the picture (a track that runs on under a
    // still) and the last video packet leaves the encoder part-fed, so
    // what is left is written after the copy rather than during it.
    while !drained {
        match blocks(block)? {
            Some(chunk) => {
                for packet in encoder.push(&chunk)? {
                    packet
                        .write_interleaved(&mut octx)
                        .map_err(|e| super::codec_error("writing audio packet", e))?;
                }
            }
            None => drained = true,
        }
    }
    for packet in encoder.finish()? {
        packet
            .write_interleaved(&mut octx)
            .map_err(|e| super::codec_error("writing audio packet", e))?;
    }
    cues.finish(&mut octx)?;
    octx.write_trailer().map_err(|e| open_error(output, e))?;
    Ok(CopyReport {
        segments: report_segments,
        duration,
        video_packets,
    })
}

/// Copies one kind of stream from a list of segments into output stream
/// `out_idx`, returning the per-segment report and the total duration.
#[allow(
    clippy::too_many_arguments,
    reason = "one demuxing loop with its output stream, its counters and its hook"
)]
fn copy_track(
    octx: &mut ffmpeg_next::format::context::Output,
    segments: &[CopySegment],
    kind: Type,
    out_idx: usize,
    packets: &mut u64,
    cues: &mut SubtitleWriter,
    lead_in_ok: bool,
    reached: &mut dyn FnMut(
        &mut ffmpeg_next::format::context::Output,
        Ratio,
    ) -> Result<(), MediaError>,
) -> Result<(Vec<SegmentReport>, Ratio), MediaError> {
    let mut report = Vec::new();
    let mut offset = Ratio::ZERO;
    // Cues are written alongside the video packets, or the audio ones when
    // the output has no video.
    let cues_follow_video = octx
        .streams()
        .any(|s| s.parameters().medium() == Type::Video);
    let out_tb = octx.stream(out_idx).expect("added").time_base();
    let out_scale = Ratio::new(
        i64::from(out_tb.denominator()),
        i64::from(out_tb.numerator()),
    );
    for segment in segments {
        let mut ictx =
            ffmpeg_next::format::input(&segment.path).map_err(|e| open_error(&segment.path, e))?;
        let (in_idx, tb) = {
            let s = ictx.streams().best(kind).expect("checked by the plan");
            (s.index(), s.time_base())
        };
        // Time zero of the file is its first video frame, for both kinds
        // of stream, so an audio track keeps its offset from the picture.
        let zero = file_zero(&ictx);
        let seek_to = |ictx: &mut ffmpeg_next::format::context::Input| {
            if segment.from > Ratio::ZERO {
                super::probe::seek_before(ictx, in_idx, segment.from + zero, tb).map_err(|e| {
                    super::codec_error(format!("{}: seeking", segment.path.display()), e)
                })?;
            }
            Ok::<(), MediaError>(())
        };
        seek_to(&mut ictx)?;
        // Video without decode timestamps (Matroska stores none) gets them
        // from the presentation order, as the container needs.
        let mut decode_times = None;
        // The segment starts at the first keyframe the demuxer yields;
        // audio starts where it is asked to, so that its offset from the
        // picture survives, and at the start of the output the packet
        // under the cut comes too, so that the decoder has its lead-in
        // (an encoder's priming, which the container then trims).
        // Everything is timed relative to the segment's start.
        let mut segment_start: Option<Ratio> = if kind == Type::Audio {
            Some(segment.from)
        } else {
            None
        };
        let mut last_end = segment.from;
        let mut index = 0usize;
        // Video packets from the latest keyframe at or before `from`,
        // held until it is known to be the last such keyframe (a packet
        // past `from` arrives), then written as the segment's start.
        let mut held: Vec<Packet> = Vec::new();
        let mut held_start: Option<Ratio> = None;
        let mut queue: std::collections::VecDeque<Packet> = std::collections::VecDeque::new();
        let mut packet = Packet::empty();
        loop {
            let mut packet = match queue.pop_front() {
                Some(p) => p,
                None => {
                    if super::read_packet(&mut packet, &mut ictx).is_err() {
                        break;
                    }
                    if packet.stream() != in_idx {
                        continue;
                    }
                    packet.clone()
                }
            };
            let Some(pts) = packet.pts().or(packet.dts()) else {
                continue;
            };
            let time = ts_to_secs(pts, tb) - zero;
            let packet_duration = if packet.duration() > 0 {
                ts_to_secs(packet.duration(), tb)
            } else {
                Ratio::ZERO
            };
            if segment_start.is_none() {
                if time <= segment.from {
                    if packet.is_key() {
                        held.clear();
                        held_start = Some(time);
                    }
                    if held_start.is_some() {
                        held.push(packet);
                    }
                    continue;
                }
                match held_start {
                    Some(start) => {
                        // Every later keyframe is past `from`: the held
                        // one starts the segment, its packets first.
                        segment_start = Some(start);
                        queue.extend(held.drain(..));
                        queue.push_back(packet);
                        continue;
                    }
                    None if packet.is_key() => segment_start = Some(time),
                    None => continue,
                }
            }
            let seg_start = segment_start.expect("set above");
            let lead_in = kind == Type::Audio
                && lead_in_ok
                && offset == Ratio::ZERO
                && time + packet_duration >= seg_start;
            if time < seg_start && !lead_in {
                continue;
            }
            if segment.to.is_some_and(|to| time >= to) {
                break;
            }
            if kind == Type::Video && packet.dts().is_none() && decode_times.is_none() {
                let mut pass = ffmpeg_next::format::input(&segment.path)
                    .map_err(|e| open_error(&segment.path, e))?;
                seek_to(&mut pass)?;
                decode_times = Some(decode_order_times(
                    &mut pass,
                    in_idx,
                    tb,
                    zero + seg_start,
                    segment.to.map(|to| to + zero),
                ));
            }
            last_end = last_end.max(time + packet_duration);
            let shifted = time - seg_start + offset;
            let out_pts = (shifted * out_scale).round();
            let out_dts = match (packet.dts(), &decode_times) {
                (Some(d), _) => ((shifted + ts_to_secs(d - pts, tb)) * out_scale).round(),
                (None, Some(times)) => match times.get(index) {
                    Some(&d) => ((shifted + ts_to_secs(d - pts, tb)) * out_scale).round(),
                    None => out_pts,
                },
                (None, None) => out_pts,
            };
            index += 1;
            packet.set_stream(out_idx);
            packet.set_pts(Some(out_pts));
            packet.set_dts(Some(out_dts));
            if packet.duration() > 0 {
                packet.set_duration(rescale(packet.duration(), tb, out_tb));
            }
            packet.set_position(-1);
            packet
                .write_interleaved(octx)
                .map_err(|e| super::codec_error("writing copied packet", e))?;
            *packets += 1;
            if kind == Type::Video || !cues_follow_video {
                cues.write_due(octx, shifted)?;
            }
            // Another track that is written as this one advances, so the
            // muxer interleaves the two instead of holding one whole.
            reached(octx, shifted)?;
        }
        let seg_start = segment_start.unwrap_or(segment.from);
        let seg_end = segment
            .to
            .map_or(last_end, |t| t.min(last_end).max(seg_start));
        // Packets before time zero are encoder priming, not a moved cut.
        report.push((segment.from, seg_start.max(Ratio::ZERO), seg_end));
        offset = offset + (seg_end - seg_start);
    }
    Ok((report, offset))
}

/// Time zero of a file: its first video frame, or its first sample when
/// it has no video.
pub(super) fn file_zero(ictx: &ffmpeg_next::format::context::Input) -> Ratio {
    let start = |s: ffmpeg_next::format::stream::Stream| {
        let t = s.start_time();
        if t == i64::MIN {
            Ratio::ZERO
        } else {
            ts_to_secs(t, s.time_base())
        }
    };
    match ictx.streams().best(Type::Video) {
        Some(video) => start(video),
        None => ictx
            .streams()
            .best(Type::Audio)
            .map_or(Ratio::ZERO, |a| start(a).max(Ratio::ZERO)),
    }
}

/// Decode timestamps for the video packets of one segment, in the order
/// they are stored, for a container that stores only presentation
/// times: the k-th packet decodes at the k-th earliest presentation
/// time, shifted back by the reorder depth so that every packet decodes
/// before it is shown. The result is indexed like the packets read from
/// `from` (the first keyframe at or after it) up to `to`.
fn decode_order_times(
    ictx: &mut ffmpeg_next::format::context::Input,
    in_idx: usize,
    tb: Rational,
    from: Ratio,
    to: Option<Ratio>,
) -> Vec<i64> {
    let mut pts_list = Vec::new();
    let mut started = false;
    let mut packet = Packet::empty();
    while super::read_packet(&mut packet, ictx).is_ok() {
        if packet.stream() != in_idx {
            continue;
        }
        let Some(pts) = packet.pts() else {
            continue;
        };
        let time = ts_to_secs(pts, tb);
        if !started {
            if !packet.is_key() || time < from {
                continue;
            }
            started = true;
        }
        if to.is_some_and(|to| time >= to) {
            break;
        }
        pts_list.push(pts);
    }
    let mut sorted = pts_list.clone();
    sorted.sort_unstable();
    // The reorder depth: how far a packet is stored ahead of its place
    // in presentation order.
    let depth = pts_list
        .iter()
        .enumerate()
        .map(|(k, p)| k.saturating_sub(sorted.partition_point(|s| s < p)))
        .max()
        .unwrap_or(0);
    // One frame, for the first packets that decode before anything is shown.
    let step = sorted
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0)
        .min()
        .unwrap_or(1);
    (0..pts_list.len())
        .map(|k| {
            if k >= depth {
                sorted[k - depth]
            } else {
                sorted[0] - (depth - k) as i64 * step
            }
        })
        .collect()
}

fn rescale(value: i64, from: Rational, to: Rational) -> i64 {
    let r = Ratio::from_int(value)
        * Ratio::new(i64::from(from.numerator()), i64::from(from.denominator()))
        * Ratio::new(i64::from(to.denominator()), i64::from(to.numerator()));
    r.round()
}

pub(super) fn add_copied_stream(
    octx: &mut ffmpeg_next::format::context::Output,
    input: &ffmpeg_next::format::stream::Stream,
    output: &Path,
) -> Result<usize, MediaError> {
    let mut params = input.parameters();
    ffi::clear_codec_tag(&mut params);
    let ctx = codec::context::Context::from_parameters(params)
        .map_err(|e| super::codec_error("copying stream parameters", e))?;
    let mut stream = octx
        .add_stream_with(&ctx)
        .map_err(|e| open_error(output, e))?;
    stream.set_time_base(input.time_base());
    if input.parameters().medium() == Type::Video {
        stream.set_avg_frame_rate(input.avg_frame_rate());
        stream.set_rate(input.rate());
        let params = input.parameters();
        if ffi::display_matrix(&stream.parameters()).is_none() {
            if let Some(matrix) = ffi::display_matrix(&params) {
                ffi::attach_display_matrix(&mut stream, &matrix);
            }
        }
    }
    Ok(stream.index())
}
