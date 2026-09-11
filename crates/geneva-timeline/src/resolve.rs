//! Resolution of a parsed timeline into an exact, renderable composition.
//!
//! Resolution and semantic validation are one pass: every rule that needs
//! timing or cross-references is checked here while the composition is
//! built, so the diagnostics and the resolved values can never disagree.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use geneva_anim::{Easing, Keyframe, Track};
use geneva_color::{Color, ColorTags, LinearRgba, ResolvedTags};
use serde_json::json;

use crate::animated::Animated;
use crate::color::ColorValue;
use crate::diagnostic::{Diagnostic, Path};
use crate::length::{Length, Point, Scale};
use crate::ratio::Ratio;
use crate::schema::{
    ACCEPTED_VERSIONS, Asset, AssetKind, AudioOutput, AudioTrack, BlendMode, CompositionDef, Crop,
    Encode, FORMAT_VERSION, Fit, Layer, ShapeKind, Source, TextSource, Timeline, Transform,
    TransitionKind,
};
use crate::time::Time;

/// A fully resolved composition with exact times and sampleable tracks.
#[derive(Debug, Clone)]
pub struct Composition {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Frame rate.
    pub fps: Ratio,
    /// Total duration in seconds.
    pub duration: Ratio,
    /// Clear color.
    pub background: Color,
    /// Output color tags.
    pub color: ResolvedTags,
    /// Audio output format, as written.
    pub audio_output: Option<AudioOutput>,
    /// Encoder settings, as written.
    pub encode: Option<Encode>,
    /// Assets by id.
    pub assets: BTreeMap<String, ResolvedAsset>,
    /// Visual layers, bottom first.
    pub layers: Vec<ResolvedLayer>,
    /// Audio tracks.
    pub audio: Vec<ResolvedAudioTrack>,
    /// Subtitle tracks to write as text streams.
    pub subtitles: Vec<ResolvedSubtitleTrack>,
}

impl Composition {
    /// Number of frames in the output, rounding partial frames up.
    pub fn frame_count(&self) -> u64 {
        (self.duration * self.fps).ceil().max(0) as u64
    }

    /// The presentation time of frame `n`.
    pub fn frame_time(&self, n: u64) -> Ratio {
        Ratio::from_int(n as i64) / self.fps
    }

    /// Clips visible at time `t`, bottom layer first, in layer order.
    pub fn clips_at(&self, t: Ratio) -> impl Iterator<Item = (&ResolvedLayer, &ResolvedClip)> {
        self.layers.iter().flat_map(move |layer| {
            layer
                .clips
                .iter()
                .filter(move |c| c.start <= t && t < c.end)
                .map(move |c| (layer, c))
        })
    }
}

/// An asset with its kind decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAsset {
    /// Relative path as written.
    pub src: String,
    /// Content kind.
    pub kind: AssetKind,
    /// Color tag overrides.
    pub color: ColorTags,
}

/// A visual layer.
#[derive(Debug, Clone)]
pub struct ResolvedLayer {
    /// Identifier for diagnostics: the given id or a positional name.
    pub id: String,
    /// Clips in time order.
    pub clips: Vec<ResolvedClip>,
}

/// A clip with exact timing and sampleable properties.
#[derive(Debug, Clone)]
pub struct ResolvedClip {
    /// Identifier for diagnostics.
    pub id: String,
    /// JSON pointer to the clip in the source document.
    pub path: String,
    /// Timeline time at which the clip becomes visible.
    pub start: Ratio,
    /// Timeline time at which the clip stops being visible (exclusive).
    pub end: Ratio,
    /// Content.
    pub source: ResolvedSource,
    /// The rectangle of the source shown, when not all of it; resolved
    /// against the source's size by [`Crop::to_px`] once that is known.
    pub crop: Option<Crop>,
    /// Sizing mode.
    pub fit: Fit,
    /// Blend mode.
    pub blend: BlendMode,
    /// Anchor point on the clip's box.
    pub anchor: Point,
    /// Anchor position in output pixels over clip-local time.
    pub position: Track<[f64; 2]>,
    /// Scale factors over clip-local time.
    pub scale: Track<[f64; 2]>,
    /// Rotation in degrees over clip-local time.
    pub rotation: Track<f64>,
    /// Opacity over clip-local time.
    pub opacity: Track<f64>,
    /// Transition from the previous clip, with its duration.
    pub transition_in: Option<(TransitionKind, Ratio)>,
}

impl ResolvedClip {
    /// Duration of the clip.
    pub fn duration(&self) -> Ratio {
        self.end - self.start
    }
}

/// Clip content after resolution.
#[derive(Debug, Clone)]
pub enum ResolvedSource {
    /// Frames from a video asset.
    Video {
        /// Asset id.
        asset: String,
        /// Source time corresponding to the clip start.
        in_: Ratio,
        /// Whether the asset's audio is mixed in.
        audio: bool,
    },
    /// A still image asset.
    Image {
        /// Asset id.
        asset: String,
    },
    /// A frame-sized solid color.
    Solid {
        /// Premultiplied linear color over clip-local time.
        color: Track<LinearRgba>,
    },
    /// A vector shape.
    Shape {
        /// Geometry.
        kind: ShapeKind,
        /// Box width in pixels.
        width: f64,
        /// Box height in pixels.
        height: f64,
        /// Fill over clip-local time.
        fill: Track<LinearRgba>,
        /// Outline color and width.
        stroke: Option<(LinearRgba, f64)>,
        /// Corner radius in pixels.
        radius: f64,
    },
    /// Text.
    Text(Box<ResolvedText>),
    /// A nested composition, rendered as a unit.
    Composition(Box<ResolvedComposition>),
}

/// A nested composition with its layers resolved relative to the clip
/// that shows it.
#[derive(Debug, Clone)]
pub struct ResolvedComposition {
    /// Name under "compositions".
    pub name: String,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Clear color.
    pub background: Color,
    /// Layers, with times relative to the clip start.
    pub layers: Vec<ResolvedLayer>,
}

/// Text content with defaults applied.
#[derive(Debug, Clone)]
pub struct ResolvedText {
    /// The full text.
    pub text: String,
    /// Timed words as `(word, start, end)` in clip-local seconds.
    pub words: Vec<(String, Ratio, Ratio)>,
    /// Maximum line width in pixels.
    pub max_width: f64,
    /// The style block as written.
    pub spec: TextSource,
}

/// A subtitle track with its timing offset resolved.
#[derive(Debug, Clone)]
pub struct ResolvedSubtitleTrack {
    /// Identifier for diagnostics.
    pub id: String,
    /// Asset id of the subtitle file.
    pub asset: String,
    /// Language code, if given.
    pub language: Option<String>,
    /// Title, if given.
    pub title: Option<String>,
    /// Shift applied to every cue, in seconds; may be negative.
    pub offset: Ratio,
}

/// An audio track.
#[derive(Debug, Clone)]
pub struct ResolvedAudioTrack {
    /// Identifier for diagnostics.
    pub id: String,
    /// Clips in time order.
    pub clips: Vec<ResolvedAudioClip>,
}

/// An audio clip with exact timing.
#[derive(Debug, Clone)]
pub struct ResolvedAudioClip {
    /// Identifier for diagnostics.
    pub id: String,
    /// Asset id.
    pub asset: String,
    /// Source time corresponding to the clip start.
    pub in_: Ratio,
    /// Timeline start.
    pub start: Ratio,
    /// Timeline end (exclusive).
    pub end: Ratio,
    /// Gain in decibels over clip-local time.
    pub gain_db: Track<f64>,
    /// Fade-in duration.
    pub fade_in: Ratio,
    /// Fade-out duration.
    pub fade_out: Ratio,
}

/// Anchor, position, scale and rotation of a clip after resolution.
type ResolvedTransform = (Point, Track<[f64; 2]>, Track<[f64; 2]>, Track<f64>);

/// Facts about asset files that only reading them can provide.
pub trait AssetInfo {
    /// Duration of a media asset in seconds, if known.
    fn duration(&self, asset_id: &str, src: &str) -> Option<Ratio>;
}

/// An [`AssetInfo`] that knows nothing, for validation without files.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAssetInfo;

impl AssetInfo for NoAssetInfo {
    fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
        None
    }
}

/// Resolves a timeline without reading any files. Returns the composition
/// only when there are no errors; diagnostics are returned in either case.
pub fn resolve(timeline: &Timeline) -> (Option<Composition>, Vec<Diagnostic>) {
    resolve_with(timeline, &NoAssetInfo)
}

/// Resolves a timeline using `info` to close open-ended media clips at the
/// end of their files.
pub fn resolve_with(
    timeline: &Timeline,
    info: &dyn AssetInfo,
) -> (Option<Composition>, Vec<Diagnostic>) {
    let mut r = Resolver {
        tl: timeline,
        info,
        diags: Vec::new(),
        fps: timeline.output.fps.ratio(),
        used_assets: BTreeSet::new(),
        used_compositions: BTreeSet::new(),
        composition_stack: Vec::new(),
    };
    let comp = r.run();
    let has_errors = r.diags.iter().any(Diagnostic::is_error);
    (if has_errors { None } else { Some(comp) }, r.diags)
}

struct Resolver<'a> {
    tl: &'a Timeline,
    info: &'a dyn AssetInfo,
    diags: Vec<Diagnostic>,
    fps: Ratio,
    used_assets: BTreeSet<String>,
    used_compositions: BTreeSet<String>,
    /// Names of the compositions currently being resolved, for cycle checks.
    composition_stack: Vec<String>,
}

/// Deepest allowed nesting of compositions inside compositions.
const MAX_COMPOSITION_DEPTH: usize = 8;

/// The frame a layer is resolved against: the output, or a composition.
#[derive(Clone, Copy)]
struct FrameSize {
    width: u32,
    height: u32,
}

/// Layers of a frame after resolution, plus whether any clip was
/// open-ended (took its length from the frame's duration).
struct ResolvedLayers {
    layers: Vec<ResolvedLayer>,
    any_open: bool,
}

/// The length of a clip before its end is known.
enum ClipLength {
    /// A fixed length.
    Fixed(Ratio),
    /// Extends to the end of the output.
    Open,
}

impl Resolver<'_> {
    fn push(&mut self, d: Diagnostic) {
        self.diags.push(d);
    }

    fn run(&mut self) -> Composition {
        let tl = self.tl;
        if !ACCEPTED_VERSIONS.contains(&tl.geneva.as_str()) {
            self.push(
                Diagnostic::error(
                    "E110",
                    "/geneva",
                    format!("unsupported format version {:?}", tl.geneva),
                )
                .with_value(tl.geneva.clone())
                .with_help(format!(
                    "this build writes version \"{FORMAT_VERSION}\" and reads {}",
                    ACCEPTED_VERSIONS
                        .iter()
                        .map(|v| format!("\"{v}\""))
                        .collect::<Vec<_>>()
                        .join(" and ")
                )),
            );
        }

        let out = &tl.output;
        let out_path = Path::root().key("output");
        for (name, v) in [("width", out.width), ("height", out.height)] {
            if v == 0 {
                self.push(
                    Diagnostic::error(
                        "E402",
                        out_path.key(name),
                        format!("output {name} must be greater than 0"),
                    )
                    .with_value(v),
                );
            } else if v % 2 == 1 {
                self.push(
                    Diagnostic::warning(
                        "W401",
                        out_path.key(name),
                        format!("output {name} {v} is odd"),
                    )
                    .with_value(v)
                    .with_help(format!(
                        "most video codecs need even dimensions; use {} or {}",
                        v - 1,
                        v + 1
                    )),
                );
            }
        }
        if self.fps <= Ratio::ZERO {
            self.push(Diagnostic::error(
                "E402",
                out_path.key("fps"),
                "frame rate must be positive",
            ));
        }

        let explicit_duration = out.duration.map(|d| {
            let secs = d.resolve(self.fps);
            if secs <= Ratio::ZERO {
                self.push(
                    Diagnostic::error(
                        "E402",
                        out_path.key("duration"),
                        "output duration must be greater than 0",
                    )
                    .with_value(json!(d.to_string())),
                );
            }
            secs
        });

        let assets = self.resolve_assets();

        let frame = FrameSize {
            width: out.width,
            height: out.height,
        };
        let mut layers = self
            .resolve_layers(
                &tl.layers,
                &Path::root().key("layers"),
                &assets,
                frame,
                explicit_duration,
            )
            .layers;
        let mut audio = Vec::new();
        for (i, track) in tl.audio.iter().enumerate() {
            let path = Path::root().key("audio").index(i);
            if !track.enabled {
                continue;
            }
            audio.push(self.resolve_audio_track(track, i, &path, &assets));
        }
        let mut subtitles = Vec::new();
        for (i, track) in tl.subtitles.iter().enumerate() {
            let path = Path::root().key("subtitles").index(i);
            if !track.enabled {
                continue;
            }
            let id = track
                .id
                .clone()
                .unwrap_or_else(|| format!("subtitle track {i}"));
            self.asset_ref(
                &track.asset,
                &path.key("asset"),
                &assets,
                &[AssetKind::Subtitle],
            );
            let offset = track.offset.map_or(Ratio::ZERO, |t| t.resolve(self.fps));
            subtitles.push(ResolvedSubtitleTrack {
                id,
                asset: track.asset.clone(),
                language: track.language.clone(),
                title: track.title.clone(),
                offset,
            });
        }

        let last_end = layers
            .iter()
            .flat_map(|l| l.clips.iter().map(|c| c.end))
            .chain(audio.iter().flat_map(|t| t.clips.iter().map(|c| c.end)))
            .max()
            .unwrap_or(Ratio::ZERO);
        let duration = match explicit_duration {
            Some(d) => d,
            None if last_end > Ratio::ZERO => last_end,
            None => {
                self.push(
                    Diagnostic::error(
                        "E305",
                        out_path.key("duration"),
                        "the output duration cannot be determined",
                    )
                    .with_help("set output.duration, or add a clip with a known length"),
                );
                Ratio::ZERO
            }
        };

        // Clips that run past the output are cut, with a warning, so the
        // renderer never has to reason about frames beyond the end.
        for layer in &mut layers {
            for clip in &mut layer.clips {
                if clip.start >= duration {
                    self.diags.push(
                        Diagnostic::warning("W301", format!("{}/start", clip.path), format!("{} starts at {}s, at or after the end of the output ({}s)", clip.id, clip.start, duration))
                            .with_help("it will never be visible; shorten it, move it earlier or extend output.duration"),
                    );
                } else if clip.end > duration {
                    self.diags.push(
                        Diagnostic::warning("W301", format!("{}/duration", clip.path), format!("{} ends at {}s, after the end of the output ({}s)", clip.id, clip.end, duration))
                            .with_help("it will be cut at the output end; shorten it or extend output.duration"),
                    );
                    clip.end = duration;
                }
            }
        }

        for name in tl.compositions.keys() {
            if !self.used_compositions.contains(name) {
                self.diags.push(Diagnostic::note(
                    "W202",
                    Path::root().key("compositions").key(name),
                    format!("composition {name:?} is never used"),
                ));
            }
        }
        for (id, asset) in &tl.assets {
            if asset.kind != Some(AssetKind::Font) && !self.used_assets.contains(id) {
                self.diags.push(Diagnostic::note(
                    "W201",
                    Path::root().key("assets").key(id),
                    format!("asset {id:?} is never used"),
                ));
            }
        }

        // Outputs default to BT.709 SDR at any size; size-based inference is
        // for untagged sources, not for what gets written.
        let requested = out.color.unwrap_or_default();
        let color = ResolvedTags {
            primaries: requested
                .primaries
                .unwrap_or(ResolvedTags::SDR_VIDEO.primaries),
            transfer: requested
                .transfer
                .unwrap_or(ResolvedTags::SDR_VIDEO.transfer),
            matrix: requested.matrix.unwrap_or(ResolvedTags::SDR_VIDEO.matrix),
            range: requested.range.unwrap_or(ResolvedTags::SDR_VIDEO.range),
        };
        if color.is_hdr() {
            self.push(
                Diagnostic::error(
                    "E420",
                    out_path.key("color").key("transfer"),
                    "HDR output is not supported",
                )
                .with_help("remove output.color.transfer or set it to \"bt709\""),
            );
        }
        if let Some(video) = out.encode.as_ref().and_then(|e| e.video.as_ref()) {
            let vpath = out_path.key("encode").key("video");
            if video
                .keyframe_interval
                .is_some_and(|k| k <= 0.0 || !k.is_finite())
            {
                self.push(
                    Diagnostic::error(
                        "E402",
                        vpath.key("keyframe_interval"),
                        "keyframe_interval must be greater than 0",
                    )
                    .with_value(json!(video.keyframe_interval)),
                );
            }
            if video.max_bitrate_kbps == Some(0) {
                self.push(Diagnostic::error(
                    "E402",
                    vpath.key("max_bitrate_kbps"),
                    "max_bitrate_kbps must be greater than 0",
                ));
            }
            if video.bitrate_kbps == Some(0) {
                self.push(Diagnostic::error(
                    "E402",
                    vpath.key("bitrate_kbps"),
                    "bitrate_kbps must be greater than 0",
                ));
            }
            if let Some(level) = &video.level {
                let ok = level.len() <= 4
                    && level.chars().next().is_some_and(|c| c.is_ascii_digit())
                    && level.chars().all(|c| c.is_ascii_digit() || c == '.');
                if !ok {
                    self.push(
                        Diagnostic::error(
                            "E402",
                            vpath.key("level"),
                            format!("{level:?} is not a level such as \"4.1\""),
                        )
                        .with_value(json!(level)),
                    );
                }
            }
        }
        if let Some(video) = out.encode.as_ref().and_then(|e| e.video.as_ref()) {
            if let Some(profile) = video.profile {
                let wanted = profile.codec();
                match video.codec {
                    Some(codec) if codec != wanted => self.push(
                        Diagnostic::error(
                            "E421",
                            out_path.key("encode").key("video").key("profile"),
                            format!(
                                "profile {profile:?} belongs to the {wanted:?} codec, not {codec:?}"
                            ),
                        )
                        .with_help(format!(
                            "set encode.video.codec to \"{}\" or choose a profile of {codec:?}",
                            serde_json::to_value(wanted)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_owned))
                                .unwrap_or_default()
                        )),
                    ),
                    Some(_) => {}
                    None => self.push(
                        Diagnostic::error(
                            "E421",
                            out_path.key("encode").key("video").key("codec"),
                            format!(
                                "a profile is set but no codec; {profile:?} belongs to {wanted:?}"
                            ),
                        )
                        .with_help(format!(
                            "set encode.video.codec to \"{}\"",
                            serde_json::to_value(wanted)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_owned))
                                .unwrap_or_default()
                        )),
                    ),
                }
            }
        }

        Composition {
            width: out.width,
            height: out.height,
            fps: self.fps,
            duration,
            background: out.background.map_or(Color::BLACK, |c| c.0),
            color,
            audio_output: out.audio.clone(),
            encode: out.encode.clone(),
            assets,
            layers,
            audio,
            subtitles,
        }
    }

    fn resolve_assets(&mut self) -> BTreeMap<String, ResolvedAsset> {
        let mut assets = BTreeMap::new();
        for (id, asset) in &self.tl.assets {
            let path = Path::root().key("assets").key(id);
            self.check_asset_path(asset, &path);
            let kind = match asset.kind {
                Some(k) => Some(k),
                None => {
                    let ext = std::path::Path::new(&asset.src)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    let inferred = AssetKind::from_extension(ext);
                    if inferred.is_none() {
                        self.push(
                            Diagnostic::error(
                                "E203",
                                path.key("kind"),
                                format!("cannot tell what kind of file {:?} is", asset.src),
                            )
                            .with_help(
                                "set \"kind\" to \"video\", \"image\", \"audio\", \"font\" or \"subtitle\"",
                            ),
                        );
                    }
                    inferred
                }
            };
            if let Some(kind) = kind {
                assets.insert(
                    id.clone(),
                    ResolvedAsset {
                        src: asset.src.clone(),
                        kind,
                        color: asset.color.unwrap_or_default(),
                    },
                );
            }
        }
        assets
    }

    fn check_asset_path(&mut self, asset: &Asset, path: &Path) {
        let src = asset.src.as_str();
        let p = std::path::Path::new(src);
        let escapes = p.is_absolute()
            || src.starts_with('/')
            || src.starts_with('\\')
            || p.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            });
        if src.trim().is_empty() {
            self.push(Diagnostic::error(
                "E202",
                path.key("src"),
                "asset path is empty",
            ));
        } else if escapes {
            self.push(
                Diagnostic::error("E202", path.key("src"), format!("asset path {src:?} leaves the asset root"))
                    .with_value(src)
                    .with_help("use a path relative to the asset root without \"..\"; pass --assets to choose the root"),
            );
        }
    }

    /// Checks an asset reference and returns its kind if valid.
    fn asset_ref(
        &mut self,
        id: &str,
        path: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        allowed: &[AssetKind],
    ) -> Option<AssetKind> {
        let Some(asset) = assets.get(id) else {
            let mut d = Diagnostic::error("E200", path.clone(), format!("unknown asset {id:?}"))
                .with_value(id);
            let known: Vec<&String> = assets.keys().collect();
            if let Some(near) = nearest(id, known.iter().map(|s| s.as_str())) {
                d = d.with_help(format!(
                    "did you mean {near:?}? assets are declared under \"assets\""
                ));
            } else if known.is_empty() {
                d = d.with_help("declare it under \"assets\", for example \"assets\": {\"clip\": {\"src\": \"clip.mp4\"}}");
            } else {
                d = d.with_help(format!(
                    "declared assets: {}",
                    known
                        .iter()
                        .map(|k| format!("{k:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            self.push(d);
            return None;
        };
        self.used_assets.insert(id.to_owned());
        if !allowed.contains(&asset.kind) {
            let wanted = allowed
                .iter()
                .map(|k| format!("\"{}\"", k.as_str()))
                .collect::<Vec<_>>()
                .join(" or ");
            self.push(
                Diagnostic::error(
                    "E201",
                    path.clone(),
                    format!(
                        "asset {id:?} is a {} file, but a {wanted} asset is needed here",
                        asset.kind.as_str()
                    ),
                )
                .with_value(id)
                .with_help(format!(
                    "reference an asset of kind {wanted}, or fix the asset's \"kind\""
                )),
            );
            return None;
        }
        Some(asset.kind)
    }

    fn time(&mut self, t: Time, path: &Path, what: &str) -> Ratio {
        if t.is_negative() {
            self.push(
                Diagnostic::error("E300", path.clone(), format!("{what} must not be negative"))
                    .with_value(json!(t.to_string())),
            );
        }
        t.resolve(self.fps)
    }

    fn resolve_layers(
        &mut self,
        layers: &[Layer],
        path: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        frame: FrameSize,
        frame_duration: Option<Ratio>,
    ) -> ResolvedLayers {
        let mut out = Vec::new();
        let mut any_open = false;
        for (i, layer) in layers.iter().enumerate() {
            if !layer.enabled {
                continue;
            }
            let (resolved, open) =
                self.resolve_layer(layer, i, &path.index(i), assets, frame, frame_duration);
            any_open |= open;
            out.push(resolved);
        }
        ResolvedLayers {
            layers: out,
            any_open,
        }
    }

    /// Resolves one layer; the flag reports whether any clip took its
    /// length from the frame duration.
    fn resolve_layer(
        &mut self,
        layer: &Layer,
        index: usize,
        path: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        frame: FrameSize,
        output_duration: Option<Ratio>,
    ) -> (ResolvedLayer, bool) {
        let id = layer.id.clone().unwrap_or_else(|| format!("layer {index}"));
        if layer.clips.is_empty() {
            self.push(Diagnostic::warning(
                "W302",
                path.key("clips"),
                format!("{id} has no clips"),
            ));
        }
        let mut clips: Vec<ResolvedClip> = Vec::new();
        let mut cursor = Ratio::ZERO;
        let mut any_open = false;
        for (i, clip) in layer.clips.iter().enumerate() {
            let cpath = path.key("clips").index(i);
            let clip_id = clip
                .id
                .clone()
                .unwrap_or_else(|| format!("clip {i} of {id}"));

            let mut start = match clip.start {
                Some(t) => self.time(t, &cpath.key("start"), "start"),
                None => cursor,
            };
            // The longest this clip can run if it is open-ended: an explicit
            // duration, or the rest of the frame.
            let explicit_len = clip
                .duration
                .map(|d| self.time(d, &cpath.key("duration"), "duration"));
            let open_hint =
                explicit_len.or_else(|| output_duration.map(|d| (d - start).max(Ratio::ZERO)));
            let (source, length) =
                self.resolve_source(&clip.source, &cpath, assets, frame, open_hint);
            let mut transition_in = None;
            if let Some(tr) = &clip.transition {
                let tdur = self.time(
                    tr.duration,
                    &cpath.key("transition").key("duration"),
                    "transition duration",
                );
                if i == 0 {
                    self.push(
                        Diagnostic::warning("W303", cpath.key("transition"), format!("{clip_id} is the first clip in {id}; its transition has nothing to blend from"))
                            .with_help("remove the transition or use an opacity keyframe for a fade from the background"),
                    );
                } else {
                    if clip.start.is_none() {
                        start = start - tdur;
                    }
                    transition_in = Some((tr.kind, tdur));
                }
            }

            let length = match (explicit_len, clip.duration) {
                (Some(secs), Some(d)) => {
                    if let ClipLength::Fixed(src_len) = length {
                        if secs > src_len {
                            self.push(
                                Diagnostic::error("E301", cpath.key("duration"), format!("{clip_id} lasts {secs}s but its source range is only {src_len}s"))
                                    .with_value(json!(d.to_string()))
                                    .with_help("shorten the duration or widen the in/out range"),
                            );
                        }
                    }
                    secs
                }
                _ => match length {
                    ClipLength::Fixed(l) => l,
                    ClipLength::Open => match output_duration {
                        Some(d) if d > start => {
                            any_open = true;
                            d - start
                        }
                        Some(_) => Ratio::ZERO,
                        None => {
                            self.push(
                                Diagnostic::error("E305", cpath.clone(), format!("the length of {clip_id} cannot be determined"))
                                    .with_help("set \"duration\" on the clip, or set output.duration so open-ended clips know where to stop"),
                            );
                            Ratio::ZERO
                        }
                    },
                },
            };
            if length <= Ratio::ZERO && clip.duration.is_some() {
                self.push(
                    Diagnostic::error(
                        "E301",
                        cpath.key("duration"),
                        format!("{clip_id} has a duration of {length}s"),
                    )
                    .with_help("duration must be greater than 0"),
                );
            }
            let end = start + length;

            if let Some(prev) = clips.last() {
                let overlap = prev.end - start;
                if overlap > Ratio::ZERO {
                    match transition_in {
                        Some((_, tdur)) if overlap <= tdur => {
                            if overlap < tdur {
                                self.push(
                                    Diagnostic::error("E306", cpath.key("transition").key("duration"), format!("{clip_id} needs {tdur}s of overlap for its transition but {} only covers {overlap}s of it", prev.id))
                                        .with_help(format!("shorten the transition to {overlap}s or lengthen the previous clip")),
                                );
                            }
                        }
                        _ => {
                            self.push(
                                Diagnostic::error("E302", cpath.key("start"), format!("{clip_id} starts at {start}s, before {} ends at {}s", prev.id, prev.end))
                                    .with_value(json!(format!("{start}s")))
                                    .with_help(format!("start it at {}s or later, move it to another layer, or add a transition to blend the overlap", prev.end)),
                            );
                        }
                    }
                }
                if let Some((_, tdur)) = transition_in {
                    if start < prev.start {
                        self.push(
                            Diagnostic::error(
                                "E306",
                                cpath.key("transition").key("duration"),
                                format!("the transition into {clip_id} is longer than {}", prev.id),
                            )
                            .with_help(format!(
                                "use a transition no longer than {}s",
                                prev.duration().min(tdur)
                            )),
                        );
                    }
                }
            }

            let width = f64::from(frame.width);
            let height = f64::from(frame.height);
            let transform = clip.transform.clone().unwrap_or_default();
            let tpath = cpath.key("transform");
            let (anchor, position, scale, rotation) =
                self.resolve_transform(&transform, &tpath, width, height, length);
            let opacity = self.track_f64(
                clip.opacity.as_ref(),
                &cpath.key("opacity"),
                1.0,
                length,
                Some((0.0, 1.0)),
                "opacity",
            );
            if let Some(crop) = &clip.crop {
                self.check_crop(crop, &cpath.key("crop"));
            }

            clips.push(ResolvedClip {
                id: clip_id,
                path: cpath.pointer(),
                start,
                end,
                source,
                crop: clip.crop,
                fit: clip.fit.unwrap_or(match clip.source {
                    Source::Video { .. } => Fit::Contain,
                    _ => Fit::None,
                }),
                blend: clip.blend.unwrap_or_default(),
                anchor,
                position,
                scale,
                rotation,
                opacity,
                transition_in,
            });
            cursor = end;
        }
        (ResolvedLayer { id, clips }, any_open)
    }

    /// Resolves a clip's source. `frame` is the frame percentages refer to
    /// and `open_hint` the length an open-ended source would get, which a
    /// nested composition needs to close its own open-ended clips.
    fn resolve_source(
        &mut self,
        source: &Source,
        cpath: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        frame: FrameSize,
        open_hint: Option<Ratio>,
    ) -> (ResolvedSource, ClipLength) {
        let spath = cpath.key("source");
        match source {
            Source::Video {
                asset,
                in_,
                out,
                audio,
            } => {
                self.asset_ref(asset, &spath.key("asset"), assets, &[AssetKind::Video]);
                let (in_secs, length) = self.source_range(*in_, *out, &spath, asset, assets);
                (
                    ResolvedSource::Video {
                        asset: asset.clone(),
                        in_: in_secs,
                        audio: audio.unwrap_or(true),
                    },
                    length,
                )
            }
            Source::Image { asset } => {
                self.asset_ref(asset, &spath.key("asset"), assets, &[AssetKind::Image]);
                (
                    ResolvedSource::Image {
                        asset: asset.clone(),
                    },
                    ClipLength::Open,
                )
            }
            Source::Solid { color } => {
                let color = self.track_color(Some(color), &spath.key("color"), Color::WHITE);
                (ResolvedSource::Solid { color }, ClipLength::Open)
            }
            Source::Shape {
                shape,
                width,
                height,
                fill,
                stroke,
                radius,
            } => {
                let w = self.positive_length(
                    *width,
                    &spath.key("width"),
                    f64::from(frame.width),
                    "width",
                );
                let h = self.positive_length(
                    *height,
                    &spath.key("height"),
                    f64::from(frame.height),
                    "height",
                );
                let fill = self.track_color(fill.as_ref(), &spath.key("fill"), Color::WHITE);
                let stroke = stroke.as_ref().map(|s| {
                    if s.width < 0.0 {
                        self.push(
                            Diagnostic::error(
                                "E402",
                                spath.key("stroke").key("width"),
                                "stroke width must not be negative",
                            )
                            .with_value(s.width),
                        );
                    }
                    (s.color.0.to_linear(), s.width.max(0.0))
                });
                let radius = radius.unwrap_or(0.0);
                if radius < 0.0 {
                    self.push(
                        Diagnostic::error(
                            "E402",
                            spath.key("radius"),
                            "corner radius must not be negative",
                        )
                        .with_value(radius),
                    );
                }
                (
                    ResolvedSource::Shape {
                        kind: *shape,
                        width: w,
                        height: h,
                        fill,
                        stroke,
                        radius: radius.max(0.0),
                    },
                    ClipLength::Open,
                )
            }
            Source::Text(text) => {
                let resolved = self.resolve_text(text, &spath, assets, frame);
                (ResolvedSource::Text(Box::new(resolved)), ClipLength::Open)
            }
            Source::Composition { composition } => {
                self.resolve_composition(composition, &spath, assets, open_hint)
            }
        }
    }

    fn resolve_composition(
        &mut self,
        name: &str,
        spath: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        open_hint: Option<Ratio>,
    ) -> (ResolvedSource, ClipLength) {
        let ref_path = spath.key("composition");
        let placeholder = |name: &str| {
            ResolvedSource::Composition(Box::new(ResolvedComposition {
                name: name.to_owned(),
                width: 1,
                height: 1,
                background: Color::TRANSPARENT,
                layers: Vec::new(),
            }))
        };
        let Some(def): Option<&CompositionDef> = self.tl.compositions.get(name) else {
            let mut d =
                Diagnostic::error("E206", ref_path, format!("unknown composition {name:?}"))
                    .with_value(name);
            let known: Vec<&str> = self.tl.compositions.keys().map(String::as_str).collect();
            d = match nearest(name, known.iter().copied()) {
                Some(near) => d.with_help(format!(
                    "did you mean {near:?}? compositions are declared under \"compositions\""
                )),
                None if known.is_empty() => d.with_help("declare it under \"compositions\""),
                None => d.with_help(format!(
                    "declared compositions: {}",
                    known
                        .iter()
                        .map(|k| format!("{k:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            };
            self.push(d);
            return (placeholder(name), ClipLength::Open);
        };
        if self.composition_stack.iter().any(|n| n == name) {
            let chain = self
                .composition_stack
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(name))
                .map(|n| format!("{n:?}"))
                .collect::<Vec<_>>()
                .join(" -> ");
            self.push(
                Diagnostic::error(
                    "E207",
                    ref_path,
                    format!("composition {name:?} contains itself ({chain})"),
                )
                .with_help(
                    "a composition cannot include itself, directly or through another composition",
                ),
            );
            return (placeholder(name), ClipLength::Open);
        }
        if self.composition_stack.len() >= MAX_COMPOSITION_DEPTH {
            self.push(
                Diagnostic::error(
                    "E207",
                    ref_path,
                    format!(
                        "compositions are nested more than {MAX_COMPOSITION_DEPTH} levels deep"
                    ),
                )
                .with_help("flatten the structure"),
            );
            return (placeholder(name), ClipLength::Open);
        }
        self.used_compositions.insert(name.to_owned());
        let cpath = Path::root().key("compositions").key(name);
        for (field, v) in [("width", def.width), ("height", def.height)] {
            if v == 0 {
                self.push(
                    Diagnostic::error(
                        "E402",
                        cpath.key(field),
                        format!("composition {field} must be greater than 0"),
                    )
                    .with_value(v),
                );
            }
        }
        let frame = FrameSize {
            width: def.width.max(1),
            height: def.height.max(1),
        };
        self.composition_stack.push(name.to_owned());
        let inner =
            self.resolve_layers(&def.layers, &cpath.key("layers"), assets, frame, open_hint);
        self.composition_stack.pop();
        let natural = inner
            .layers
            .iter()
            .flat_map(|l| l.clips.iter().map(|c| c.end))
            .max()
            .unwrap_or(Ratio::ZERO);
        let length = if inner.any_open || natural <= Ratio::ZERO {
            ClipLength::Open
        } else {
            ClipLength::Fixed(natural)
        };
        let resolved = ResolvedComposition {
            name: name.to_owned(),
            width: frame.width,
            height: frame.height,
            background: def.background.map_or(Color::TRANSPARENT, |c| c.0),
            layers: inner.layers,
        };
        (ResolvedSource::Composition(Box::new(resolved)), length)
    }

    /// Resolves an in/out range, returning the in point and the length when
    /// it can be known: from `out`, or from the file's duration when the
    /// asset information provides one.
    fn source_range(
        &mut self,
        in_: Option<Time>,
        out: Option<Time>,
        spath: &Path,
        asset_id: &str,
        assets: &BTreeMap<String, ResolvedAsset>,
    ) -> (Ratio, ClipLength) {
        let in_secs = in_.map_or(Ratio::ZERO, |t| self.time(t, &spath.key("in"), "in"));
        let file_len = assets
            .get(asset_id)
            .and_then(|a| self.info.duration(asset_id, &a.src));
        match out {
            Some(o) => {
                let out_secs = self.time(o, &spath.key("out"), "out");
                if out_secs <= in_secs {
                    self.push(
                        Diagnostic::error(
                            "E301",
                            spath.key("out"),
                            format!("out ({out_secs}s) must be after in ({in_secs}s)"),
                        )
                        .with_value(json!(o.to_string())),
                    );
                    return (in_secs, ClipLength::Fixed(Ratio::ZERO));
                }
                if let Some(len) = file_len {
                    if out_secs > len {
                        self.push(
                            Diagnostic::warning(
                                "W305",
                                spath.key("out"),
                                format!("out ({out_secs}s) is past the end of the file ({len}s)"),
                            )
                            .with_value(json!(o.to_string()))
                            .with_help("the clip will end where the file ends"),
                        );
                        return (in_secs, ClipLength::Fixed((len - in_secs).max(Ratio::ZERO)));
                    }
                }
                (in_secs, ClipLength::Fixed(out_secs - in_secs))
            }
            None => match file_len {
                Some(len) if in_secs >= len => {
                    self.push(
                        Diagnostic::error(
                            "E301",
                            spath.key("in"),
                            format!("in ({in_secs}s) is at or after the end of the file ({len}s)"),
                        )
                        .with_value(json!(in_secs.to_string())),
                    );
                    (in_secs, ClipLength::Fixed(Ratio::ZERO))
                }
                Some(len) => (in_secs, ClipLength::Fixed(len - in_secs)),
                None => (in_secs, ClipLength::Open),
            },
        }
    }

    /// A crop's edges must lie inside the source and its size must be
    /// positive; what that means in pixels depends on the source, so the
    /// checks are on the values as written.
    fn check_crop(&mut self, crop: &Crop, path: &Path) {
        let edges = [("x", crop.x), ("y", crop.y)];
        for (name, v) in edges {
            let bad = match v {
                Some(Length::Px(px)) => !(px >= 0.0 && px.is_finite()),
                Some(Length::Percent(p)) => !(0.0..100.0).contains(&p),
                None => false,
            };
            if bad {
                self.push(
                    Diagnostic::error(
                        "E402",
                        path.key(name),
                        format!("crop {name} must be inside the source (0 or more, below 100%)"),
                    )
                    .with_value(json!(v.map(|l| l.to_string()))),
                );
            }
        }
        let sizes = [("width", crop.width), ("height", crop.height)];
        for (name, v) in sizes {
            let bad = match v {
                Some(Length::Px(px)) => !(px > 0.0 && px.is_finite()),
                Some(Length::Percent(p)) => !(p > 0.0 && p <= 100.0),
                None => false,
            };
            if bad {
                self.push(
                    Diagnostic::error(
                        "E402",
                        path.key(name),
                        format!("crop {name} must be greater than 0 (and at most 100%)"),
                    )
                    .with_value(json!(v.map(|l| l.to_string()))),
                );
            }
        }
    }

    fn positive_length(&mut self, len: Length, path: &Path, reference: f64, what: &str) -> f64 {
        let px = len.to_px(reference);
        if px.partial_cmp(&0.0) != Some(Ordering::Greater) {
            self.push(
                Diagnostic::error(
                    "E402",
                    path.clone(),
                    format!("{what} must be greater than 0"),
                )
                .with_value(json!(len.to_string())),
            );
        }
        px
    }

    fn resolve_text(
        &mut self,
        text: &TextSource,
        spath: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        frame: FrameSize,
    ) -> ResolvedText {
        // A `font` shorthand becomes its fields first, so the checks below
        // and the renderer see the same style.
        let mut expanded = text.clone();
        if let Err(e) = crate::css::expand_font(&mut expanded.style, &mut expanded.line_height) {
            self.push(
                Diagnostic::error("E103", spath.key("font"), e)
                    .with_help("a font is a family name, a font asset id, or \"[italic] [weight] <size>[/<line-height>] <family>\""),
            );
        }
        if let Some(h) = expanded.highlight.as_mut() {
            let mut ignored = None;
            if let Err(e) = crate::css::expand_font(h, &mut ignored) {
                self.push(Diagnostic::error(
                    "E103",
                    spath.key("highlight").key("font"),
                    e,
                ));
            }
        }
        let text = &expanded;
        let mut words = Vec::new();
        if let Some(list) = &text.words {
            let wpath = spath.key("words");
            let mut prev_end = Ratio::ZERO;
            for (i, w) in list.iter().enumerate() {
                let p = wpath.index(i);
                let start = self.time(w.start, &p.key("start"), "word start");
                let end = self.time(w.end, &p.key("end"), "word end");
                if end <= start {
                    self.push(
                        Diagnostic::error(
                            "E411",
                            p.key("end"),
                            format!(
                                "word {:?} ends at {end}s, not after it starts at {start}s",
                                w.text
                            ),
                        )
                        .with_value(json!(w.end.to_string())),
                    );
                }
                if start < prev_end {
                    self.push(
                        Diagnostic::error("E411", p.key("start"), format!("word {:?} starts at {start}s, before the previous word ends at {prev_end}s", w.text))
                            .with_value(json!(w.start.to_string()))
                            .with_help("words must be in time order and must not overlap"),
                    );
                }
                prev_end = end.max(prev_end);
                words.push((w.text.clone(), start, end));
            }
        }
        let content = match (&text.text, &text.words) {
            (Some(t), _) => t.clone(),
            (None, Some(list)) if !list.is_empty() => list
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            _ => {
                self.push(
                    Diagnostic::error(
                        "E405",
                        spath.clone(),
                        "text source has neither \"text\" nor \"words\"",
                    )
                    .with_help("add \"text\": \"...\" or a non-empty \"words\" list"),
                );
                String::new()
            }
        };
        for (style, p) in [
            (&text.style, spath.clone()),
            (
                text.highlight.as_ref().unwrap_or(&text.style),
                spath.key("highlight"),
            ),
        ] {
            if let Some(font) = &style.font {
                if assets.contains_key(font) {
                    self.asset_ref(font, &p.key("font"), assets, &[AssetKind::Font]);
                }
            }
            if let Some(size) = style.size {
                if size.partial_cmp(&0.0) != Some(Ordering::Greater) {
                    self.push(
                        Diagnostic::error(
                            "E402",
                            p.key("size"),
                            "font size must be greater than 0",
                        )
                        .with_value(size),
                    );
                }
            }
            if let Some(w) = style.weight {
                if !(100..=900).contains(&w) {
                    self.push(
                        Diagnostic::error(
                            "E402",
                            p.key("weight"),
                            format!("font weight {w} is outside 100..=900"),
                        )
                        .with_value(w)
                        .with_help("use 400 for regular and 700 for bold"),
                    );
                }
            }
        }
        let width = f64::from(frame.width);
        let max_width = text.max_width.map_or(width, |m| {
            self.positive_length(m, &spath.key("max_width"), width, "max_width")
        });
        ResolvedText {
            text: content,
            words,
            max_width,
            spec: text.clone(),
        }
    }

    fn resolve_transform(
        &mut self,
        t: &Transform,
        tpath: &Path,
        width: f64,
        height: f64,
        length: Ratio,
    ) -> ResolvedTransform {
        let anchor = t.anchor.unwrap_or(Point {
            x: Length::Percent(50.0),
            y: Length::Percent(50.0),
        });
        let default_pos = [width / 2.0, height / 2.0];
        let position = self.track(
            t.position.as_ref(),
            &tpath.key("position"),
            default_pos,
            length,
            |p: &Point| p.to_px(width, height),
        );
        let scale = self.track(
            t.scale.as_ref(),
            &tpath.key("scale"),
            [1.0, 1.0],
            length,
            |s: &Scale| s.to_array(),
        );
        let rotation = self.track_f64(
            t.rotation.as_ref(),
            &tpath.key("rotation"),
            0.0,
            length,
            None,
            "rotation",
        );
        (anchor, position, scale, rotation)
    }

    fn track_f64(
        &mut self,
        a: Option<&Animated<f64>>,
        path: &Path,
        default: f64,
        length: Ratio,
        range: Option<(f64, f64)>,
        what: &str,
    ) -> Track<f64> {
        if let (Some(a), Some((lo, hi))) = (a, range) {
            let bad: Vec<f64> = a
                .values()
                .copied()
                .filter(|v| !(lo..=hi).contains(v) || !v.is_finite())
                .collect();
            if let Some(v) = bad.first() {
                let p = match a {
                    Animated::Constant(_) => path.clone(),
                    Animated::Keyframes(k) => {
                        let i = k.iter().position(|k| k.v == *v).unwrap_or(0);
                        path.key("keyframes").index(i).key("v")
                    }
                };
                self.push(
                    Diagnostic::error("E402", p, format!("{what} {v} is outside {lo}..={hi}"))
                        .with_value(*v),
                );
            }
        }
        self.track(a, path, default, length, |v: &f64| *v)
    }

    fn track_color(
        &mut self,
        a: Option<&Animated<ColorValue>>,
        path: &Path,
        default: Color,
    ) -> Track<LinearRgba> {
        self.track(
            a,
            path,
            default.to_linear(),
            Ratio::ZERO,
            |c: &ColorValue| c.0.to_linear(),
        )
    }

    /// Builds a track from an animated property, checking keyframe order,
    /// easing validity and range against the clip length.
    fn track<T, U: geneva_anim::Interpolate>(
        &mut self,
        a: Option<&Animated<T>>,
        path: &Path,
        default: U,
        length: Ratio,
        map: impl Fn(&T) -> U,
    ) -> Track<U> {
        match a {
            None => Track::constant(default),
            Some(Animated::Constant(v)) => Track::constant(map(v)),
            Some(Animated::Keyframes(keys)) => {
                let kpath = path.key("keyframes");
                let mut out = Vec::with_capacity(keys.len());
                let mut prev: Option<Ratio> = None;
                for (i, k) in keys.iter().enumerate() {
                    let p = kpath.index(i);
                    let t = self.time(k.t, &p.key("t"), "keyframe time");
                    if let Some(pt) = prev {
                        if t <= pt {
                            self.push(
                                Diagnostic::error(
                                    "E303",
                                    p.key("t"),
                                    format!(
                                        "keyframe {i} at {t}s is not after keyframe {} at {pt}s",
                                        i - 1
                                    ),
                                )
                                .with_value(json!(k.t.to_string()))
                                .with_help("keyframes must be in strictly increasing time order"),
                            );
                        }
                    }
                    if length > Ratio::ZERO && t > length {
                        self.push(
                            Diagnostic::warning("W300", p.key("t"), format!("keyframe {i} at {t}s is after the clip ends at {length}s"))
                                .with_help("keyframe times are relative to the clip start; the value will never be reached"),
                        );
                    }
                    if let Some(reason) = k.ease.as_ref().and_then(Easing::validate) {
                        self.push(Diagnostic::error("E401", p.key("ease"), reason));
                    }
                    prev = Some(t);
                    out.push(Keyframe {
                        time: t.to_f64(),
                        value: map(&k.v),
                        easing: k.ease.unwrap_or_default(),
                    });
                }
                let first = out[0].value.clone();
                Track::new(out).unwrap_or_else(|| Track::constant(first))
            }
        }
    }

    fn resolve_audio_track(
        &mut self,
        track: &AudioTrack,
        index: usize,
        path: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
    ) -> ResolvedAudioTrack {
        let id = track
            .id
            .clone()
            .unwrap_or_else(|| format!("audio track {index}"));
        if track.clips.is_empty() {
            self.push(Diagnostic::warning(
                "W302",
                path.key("clips"),
                format!("{id} has no clips"),
            ));
        }
        let mut clips: Vec<ResolvedAudioClip> = Vec::new();
        let mut cursor = Ratio::ZERO;
        for (i, clip) in track.clips.iter().enumerate() {
            let cpath = path.key("clips").index(i);
            let clip_id = clip
                .id
                .clone()
                .unwrap_or_else(|| format!("clip {i} of {id}"));
            self.asset_ref(
                &clip.asset,
                &cpath.key("asset"),
                assets,
                &[AssetKind::Audio, AssetKind::Video],
            );
            let (in_secs, src_len) =
                self.source_range(clip.in_, clip.out, &cpath, &clip.asset, assets);
            let start = clip
                .start
                .map_or(cursor, |t| self.time(t, &cpath.key("start"), "start"));
            let length = match (clip.duration, src_len) {
                (Some(d), _) => self.time(d, &cpath.key("duration"), "duration"),
                (None, ClipLength::Fixed(l)) => l,
                (None, ClipLength::Open) => {
                    self.push(
                        Diagnostic::error(
                            "E305",
                            cpath.clone(),
                            format!("the length of {clip_id} cannot be determined"),
                        )
                        .with_help("set \"duration\" or \"out\" on the clip"),
                    );
                    Ratio::ZERO
                }
            };
            if length <= Ratio::ZERO && clip.duration.is_some() {
                self.push(
                    Diagnostic::error(
                        "E301",
                        cpath.key("duration"),
                        format!("{clip_id} has a duration of {length}s"),
                    )
                    .with_help("duration must be greater than 0"),
                );
            }
            let end = start + length;
            if let Some(prev) = clips.last() {
                if start < prev.end {
                    self.push(
                        Diagnostic::error(
                            "E302",
                            cpath.key("start"),
                            format!(
                                "{clip_id} starts at {start}s, before {} ends at {}s",
                                prev.id, prev.end
                            ),
                        )
                        .with_value(json!(format!("{start}s")))
                        .with_help(format!(
                            "start it at {}s or later, or move it to another audio track",
                            prev.end
                        )),
                    );
                }
            }
            let gain_db = self.track_f64(
                clip.gain_db.as_ref(),
                &cpath.key("gain_db"),
                0.0,
                length,
                None,
                "gain",
            );
            let fade_in = clip.fade_in.map_or(Ratio::ZERO, |t| {
                self.time(t, &cpath.key("fade_in"), "fade_in")
            });
            let fade_out = clip.fade_out.map_or(Ratio::ZERO, |t| {
                self.time(t, &cpath.key("fade_out"), "fade_out")
            });
            if fade_in + fade_out > length && length > Ratio::ZERO {
                self.push(
                    Diagnostic::warning(
                        "W304",
                        cpath.key("fade_in"),
                        format!(
                            "fades on {clip_id} add up to {}s, longer than the clip ({length}s)",
                            fade_in + fade_out
                        ),
                    )
                    .with_help("the fades will overlap; shorten one of them"),
                );
            }
            clips.push(ResolvedAudioClip {
                id: clip_id,
                asset: clip.asset.clone(),
                in_: in_secs,
                start,
                end,
                gain_db,
                fade_in,
                fade_out,
            });
            cursor = end;
        }
        ResolvedAudioTrack { id, clips }
    }
}

/// Finds the closest name within a small edit distance, for "did you mean".
fn nearest<'a>(needle: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        let d = levenshtein(needle, c);
        let limit = needle.len().max(c.len()) / 3 + 1;
        if d <= limit && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}
