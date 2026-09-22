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
use crate::animation::{Animation, Shift, Values as AnimValues, parse_animation_spec};
use crate::color::ColorValue;
use crate::diagnostic::{Diagnostic, Path};
use crate::length::{Length, Point, Scale};
use crate::motion::{NodeMotion, Play};
use crate::ratio::Ratio;
use crate::schema::{
    ACCEPTED_VERSIONS, Asset, AssetKind, AudioOutput, AudioTrack, BlendMode, BoxSize,
    CaptionPosition, Clip, CompositionDef, Crop, Effect, Encode, FORMAT_VERSION, Fit, Layer,
    Loudness, Mask, OutputKind, ShapeKind, Source, TextAlign, TextFill, TextSource, TextStyle,
    Timeline, Transform, Transition, TransitionKind, VideoCodec, Word,
};
use crate::time::Time;
use geneva_html::AnimationSpec;

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
    /// The files of a multi-output render, in name order; empty for a
    /// single-output document.
    pub outputs: Vec<ResolvedOutput>,
}

/// One entry of `outputs`, with its size and times settled.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedOutput {
    /// The entry's key.
    pub name: String,
    /// What it writes.
    pub kind: OutputKind,
    /// File name relative to the output directory.
    pub path: String,
    /// Picture width: the rendition's, the poster's, or one sprite tile's.
    pub width: u32,
    /// Picture height, likewise.
    pub height: u32,
    /// Encoder settings, as written on the entry or inherited.
    pub encode: Option<Encode>,
    /// Audio format, as written on the entry or inherited.
    pub audio: Option<AudioOutput>,
    /// Poster time in seconds; `None` picks a frame.
    pub at: Option<Ratio>,
    /// Sprite interval in seconds.
    pub every: Option<Ratio>,
    /// Sprite tiles per row.
    pub columns: Option<u32>,
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
    /// Transition from the previous clip, or the one that opens the layer.
    pub transition_in: Option<ResolvedTransition>,
    /// Transition that closes the layer, when nothing follows this clip.
    pub transition_out: Option<ResolvedTransition>,
    /// Effects on the placed picture, in order.
    pub effects: Vec<ResolvedEffect>,
    /// The mask, if any.
    pub mask: Option<ResolvedMask>,
    /// How fast the source plays; 1 is natural speed.
    pub speed: Ratio,
}

/// A transition into a clip, with its length in seconds and the color a
/// `fade` passes through.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTransition {
    /// Which transition.
    pub kind: TransitionKind,
    /// How long it lasts: the overlap between a pair of clips, or the
    /// ramp at the head or tail of a layer.
    pub duration: Ratio,
    /// Premultiplied linear color for `fade`; unused by `crossfade`.
    pub color: LinearRgba,
    /// Shape of the ramp.
    pub ease: Easing,
    /// Whether there is a clip on the other side. A pair hands over in
    /// the middle of the overlap; an edge ramps across the whole of it.
    pub paired: bool,
}

impl ResolvedTransition {
    /// The eased ramp `u` in 0 to 1, clamped.
    fn ramp(&self, u: f64) -> f64 {
        self.ease.evaluate(u.clamp(0.0, 1.0)).clamp(0.0, 1.0)
    }

    /// Progress through the transition, 0 at its start and 1 at its end.
    fn progress(&self, local: Ratio) -> Option<f64> {
        (self.duration > Ratio::ZERO && local < self.duration)
            .then(|| (local / self.duration).to_f64().max(0.0))
    }

    /// The opacity of the clip arriving, `local` seconds in.
    pub fn incoming(&self, local: Ratio) -> f64 {
        let Some(u) = self.progress(local) else {
            return 1.0;
        };
        // Only a fade between two clips waits, letting the one leaving
        // reach the color before this one starts. Everything else ramps
        // across the whole transition.
        if self.paired && self.kind == TransitionKind::Fade {
            self.ramp((u - 0.5) * 2.0)
        } else {
            self.ramp(u)
        }
    }

    /// The opacity of the clip leaving, `left` seconds before it ends.
    pub fn outgoing(&self, left: Ratio) -> f64 {
        let Some(u) = self.progress(left) else {
            return 1.0;
        };
        match (self.paired, self.kind) {
            // The clip arriving covers this one, and fading both would dip
            // the picture towards the background.
            (true, TransitionKind::Crossfade) => 1.0,
            // Down to the color over the first half of the overlap, which
            // is the second half of what is left.
            (true, TransitionKind::Fade) => self.ramp((u - 0.5) * 2.0),
            // Closing a layer, either kind takes the whole ramp.
            _ => self.ramp(u),
        }
    }

    /// How opaque the dip color is, `local` seconds in. Between clips it
    /// peaks in the middle and is clear at both ends; at the head or tail
    /// of a layer it is full at the edge and clear inside. A crossfade
    /// never shows a color at all.
    pub fn veil(&self, local: Ratio) -> f64 {
        if self.kind != TransitionKind::Fade {
            return 0.0;
        }
        let Some(u) = self.progress(local) else {
            return 0.0;
        };
        if self.paired {
            // Up over the first half, down over the second.
            self.ramp((u * 2.0).min((1.0 - u) * 2.0))
        } else {
            // Full where the layer ends, clear where the picture is.
            self.ramp(1.0 - u)
        }
    }
}

/// A mask with its values checked; lengths stay as written, since the
/// clip's box size is only known when the source is.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMask {
    /// The shape, when the mask is not an image.
    pub shape: ShapeKind,
    /// The shape's box: left, top, width, height in the clip's box.
    pub rect: [Option<Length>; 4],
    /// Corner radius of a rectangle, in pixels of the clip's box.
    pub radius: f64,
    /// Soft edge width in pixels of the clip's box.
    pub feather: f64,
    /// The luma image asset, when the mask is an image.
    pub asset: Option<String>,
    /// Whether the coverage is inverted.
    pub invert: bool,
}

impl ResolvedMask {
    /// The shape's box `[x, y, width, height]` in pixels of a `w`×`h`
    /// clip box.
    pub fn rect_px(&self, w: f64, h: f64) -> [f64; 4] {
        let [x, y, width, height] = self.rect;
        [
            x.map_or(0.0, |v| v.to_px(w)),
            y.map_or(0.0, |v| v.to_px(h)),
            width.map_or(w, |v| v.to_px(w)),
            height.map_or(h, |v| v.to_px(h)),
        ]
    }
}

/// An effect with its parameters sampleable over clip-local time.
#[derive(Debug, Clone)]
pub enum ResolvedEffect {
    /// A Gaussian blur with its standard deviation in output pixels.
    Blur(Track<f64>),
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
    /// A box of markup, already parsed and styled; only layout and paint
    /// are left, and neither depends on time.
    Html(Box<ResolvedHtml>),
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

/// A shadow's animatable parts as tracks over clip-local time.
#[derive(Debug, Clone)]
pub struct ShadowTrack {
    /// Premultiplied linear colour.
    pub color: Track<LinearRgba>,
    /// Horizontal offset in pixels.
    pub x: Track<f64>,
    /// Vertical offset in pixels.
    pub y: Track<f64>,
    /// Blur radius in pixels.
    pub blur: Track<f64>,
    /// The furthest the shadow reaches past the glyphs at any moment:
    /// the largest blur plus the largest offset. The renderer sizes its
    /// image by this, so the glyphs stay put while the shadow moves.
    pub reach: f64,
}

/// A gradient over the glyphs, with its tile's offset as tracks over
/// clip-local time.
#[derive(Debug, Clone)]
pub struct FillTrack {
    /// The colour or gradient.
    pub background: geneva_html::Background,
    /// The tile's width in pixels; the text's own when `None`.
    pub width: Option<f64>,
    /// The tile's height in pixels; the text's own when `None`.
    pub height: Option<f64>,
    /// Where the tile starts, in pixels from the left of the text's box.
    pub x: Track<f64>,
    /// Where the tile starts, in pixels from the top of the text's box.
    pub y: Track<f64>,
}

impl FillTrack {
    /// A fill whose tile stays at `(x, y)`.
    #[must_use]
    pub fn constant(
        background: geneva_html::Background,
        size: (Option<f64>, Option<f64>),
        x: f64,
        y: f64,
    ) -> Self {
        Self {
            background,
            width: size.0,
            height: size.1,
            x: Track::constant(x),
            y: Track::constant(y),
        }
    }

    /// Whether the tile stays put over the clip.
    #[must_use]
    pub fn is_constant(&self) -> bool {
        self.x.is_constant() && self.y.is_constant()
    }
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
    /// Fill colour over clip-local time.
    pub color: Track<LinearRgba>,
    /// The highlight's colour, when there is a highlight.
    pub highlight_color: Option<Track<LinearRgba>>,
    /// A gradient over the glyphs, in place of the colour.
    pub fill: Option<FillTrack>,
    /// The highlight's gradient. A highlight keeps the base fill unless
    /// it sets a `color` or a `fill` of its own.
    pub highlight_fill: Option<FillTrack>,
    /// The shadows, front to back as listed; empty for none.
    pub shadow: Vec<ShadowTrack>,
}

impl ResolvedText {
    /// A text whose style is taken as written, for specs built in code
    /// rather than read from a document. A keyframed value here is taken
    /// at its first keyframe, since there is no resolver to check it.
    #[must_use]
    pub fn constant(text: String, spec: TextSource, max_width: f64) -> Self {
        fn first<T: Clone>(a: &Animated<T>) -> T {
            match a {
                Animated::Constant(v) => v.clone(),
                Animated::Keyframes(k) => k[0].v.clone(),
            }
        }
        let color_of = |a: Option<&Animated<ColorValue>>, fallback: Color| {
            Track::constant(a.map_or(fallback, |a| first(a).0).to_linear())
        };
        let color = color_of(spec.style.color.as_ref(), Color::WHITE);
        let highlight_color = spec
            .highlight
            .as_ref()
            .map(|h| color_of(h.color.as_ref().or(spec.style.color.as_ref()), Color::WHITE));
        let shadow = spec.shadow.as_ref().map_or_else(Vec::new, |list| {
            list.0
                .iter()
                .map(|sh| {
                    let (x, y, blur) = (first(&sh.x), first(&sh.y), first(&sh.blur).max(0.0));
                    ShadowTrack {
                        color: color_of(sh.color.as_ref(), SHADOW_DEFAULT),
                        x: Track::constant(x),
                        y: Track::constant(y),
                        blur: Track::constant(blur),
                        reach: blur + x.abs().max(y.abs()),
                    }
                })
                .collect()
        });
        // A gradient that does not parse is dropped here; the resolver
        // is where it is named.
        let fill_of = |f: &TextFill| {
            geneva_html::style::background(&f.gradient)
                .ok()
                .map(|background| FillTrack {
                    background,
                    width: f.width.filter(|w| *w > 0.0),
                    height: f.height.filter(|h| *h > 0.0),
                    x: Track::constant(first(&f.x)),
                    y: Track::constant(first(&f.y)),
                })
        };
        let fill = spec.style.fill.as_ref().and_then(fill_of);
        let highlight_fill = spec
            .highlight
            .as_ref()
            .and_then(|h| highlight_fill_spec(h, &spec.style))
            .and_then(fill_of);
        Self {
            text,
            words: Vec::new(),
            max_width,
            spec,
            color,
            highlight_color,
            fill,
            highlight_fill,
            shadow,
        }
    }

    /// Whether nothing in the style changes over the clip, so one
    /// rendering can stand for every frame.
    #[must_use]
    pub fn is_static(&self) -> bool {
        self.color.is_constant()
            && self.highlight_color.as_ref().is_none_or(Track::is_constant)
            && self.fill.as_ref().is_none_or(FillTrack::is_constant)
            && self
                .highlight_fill
                .as_ref()
                .is_none_or(FillTrack::is_constant)
            && self.shadow.iter().all(|s| {
                s.color.is_constant()
                    && s.x.is_constant()
                    && s.y.is_constant()
                    && s.blur.is_constant()
            })
    }
}

/// The fill a highlight draws with: its own, or the base style's when it
/// sets neither a fill nor a colour. A highlight that names a colour
/// means that colour, so the base gradient does not paint over it.
fn highlight_fill_spec<'a>(h: &'a TextStyle, base: &'a TextStyle) -> Option<&'a TextFill> {
    h.fill.as_ref().or_else(|| {
        if h.color.is_none() {
            base.fill.as_ref()
        } else {
            None
        }
    })
}

/// What a shadow is when it names no colour: half-transparent black.
const SHADOW_DEFAULT: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.5,
};

/// Markup after parsing and styling. Layout and painting are left to the
/// renderer, which is where text can be measured.
#[derive(Debug, Clone)]
pub struct ResolvedHtml {
    /// The markup, for a renderer that has to prepare it again.
    pub html: String,
    /// The stylesheet applied after any `<style>` in the markup.
    pub css: String,
    /// The directory the markup's own paths are relative to, under the
    /// asset root: the markup file's directory, or the root itself for
    /// markup written in the document.
    pub base: String,
    /// The linked stylesheets, by href, read while resolving so that a
    /// missing one is an error before anything is drawn.
    pub linked: BTreeMap<String, String>,
    /// Box width in pixels, or `None` to fit the content.
    pub width: Option<f64>,
    /// Box height in pixels, or `None` to fit the content.
    pub height: Option<f64>,
    /// The animations on elements inside the outermost one, which the
    /// renderer plays by compositing each such element as a group. The
    /// outermost element's own animation is the clip's and is not here.
    pub motion: Vec<NodeMotion>,
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
    /// How fast the source plays; 1 is natural speed.
    pub speed: Ratio,
}

/// Anchor, position, scale and rotation of a clip after resolution.
type ResolvedTransform = (Point, Track<[f64; 2]>, Track<[f64; 2]>, Track<f64>);

/// Facts about asset files that only reading them can provide.
pub trait AssetInfo {
    /// Duration of a media asset in seconds, if known.
    fn duration(&self, asset_id: &str, src: &str) -> Option<Ratio>;

    /// The contents of a text asset, for the markup an HTML source draws.
    /// Returning `None` leaves the source to be read at render time, so
    /// validation without files still works.
    fn text(&self, asset_id: &str, src: &str) -> Option<String> {
        let _ = (asset_id, src);
        None
    }

    /// The contents of a file under the asset root, for a stylesheet a
    /// piece of markup links to. `None` means nothing was read, which is
    /// not the same as an empty file.
    fn read(&self, path: &str) -> Option<String> {
        let _ = path;
        None
    }

    /// The natural size of a picture or a video asset, for the check on
    /// a clip that names no `fit`. `None` where the caller cannot tell,
    /// so validation without files stays quiet.
    fn size(&self, asset_id: &str, src: &str) -> Option<(u32, u32)> {
        let _ = (asset_id, src);
        None
    }

    /// Whether this machine has a font family, for the check on a style
    /// that names one. `None` where the caller has no font database, so
    /// validation somewhere else stays quiet rather than guessing about
    /// a machine it cannot see.
    fn has_font_family(&self, family: &str) -> Option<bool> {
        let _ = family;
        None
    }

    /// Whether a file under the asset root is there, for a picture markup
    /// points at. `None` where the caller cannot tell, so validation
    /// without files stays quiet.
    fn exists(&self, path: &str) -> Option<bool> {
        let _ = path;
        None
    }
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
        used_rules: BTreeSet::new(),
        rules: BTreeMap::new(),
        from_markup: None,
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
    used_rules: BTreeSet<String>,
    /// Keyframe rules after parsing, by name, each sorted by offset.
    rules: BTreeMap<String, Vec<(f64, AnimValues)>>,
    /// What an `html` source's stylesheet contributed: its `@keyframes`
    /// and the `animation` on its outermost element. Set while the clip's
    /// source is resolved and taken by the clip a moment later, which is
    /// the only time it is meaningful.
    from_markup: Option<Markup>,
    /// Names of the compositions currently being resolved, for cycle checks.
    composition_stack: Vec<String>,
}

/// What a clip's markup said about motion.
#[derive(Default)]
struct Markup {
    /// `@keyframes` from the stylesheet, parsed.
    rules: BTreeMap<String, Vec<(f64, AnimValues)>>,
    /// The `animation` on the outermost element, as written.
    animation: Option<AnimationSpec>,
    /// That element's box, which is what a percentage in its animation is
    /// a share of, the way CSS resolves one against the element and not
    /// against the page.
    animated_box: (Option<f64>, Option<f64>),
}

/// Keyframes an `animation` contributes to a clip, in clip-local seconds.
#[derive(Default)]
struct AnimationKnots {
    translate: Vec<Keyframe<[f64; 2]>>,
    scale: Vec<Keyframe<[f64; 2]>>,
    rotate: Vec<Keyframe<f64>>,
    opacity: Vec<Keyframe<f64>>,
}

impl AnimationKnots {
    fn push(&mut self, time: f64, v: &AnimValues, easing: &Easing) {
        if let Some(value) = v.translate {
            // Percentages are resolved before this point, so whatever is
            // left is a distance in pixels.
            let value = [
                value[0].to_px(None).unwrap_or(0.0),
                value[1].to_px(None).unwrap_or(0.0),
            ];
            self.translate.push(Keyframe {
                time,
                value,
                easing: easing.clone(),
            });
        }
        if let Some(value) = v.scale {
            self.scale.push(Keyframe {
                time,
                value,
                easing: easing.clone(),
            });
        }
        if let Some(value) = v.rotate {
            self.rotate.push(Keyframe {
                time,
                value,
                easing: easing.clone(),
            });
        }
        if let Some(value) = v.opacity {
            self.opacity.push(Keyframe {
                time,
                value,
                easing: easing.clone(),
            });
        }
    }

    /// Runs of one animation, and separate animations, are expanded in
    /// whatever order they are written; time order is restored here.
    fn sort(&mut self) {
        self.translate.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.scale.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.rotate.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.opacity.sort_by(|a, b| a.time.total_cmp(&b.time));
    }
}

/// The most runs one animation is expanded into, so that an infinite
/// animation on a long clip cannot grow without bound.
const MAX_ANIMATION_RUNS: u32 = 10_000;

/// The clip's own box, where it is known before anything is drawn. A
/// percentage in `translate` resolves against it, as CSS resolves one
/// against the element's border box.
fn clip_box(source: &ResolvedSource, frame: FrameSize) -> (Option<f64>, Option<f64>) {
    match source {
        ResolvedSource::Html(h) => (h.width, h.height),
        ResolvedSource::Shape { width, height, .. } => (Some(*width), Some(*height)),
        ResolvedSource::Solid { .. } => {
            (Some(f64::from(frame.width)), Some(f64::from(frame.height)))
        }
        ResolvedSource::Composition(c) => (Some(f64::from(c.width)), Some(f64::from(c.height))),
        // A picture's size is whatever the file turns out to be.
        ResolvedSource::Text(_) | ResolvedSource::Image { .. } | ResolvedSource::Video { .. } => {
            (None, None)
        }
    }
}

/// The gap left where one run ends and the next begins with a different
/// value. A microsecond is far shorter than a frame at any rate, and keeps
/// the track strictly ordered.
const SEAM: f64 = 1e-6;

/// Closes the seams inside one animation: where a run ends exactly where
/// the next begins, the pair is one knot if the value is the same, and a
/// snap if it is not.
fn close_seams<T: PartialEq + Copy>(keys: &mut Vec<Keyframe<T>>) {
    keys.sort_by(|a, b| a.time.total_cmp(&b.time));
    let mut i = 0;
    while i + 1 < keys.len() {
        if keys[i].time == keys[i + 1].time {
            if keys[i].value == keys[i + 1].value {
                keys.remove(i + 1);
                continue;
            }
            keys[i].easing = Easing::Named(geneva_anim::NamedEasing::Hold);
            keys[i + 1].time += SEAM;
        }
        i += 1;
    }
}

/// Lays one animation's runs out in clip-local seconds.
fn expand(a: &Animation, steps: &[(f64, AnimValues)], length: f64, out: &mut AnimationKnots) {
    let runs = if a.iterations.is_finite() {
        a.iterations
            .ceil()
            .max(1.0)
            .min(f64::from(MAX_ANIMATION_RUNS)) as u32
    } else {
        // An infinite animation runs until the clip ends.
        let span = (length - a.delay).max(0.0);
        ((span / a.duration).ceil().max(1.0) as u32).min(MAX_ANIMATION_RUNS)
    };
    let mut mine = AnimationKnots::default();
    for i in 0..runs {
        let reversed = a.direction.reversed(i);
        // The last run of a fractional iteration count is cut short.
        let limit = if a.iterations.is_finite() {
            (a.iterations - f64::from(i)).min(1.0)
        } else {
            1.0
        };
        for (offset, v) in steps {
            let progress = if reversed { 1.0 - offset } else { *offset };
            if progress > limit + 1e-9 {
                continue;
            }
            mine.push(
                a.delay + (f64::from(i) + progress) * a.duration,
                v,
                &a.easing,
            );
        }
    }
    close_seams(&mut mine.translate);
    close_seams(&mut mine.scale);
    close_seams(&mut mine.rotate);
    close_seams(&mut mine.opacity);
    out.translate.append(&mut mine.translate);
    out.scale.append(&mut mine.scale);
    out.rotate.append(&mut mine.rotate);
    out.opacity.append(&mut mine.opacity);
}

/// A rule as a clip plays it. A property set at one offset only has
/// nothing to interpolate with, so it would hold a constant and drive
/// nothing. Dropping it keeps a rule to what actually moves, which is
/// what makes "to { transform: none }" mean "back to where you started"
/// rather than "and reset the scale and rotation too". An element inside
/// markup is played differently: there a lone `to` starts from the value
/// under it.
fn thinned(steps: &[(f64, AnimValues)]) -> Vec<(f64, AnimValues)> {
    let mut steps = steps.to_vec();
    let thin = |steps: &[(f64, AnimValues)], f: fn(&AnimValues) -> bool| {
        steps.iter().filter(|(_, v)| f(v)).count() < 2
    };
    if thin(&steps, |v| v.translate.is_some()) {
        for s in &mut steps {
            s.1.translate = None;
        }
    }
    if thin(&steps, |v| v.scale.is_some()) {
        for s in &mut steps {
            s.1.scale = None;
        }
    }
    if thin(&steps, |v| v.rotate.is_some()) {
        for s in &mut steps {
            s.1.rotate = None;
        }
    }
    if thin(&steps, |v| v.opacity.is_some()) {
        for s in &mut steps {
            s.1.opacity = None;
        }
    }
    steps.retain(|(_, v)| {
        v.translate.is_some() || v.scale.is_some() || v.rotate.is_some() || v.opacity.is_some()
    });
    steps
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

        self.parse_keyframe_rules();

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
        for name in tl.keyframes.keys() {
            if !self.used_rules.contains(name) {
                self.diags.push(Diagnostic::note(
                    "W203",
                    Path::root().key("keyframes").key(name),
                    format!("keyframes {name:?} are never used"),
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
            // HDR needs ten bits, which only some codecs carry; the
            // default codec (H.264) does not.
            let codec = out
                .encode
                .as_ref()
                .and_then(|e| e.video.as_ref())
                .and_then(|v| v.codec);
            let ten_bit = matches!(
                codec,
                Some(VideoCodec::H265 | VideoCodec::Av1 | VideoCodec::Vp9 | VideoCodec::Prores)
            );
            if !ten_bit {
                self.push(
                    Diagnostic::error(
                        "E420",
                        out_path.key("color").key("transfer"),
                        match codec {
                            Some(c) => format!("an HDR output needs a ten-bit codec; {c:?} carries eight bits"),
                            None => "an HDR output needs a ten-bit codec, and the default is H.264".to_owned(),
                        },
                    )
                    .with_help("set encode.video.codec to \"h265\", \"av1\", \"vp9\" or \"prores\", or the transfer to \"bt709\""),
                );
            }
        }
        if let Some(loudness) = out.audio.as_ref().and_then(|a| a.loudness.as_ref()) {
            self.check_loudness(loudness, &out_path.key("audio").key("loudness"));
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
            if video.fixed_keyframes == Some(true) && video.keyframe_interval.is_none() {
                self.push(
                    Diagnostic::error(
                        "E422",
                        out_path.key("encode").key("video").key("fixed_keyframes"),
                        "fixed keyframes need an interval to be fixed at",
                    )
                    .with_help("set encode.video.keyframe_interval, in seconds"),
                );
            }
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

        let outputs = self.resolve_outputs(duration);
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
            outputs,
        }
    }

    /// The `outputs` entries, each checked for the fields its kind takes,
    /// a usable file name, and a size derived from the canvas.
    fn resolve_outputs(&mut self, duration: Ratio) -> Vec<ResolvedOutput> {
        let tl = self.tl;
        let out = &tl.output;
        let mut resolved = Vec::new();
        let mut paths: BTreeMap<String, String> = BTreeMap::new();
        for (name, spec) in &tl.outputs {
            let path = Path::root().key("outputs").key(name);
            let kind = spec.kind;
            // Fields that belong to other kinds.
            let stray: &[(&str, bool)] = &[
                ("at", spec.at.is_some() && kind != OutputKind::Poster),
                ("every", spec.every.is_some() && kind != OutputKind::Sprites),
                (
                    "columns",
                    spec.columns.is_some() && kind != OutputKind::Sprites,
                ),
                (
                    "encode",
                    spec.encode.is_some() && !matches!(kind, OutputKind::Video | OutputKind::Audio),
                ),
                (
                    "audio",
                    spec.audio.is_some() && !matches!(kind, OutputKind::Video | OutputKind::Audio),
                ),
                (
                    "width",
                    spec.width.is_some() && matches!(kind, OutputKind::Audio | OutputKind::Sprites),
                ),
                ("height", spec.height.is_some() && kind == OutputKind::Audio),
            ];
            for (field, is_stray) in stray {
                if *is_stray {
                    self.push(
                        Diagnostic::error(
                            "E430",
                            path.key(field),
                            format!("\"{field}\" does not apply to a {} output", kind.as_str()),
                        )
                        .with_help("remove the field, or change \"kind\""),
                    );
                }
            }
            // The file name: relative, inside the directory, with an
            // extension the kind can write.
            let default_ext = match kind {
                OutputKind::Video => "mp4",
                OutputKind::Poster | OutputKind::Sprites => "jpg",
                OutputKind::Audio => "wav",
            };
            let file = spec
                .path
                .clone()
                .unwrap_or_else(|| format!("{name}.{default_ext}"));
            let ext = std::path::Path::new(&file)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let allowed: &[&str] = match kind {
                OutputKind::Video => &["mp4", "mov", "mkv", "webm", "mxf"],
                OutputKind::Poster | OutputKind::Sprites => &["jpg", "jpeg", "png"],
                OutputKind::Audio => &["wav", "m4a", "mp3", "flac", "ogg"],
            };
            if file.starts_with('/')
                || file.contains("..")
                || file.contains('\\')
                || file.contains('/')
                || file.is_empty()
            {
                self.push(
                    Diagnostic::error(
                        "E431",
                        path.key("path"),
                        "the path must be a plain file name inside the output directory",
                    )
                    .with_value(file.clone()),
                );
            } else if !allowed.contains(&ext.as_str()) {
                self.push(
                    Diagnostic::error(
                        "E431",
                        path.key("path"),
                        format!(
                            "a {} output cannot be written as .{ext}; use {}",
                            kind.as_str(),
                            allowed
                                .iter()
                                .map(|e| format!(".{e}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .with_value(file.clone()),
                );
            }
            if let Some(other) = paths.insert(file.clone(), name.clone()) {
                self.push(
                    Diagnostic::error(
                        "E432",
                        path.key("path"),
                        format!("outputs \"{other}\" and \"{name}\" would both write {file}"),
                    )
                    .with_value(file.clone()),
                );
            }
            // The picture size. A derived video dimension is rounded to
            // even, as codecs want; pictures keep exact sizes.
            let even = |v: u32| {
                if kind == OutputKind::Video && v % 2 == 1 {
                    v + 1
                } else {
                    v
                }
                .max(1)
            };
            let (width, height) = match kind {
                OutputKind::Audio => (0, 0),
                OutputKind::Sprites => {
                    let h = spec.height.unwrap_or(90).max(1);
                    let w = (f64::from(h) * f64::from(out.width) / f64::from(out.height.max(1)))
                        .round() as u32;
                    (w.max(1), h)
                }
                OutputKind::Video | OutputKind::Poster => match (spec.width, spec.height) {
                    (None, None) => (out.width, out.height),
                    (Some(w), Some(h)) => {
                        for (field, v) in [("width", w), ("height", h)] {
                            if v == 0 {
                                self.push(
                                    Diagnostic::error(
                                        "E402",
                                        path.key(field),
                                        format!("{field} must be greater than 0"),
                                    )
                                    .with_value(v),
                                );
                            }
                        }
                        (w, h)
                    }
                    (Some(w), None) => {
                        let h = (f64::from(w) * f64::from(out.height) / f64::from(out.width.max(1)))
                            .round() as u32;
                        (w, even(h.max(1)))
                    }
                    (None, Some(h)) => {
                        let w = (f64::from(h) * f64::from(out.width) / f64::from(out.height.max(1)))
                            .round() as u32;
                        (even(w.max(1)), h)
                    }
                },
            };
            if kind == OutputKind::Video {
                for (field, v) in [("width", width), ("height", height)] {
                    if v % 2 == 1 {
                        self.push(
                            Diagnostic::warning(
                                "W401",
                                path.key(field),
                                format!("{field} {v} is odd"),
                            )
                            .with_value(v)
                            .with_help("most video codecs need even dimensions"),
                        );
                    }
                }
            }
            // Times.
            let at = spec.at.map(|t| {
                let secs = t.resolve(self.fps);
                if secs < Ratio::ZERO || secs >= duration {
                    self.push(
                        Diagnostic::error(
                            "E433",
                            path.key("at"),
                            format!("the poster time {secs}s is outside the composition (0 to {duration}s)"),
                        )
                        .with_value(json!(t.to_string())),
                    );
                }
                secs
            });
            let every = spec.every.map(|t| {
                let secs = t.resolve(self.fps);
                if secs <= Ratio::ZERO {
                    self.push(
                        Diagnostic::error(
                            "E433",
                            path.key("every"),
                            "the sprite interval must be greater than 0",
                        )
                        .with_value(json!(t.to_string())),
                    );
                }
                secs
            });
            if let Some(c) = spec.columns {
                if c == 0 {
                    self.push(
                        Diagnostic::error(
                            "E433",
                            path.key("columns"),
                            "columns must be at least 1",
                        )
                        .with_value(c),
                    );
                }
            }
            let encode = match kind {
                OutputKind::Video => spec.encode.clone().or_else(|| out.encode.clone()),
                OutputKind::Audio => spec.encode.clone(),
                _ => None,
            };
            let audio = match kind {
                OutputKind::Video | OutputKind::Audio => {
                    spec.audio.clone().or_else(|| out.audio.clone())
                }
                _ => None,
            };
            if let Some(loudness) = spec.audio.as_ref().and_then(|a| a.loudness.as_ref()) {
                self.check_loudness(loudness, &path.key("audio").key("loudness"));
            }
            resolved.push(ResolvedOutput {
                name: name.clone(),
                kind,
                path: file,
                width,
                height,
                encode,
                audio,
                at,
                every,
                columns: spec.columns,
            });
        }
        resolved
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

    /// Whether a relative path stays under the asset root. The rule is
    /// the same wherever a path appears: no root, no drive, no "..".
    fn escapes_root(src: &str) -> bool {
        let p = std::path::Path::new(src);
        p.is_absolute()
            || src.starts_with('/')
            || src.starts_with('\\')
            || src.contains("://")
            || p.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
    }

    fn check_asset_path(&mut self, asset: &Asset, path: &Path) {
        let src = asset.src.as_str();
        let escapes = Self::escapes_root(src);
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

    /// Reads a caption asset and gathers it into cues. `None` means the
    /// file could not be read, and a diagnostic says why.
    #[allow(clippy::too_many_arguments)]
    fn read_cues(
        &mut self,
        asset: &str,
        spath: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        max_lines: Option<u32>,
        min_duration: Option<Time>,
        merge_gap: Option<Time>,
        style: &TextSource,
    ) -> Option<Vec<crate::captions::Cue>> {
        let apath = spath.key("asset");
        // A .srt or .vtt infers as a subtitle, which is the same file
        // whether it is muxed as a stream or drawn into the picture.
        self.asset_ref(
            asset,
            &apath,
            assets,
            &[AssetKind::Captions, AssetKind::Subtitle],
        )?;
        let src = assets.get(asset).map_or("", |a| a.src.as_str());
        // Nothing read is either no reader at all, where validation
        // without files should stay quiet, or a file that is not there,
        // which is worth saying.
        let Some(text) = self.info.text(asset, src) else {
            if self.info.exists(src) == Some(false) {
                self.push(
                    Diagnostic::error(
                        "E453",
                        apath,
                        format!("the caption file {src:?} is not there"),
                    )
                    .with_value(src),
                );
            }
            return None;
        };

        let format = std::path::Path::new(src)
            .extension()
            .and_then(|e| e.to_str())
            .and_then(crate::captions::CaptionFormat::from_extension);
        let Some(format) = format else {
            self.push(
                Diagnostic::error(
                    "E453",
                    apath,
                    format!("{src:?} is not a caption file geneva reads"),
                )
                .with_help("use .srt, .vtt, or the .json a speech recogniser writes"),
            );
            return None;
        };

        let cues = match format {
            crate::captions::CaptionFormat::Words => {
                let words = match crate::captions::parse_words(&text) {
                    Ok(w) => w,
                    Err(e) => {
                        self.push(Diagnostic::error("E453", apath, e.to_string()));
                        return None;
                    }
                };
                let mut rules = crate::captions::Grouping::default();
                if let Some(n) = max_lines {
                    rules.max_lines = n.max(1) as usize;
                }
                if let Some(d) = min_duration {
                    rules.min_duration = self.time(d, &spath.key("min_duration"), "min_duration");
                }
                if let Some(d) = merge_gap {
                    rules.merge_gap = self.time(d, &spath.key("merge_gap"), "merge_gap");
                }
                crate::captions::cues_from_words(&words, rules)
            }
            _ => {
                let cues = match crate::captions::parse(&text) {
                    Ok(c) => c,
                    Err(e) => {
                        self.push(Diagnostic::error("E453", apath, e.to_string()));
                        return None;
                    }
                };
                if style.highlight.is_some() {
                    self.push(
                        Diagnostic::warning(
                            "W453",
                            spath.key("highlight"),
                            format!(
                                "{src:?} has no word times, so nothing is picked out as it is said"
                            ),
                        )
                        .with_help(
                            "a highlight needs a word file; SubRip and WebVTT time whole cues",
                        ),
                    );
                }
                cues
            }
        };

        if cues.is_empty() {
            self.push(Diagnostic::warning(
                "W453",
                apath,
                format!("{src:?} has no cues in it"),
            ));
            return None;
        }
        self.push(Diagnostic::note(
            "N453",
            spath.clone(),
            format!("{} cues read from {src:?}", cues.len()),
        ));
        Some(cues)
    }

    /// Turns every captions clip in a layer into one clip per cue, and
    /// leaves every other clip alone. The index beside each clip is the
    /// one it has in the document, so a diagnostic about a cue still
    /// points at the line someone wrote.
    fn expand_captions(
        &mut self,
        layer: &Layer,
        path: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        frame: FrameSize,
    ) -> Vec<(Clip, usize)> {
        let mut out: Vec<(Clip, usize)> = Vec::new();
        for (i, clip) in layer.clips.iter().enumerate() {
            let Source::Captions {
                asset,
                position,
                margin,
                safe,
                follow_file,
                max_lines,
                min_duration,
                merge_gap,
                style,
            } = &clip.source
            else {
                out.push((clip.clone(), i));
                continue;
            };
            let cpath = path.key("clips").index(i);
            let spath = cpath.key("source");
            // Cue times are clip-relative, like every other time in a
            // document (timing rule 6). The file's times are whatever the
            // recogniser or the subtitle wrote, measured from the start of
            // the media it was made from, so the clip's own start is what
            // puts them on the timeline.
            let offset = match clip.start {
                Some(t) => self.time(t, &cpath.key("start"), "start"),
                None => Ratio::ZERO,
            };
            let style = style.as_deref().cloned().unwrap_or_default();
            let Some(cues) = self.read_cues(
                asset,
                &spath,
                assets,
                *max_lines,
                *min_duration,
                *merge_gap,
                &style,
            ) else {
                continue;
            };

            let height = f64::from(frame.height);
            let safe = safe.unwrap_or(5.0).clamp(0.0, 49.0);
            let safe_px = height * safe / 100.0;
            let margin_px = match margin {
                Some(l) => {
                    let m = self.positive_length(*l, &spath.key("margin"), height, "margin");
                    if m < safe_px {
                        self.push(
                            Diagnostic::note(
                                "N453",
                                spath.key("margin"),
                                format!(
                                    "captions sit {m}px from the edge, inside the title-safe \
inset of {safe_px}px"
                                ),
                            )
                            .with_help("raise \"margin\", or lower \"safe\" to accept it"),
                        );
                    }
                    m
                }
                None => safe_px,
            };
            let place = position.unwrap_or_default();

            for cue in cues {
                let mut spec = style.clone();
                if cue.words.is_empty() {
                    spec.text = Some(cue.text.clone());
                    spec.words = None;
                } else {
                    // Word times are clip-relative, and the clip starts
                    // where the cue does.
                    spec.words = Some(
                        cue.words
                            .iter()
                            .map(|w| Word {
                                text: w.text.clone(),
                                start: Time::Seconds((w.start - cue.start).max(Ratio::ZERO)),
                                end: Some(Time::Seconds((w.end - cue.start).max(Ratio::ZERO))),
                            })
                            .collect(),
                    );
                    spec.text = Some(cue.text.clone());
                }
                let follow = follow_file.unwrap_or(true) && !cue.place.is_empty();
                if follow {
                    if let Some(align) = cue.place.align {
                        spec.align = Some(match align {
                            crate::captions::Align::Left => TextAlign::Left,
                            crate::captions::Align::Center => TextAlign::Center,
                            crate::captions::Align::Right => TextAlign::Right,
                        });
                    }
                    if let Some(size) = cue.place.size {
                        spec.max_width = Some(Length::Percent(f64::from(size)));
                    }
                }
                let (anchor, y) = match (follow.then_some(cue.place.line).flatten(), place) {
                    // WebVTT measures its line from the top, as a share of
                    // the frame, and the box hangs below it.
                    (Some(line), _) => (
                        Point {
                            x: Length::Percent(50.0),
                            y: Length::Percent(0.0),
                        },
                        Length::Percent(f64::from(line)),
                    ),
                    (None, CaptionPosition::Bottom) => (
                        Point {
                            x: Length::Percent(50.0),
                            y: Length::Percent(100.0),
                        },
                        Length::Px(height - margin_px),
                    ),
                    (None, CaptionPosition::Top) => (
                        Point {
                            x: Length::Percent(50.0),
                            y: Length::Percent(0.0),
                        },
                        Length::Px(margin_px),
                    ),
                    (None, CaptionPosition::Center) => (
                        Point {
                            x: Length::Percent(50.0),
                            y: Length::Percent(50.0),
                        },
                        Length::Percent(50.0),
                    ),
                };
                let x = match follow.then_some(cue.place.position).flatten() {
                    Some(p) => Length::Percent(f64::from(p)),
                    None => Length::Percent(50.0),
                };

                out.push((
                    Clip {
                        id: clip.id.clone(),
                        source: Source::Text(Box::new(spec)),
                        start: Some(Time::Seconds(offset + cue.start)),
                        duration: Some(Time::Seconds((cue.end - cue.start).max(Ratio::ZERO))),
                        transform: Some(Transform {
                            position: Some(Animated::Constant(Point { x, y })),
                            anchor: Some(anchor),
                            ..Transform::default()
                        }),
                        ..clip.clone()
                    },
                    i,
                ));
            }
        }
        out
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
        // A captions clip is one line in the document and a run of clips
        // on the timeline, one per cue, so the copy planner can copy the
        // gaps between them. Each carries the index of the clip it came
        // from, so diagnostics still point at what someone wrote.
        let source_clips = self.expand_captions(layer, path, assets, frame);

        let mut clips: Vec<ResolvedClip> = Vec::new();
        let mut cursor = Ratio::ZERO;
        let mut any_open = false;
        for (clip, i) in &source_clips {
            let (clip, i) = (clip, *i);
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
            // Taken straight after the source, which is the only moment it
            // belongs to this clip.
            let markup = self.from_markup.take().unwrap_or_default();
            let speed = self.speed_of(clip.speed, &cpath.key("speed"));
            // The source plays `speed` times faster, so it lasts that much
            // less.
            let length = match length {
                ClipLength::Fixed(l) => ClipLength::Fixed(l / speed),
                ClipLength::Open => ClipLength::Open,
            };
            // A transition is resolved the same way wherever it sits; only
            // whether there is a clip on the other side changes what the
            // ramps do.
            let resolve_transition = |me: &mut Self, tr: &Transition, field: &str, paired: bool| {
                let tpath = cpath.key(field);
                let duration = me.time(tr.duration, &tpath.key("duration"), "transition duration");
                let color = tr.color.as_ref().map_or(Color::BLACK, |c| c.0).to_linear();
                if tr.color.is_some() && tr.kind != TransitionKind::Fade {
                    me.push(
                        Diagnostic::warning(
                            "W304",
                            tpath.key("color"),
                            format!(
                                "{clip_id} has a transition color, but a {:?} transition never shows one",
                                tr.kind
                            ),
                        )
                        .with_help("use \"kind\": \"fade\" to dip through the color"),
                    );
                }
                let ease = tr.ease.clone().unwrap_or_default();
                if let Some(reason) = ease.validate() {
                    me.push(Diagnostic::error("E308", tpath.key("ease"), reason));
                }
                ResolvedTransition {
                    kind: tr.kind,
                    duration,
                    color,
                    ease,
                    paired,
                }
            };

            let mut transition_in = None;
            if let Some(tr) = &clip.transition {
                // The first clip on a layer has nothing before it, so its
                // transition opens the piece instead of joining a pair and
                // does not pull the clip earlier.
                let paired = i > 0;
                let t = resolve_transition(self, tr, "transition", paired);
                if paired && clip.start.is_none() {
                    start = start - t.duration;
                }
                transition_in = Some(t);
            }
            let mut transition_out = None;
            if let Some(tr) = &clip.transition_out {
                let follower = layer
                    .clips
                    .get(i + 1)
                    .is_some_and(|next| next.transition.is_some());
                if follower {
                    self.push(
                        Diagnostic::error(
                            "E307",
                            cpath.key("transition_out"),
                            format!(
                                "{clip_id} sets transition_out, but the clip after it already \
transitions in over the same join"
                            ),
                        )
                        .with_help("keep one of the two; the arriving clip's \"transition\" covers the pair"),
                    );
                } else {
                    transition_out = Some(resolve_transition(self, tr, "transition_out", false));
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
                    match &transition_in {
                        Some(tr) if overlap <= tr.duration => {
                            let tdur = tr.duration;
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
                if let Some(tdur) = transition_in.as_ref().map(|t| t.duration) {
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
            // A rule's transform is laid over what the clip already sets:
            // translations add to the position, scales multiply, rotations
            // add. Opacity is the same property either way, so it replaces.
            // A clip's own `animation` replaces the one the markup carries,
            // since only the document knows where the clip sits in time.
            if clip.animation.is_some() && markup.animation.is_some() {
                self.push(
                    Diagnostic::warning(
                        "W451",
                        cpath.key("animation"),
                        "this replaces the animation the markup asks for".to_owned(),
                    )
                    .with_help("remove one of them so the file and the document agree"),
                );
            }
            let played = match &clip.animation {
                Some(a) => Some(AnimationSpec {
                    shorthand: Some(a.clone()),
                    longhands: Vec::new(),
                }),
                None => markup.animation,
            };
            let (position, scale, rotation, opacity) = match played.as_ref() {
                None => (position, scale, rotation, opacity),
                Some(spec) => {
                    let apath = if clip.animation.is_some() {
                        cpath.key("animation")
                    } else {
                        cpath.key("source")
                    };
                    // A percentage in the clip's own animation is a share
                    // of the clip's box; in one the markup carries it is a
                    // share of the element that carries it, as in a
                    // browser.
                    let relative_to = if clip.animation.is_some() {
                        clip_box(&source, frame)
                    } else {
                        markup.animated_box
                    };
                    let k =
                        self.resolve_animation(spec, &apath, length, &markup.rules, relative_to);
                    (
                        self.animated_over(
                            position,
                            &k.translate,
                            &tpath.key("position"),
                            "position",
                            |b, d| [b[0] + d[0], b[1] + d[1]],
                        ),
                        self.animated_over(
                            scale,
                            &k.scale,
                            &tpath.key("scale"),
                            "scale",
                            |b, d| [b[0] * d[0], b[1] * d[1]],
                        ),
                        self.animated_over(
                            rotation,
                            &k.rotate,
                            &tpath.key("rotation"),
                            "rotation",
                            |b, d| b + d,
                        ),
                        self.animated_over(
                            opacity,
                            &k.opacity,
                            &cpath.key("opacity"),
                            "opacity",
                            |_, v| v,
                        ),
                    )
                }
            };
            if let Some(crop) = &clip.crop {
                self.check_crop(crop, &cpath.key("crop"));
            }
            self.check_fit(clip, frame, &cpath, assets);
            let mask = clip
                .mask
                .as_ref()
                .map(|m| self.resolve_mask(m, &cpath.key("mask"), assets));
            let effects = clip
                .effects
                .iter()
                .enumerate()
                .map(|(k, e)| match e {
                    Effect::Blur { radius } => ResolvedEffect::Blur(self.track_f64(
                        Some(radius),
                        &cpath.key("effects").index(k).key("radius"),
                        0.0,
                        length,
                        Some((0.0, 4096.0)),
                        "blur radius",
                    )),
                })
                .collect();

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
                transition_out,
                effects,
                mask,
                speed,
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
            Source::Html {
                html,
                asset,
                css,
                width,
                height,
            } => (
                self.resolve_html(
                    html.as_deref(),
                    asset.as_deref(),
                    css.as_deref(),
                    *width,
                    *height,
                    &spath,
                    assets,
                    frame,
                ),
                ClipLength::Open,
            ),
            // The layer expands these into text clips before anything
            // here sees them; a captions source that got this far drew a
            // diagnostic already and has no cues to show.
            Source::Captions { style, .. } => {
                let style = style.as_deref().cloned().unwrap_or_default();
                (
                    ResolvedSource::Text(Box::new(
                        self.resolve_text(&style, &spath, assets, frame),
                    )),
                    ClipLength::Open,
                )
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

    /// A speed factor as an exact ratio; 1 when absent, and 1 with an
    /// error when out of range.
    fn speed_of(&mut self, speed: Option<f64>, path: &Path) -> Ratio {
        let Some(v) = speed else {
            return Ratio::ONE;
        };
        let ratio = Ratio::approximate(v, 10_000);
        match ratio {
            Some(r) if v > 0.0 && v <= 100.0 && r > Ratio::ZERO => r,
            _ => {
                self.push(
                    Diagnostic::error(
                        "E402",
                        path.clone(),
                        "speed must be greater than 0 and at most 100",
                    )
                    .with_value(json!(v)),
                );
                Ratio::ONE
            }
        }
    }

    fn resolve_mask(
        &mut self,
        m: &Mask,
        path: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
    ) -> ResolvedMask {
        if let Some(asset) = &m.asset {
            self.asset_ref(asset, &path.key("asset"), assets, &[AssetKind::Image]);
        }
        for (name, v) in [("width", m.width), ("height", m.height)] {
            let bad = match v {
                Some(Length::Px(px)) => !(px > 0.0 && px.is_finite()),
                Some(Length::Percent(p)) => !(p > 0.0 && p.is_finite()),
                None => false,
            };
            if bad {
                self.push(
                    Diagnostic::error(
                        "E402",
                        path.key(name),
                        format!("mask {name} must be greater than 0"),
                    )
                    .with_value(json!(v.map(|l| l.to_string()))),
                );
            }
        }
        for (name, v) in [("radius", m.radius), ("feather", m.feather)] {
            if let Some(v) = v {
                if !(v >= 0.0 && v.is_finite()) {
                    self.push(
                        Diagnostic::error(
                            "E402",
                            path.key(name),
                            format!("mask {name} must be 0 or more"),
                        )
                        .with_value(json!(v)),
                    );
                }
            }
        }
        ResolvedMask {
            shape: m.shape.unwrap_or(ShapeKind::Rect),
            rect: [m.x, m.y, m.width, m.height],
            radius: m.radius.unwrap_or(0.0).max(0.0),
            feather: m.feather.unwrap_or(0.0).max(0.0),
            asset: m.asset.clone(),
            invert: m.invert.unwrap_or(false),
        }
    }

    /// A crop's edges must lie inside the source and its size must be
    /// positive; what that means in pixels depends on the source, so the
    /// checks are on the values as written.
    /// Warns when a clip that names no `fit` lands in the frame in a way
    /// its author probably did not mean: a picture bigger than the frame
    /// drawn at its own size, so the edges are cut, or a video
    /// letterboxed into a small part of the frame.
    ///
    /// The defaults are right for what each source is usually for, a
    /// picture placed where it is put and a video shown whole, and this
    /// says so where the result is surprising rather than changing them.
    /// A clip that carries a `crop` or a `scale` is left alone: both are
    /// a decision about size, and whoever made it does not need telling.
    fn check_fit(
        &mut self,
        clip: &Clip,
        frame: FrameSize,
        cpath: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
    ) {
        if clip.fit.is_some()
            || clip.crop.is_some()
            || clip.animation.is_some()
            || clip.transform.as_ref().is_some_and(|t| t.scale.is_some())
        {
            return;
        }
        let (asset_id, video) = match &clip.source {
            Source::Image { asset } => (asset, false),
            Source::Video { asset, .. } => (asset, true),
            _ => return,
        };
        let Some((sw, sh)) = assets
            .get(asset_id)
            .and_then(|a| self.info.size(asset_id, &a.src))
        else {
            return;
        };
        if sw == 0 || sh == 0 || frame.width == 0 || frame.height == 0 {
            return;
        }
        let (sw, sh) = (f64::from(sw), f64::from(sh));
        let (fw, fh) = (f64::from(frame.width), f64::from(frame.height));
        let path = cpath.key("fit");
        if video {
            // A video is shown whole, so the question is how much of the
            // frame is left over. Bars down two sides of a 4:3 picture on
            // a 16:9 frame are ordinary; a 16:9 picture on a 9:16 frame
            // reaches under a third of it, which is usually a reframe
            // nobody asked for.
            let scale = (fw / sw).min(fh / sh);
            let covered = (sw * scale * sh * scale) / (fw * fh);
            if covered < 0.67 {
                self.push(
                    Diagnostic::warning(
                        "W404",
                        path,
                        format!(
                            "the video is {sw:.0}x{sh:.0} in a {fw:.0}x{fh:.0} frame and is shown \
whole, so it covers {:.0}% of it and the rest is background",
                            covered * 100.0
                        ),
                    )
                    .with_help(
                        "that is what \"fit\": \"contain\" does, which is the default for a \
video; \"cover\" fills the frame and crops the edges instead, and the verbs' --fill blur puts a \
blurred copy behind",
                    ),
                );
            }
        } else if sw > fw * 1.05 || sh > fh * 1.05 {
            // A picture is drawn at its own size, which is what an
            // overlay wants and what a photo from a camera does not: a
            // 4000x3000 picture on a 1920x1080 frame shows its middle.
            let shown = (fw / sw).min(1.0) * (fh / sh).min(1.0);
            self.push(
                Diagnostic::warning(
                    "W404",
                    path,
                    format!(
                        "the picture is {sw:.0}x{sh:.0} in a {fw:.0}x{fh:.0} frame and is drawn at \
its own size, so about {:.0}% of it is shown and the frame cuts the rest",
                        shown * 100.0
                    ),
                )
                .with_help(
                    "that is what \"fit\": \"none\" does, which is the default for a picture, \
since a picture is usually placed where it is put; \"contain\" shows the whole of it and \"cover\" \
fills the frame",
                ),
            );
        }
    }

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

    /// Warns when a style names a font family this machine does not have.
    ///
    /// The text still draws, in whatever the shaper falls back to, which
    /// is a different picture on a different machine and the kind of
    /// difference that is only noticed once the file is somewhere else.
    /// A family that is the id of a font asset is fine by definition:
    /// the document carries the file, which is the fix this suggests.
    fn check_font(
        &mut self,
        family: Option<&str>,
        path: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        in_markup: bool,
    ) {
        let Some(family) = family else {
            return;
        };
        if assets.contains_key(family) {
            return;
        }
        if self.info.has_font_family(family) != Some(false) {
            return;
        }
        self.push(
            Diagnostic::warning(
                "W405",
                path.clone(),
                format!(
                    "no font family called {family:?} is installed here, so the text will be drawn in whatever the machine falls back to, and another machine may fall back to something else"
                ),
            )
            .with_help(if in_markup {
                "add the font file as an asset of kind \"font\"; the markup can name it by that asset id or by the family the file itself declares, which keeps the document the same everywhere"
            } else {
                "add the font file as an asset of kind \"font\" and name that asset id here, which keeps the document the same everywhere"
            }),
        );
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
        self.check_font(
            expanded.style.font.as_deref(),
            &spath.key("font"),
            assets,
            false,
        );
        if let Some(h) = &expanded.highlight {
            self.check_font(
                h.font.as_deref(),
                &spath.key("highlight").key("font"),
                assets,
                false,
            );
        }
        let text = &expanded;
        let mut words = Vec::new();
        if let Some(list) = &text.words {
            let wpath = spath.key("words");
            let mut prev_end = Ratio::ZERO;
            for (i, w) in list.iter().enumerate() {
                let p = wpath.index(i);
                let start = self.time(w.start, &p.key("start"), "word start");
                // A word without an "end" is current until the next one
                // starts. The last word has no next one, so it needs its own.
                let (written, epath) = match w.end {
                    Some(t) => (Some(t), p.key("end")),
                    None => (
                        list.get(i + 1).map(|n| n.start),
                        wpath.index(i + 1).key("start"),
                    ),
                };
                let Some(written) = written else {
                    self.push(
                        Diagnostic::error(
                            "E102",
                            p.key("end"),
                            format!("the last word {:?} has no \"end\"", w.text),
                        )
                        .with_help(
                            "a word takes its end from the word after it; the last one needs its own",
                        ),
                    );
                    continue;
                };
                let end = self.time(written, &epath, "word end");
                if end <= start {
                    self.push(
                        Diagnostic::error(
                            "E411",
                            epath,
                            format!(
                                "word {:?} ends at {end}s, not after it starts at {start}s",
                                w.text
                            ),
                        )
                        .with_value(json!(written.to_string())),
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
        let style_path = spath.key("style");
        let color = self.track_color(
            text.style.color.as_ref(),
            &style_path.key("color"),
            Color::WHITE,
        );
        let highlight_color = text.highlight.as_ref().map(|h| {
            let a = h.color.as_ref().or(text.style.color.as_ref());
            self.track_color(a, &spath.key("highlight").key("color"), Color::WHITE)
        });
        let fill = text
            .style
            .fill
            .as_ref()
            .and_then(|f| self.resolve_fill(f, &spath.key("fill")));
        let highlight_fill = text.highlight.as_ref().and_then(|h| {
            let f = highlight_fill_spec(h, &text.style)?;
            // The base fill was already checked under its own path.
            let p = if h.fill.is_some() {
                spath.key("highlight").key("fill")
            } else {
                spath.key("fill")
            };
            self.resolve_fill(f, &p)
        });
        let mut shadow = Vec::new();
        if let Some(list) = &text.shadow {
            for (i, sh) in list.0.iter().enumerate() {
                // One shadow was written as an object at "shadow"; a list
                // puts each at its index.
                let p = if list.0.len() == 1 {
                    spath.key("shadow")
                } else {
                    spath.key("shadow").index(i)
                };
                let x =
                    self.track_f64(Some(&sh.x), &p.key("x"), 0.0, Ratio::ZERO, None, "shadow x");
                let y =
                    self.track_f64(Some(&sh.y), &p.key("y"), 0.0, Ratio::ZERO, None, "shadow y");
                let blur = self.track_f64(
                    Some(&sh.blur),
                    &p.key("blur"),
                    0.0,
                    Ratio::ZERO,
                    Some((0.0, 1.0e6)),
                    "shadow blur",
                );
                let peak = |a: &Animated<f64>| a.values().fold(0.0f64, |m, v| m.max(v.abs()));
                shadow.push(ShadowTrack {
                    color: self.track_color(sh.color.as_ref(), &p.key("color"), SHADOW_DEFAULT),
                    x,
                    y,
                    blur,
                    reach: peak(&sh.blur) + peak(&sh.x).max(peak(&sh.y)),
                });
            }
        }
        ResolvedText {
            text: content,
            words,
            max_width,
            spec: text.clone(),
            color,
            highlight_color,
            fill,
            highlight_fill,
            shadow,
        }
    }

    /// A text fill's gradient parsed and its offsets as tracks. A gradient
    /// that does not parse is an E103 and no fill; a tile with no area is
    /// an E402.
    fn resolve_fill(&mut self, f: &TextFill, path: &Path) -> Option<FillTrack> {
        let background = match geneva_html::style::background(&f.gradient) {
            Ok(bg) => bg,
            Err(e) => {
                self.push(
                    Diagnostic::error("E103", path.key("gradient"), e)
                        .with_value(json!(f.gradient))
                        .with_help("a fill is linear-gradient(...), radial-gradient(...) or a colour, as CSS writes them"),
                );
                return None;
            }
        };
        for (name, v) in [("width", f.width), ("height", f.height)] {
            if let Some(v) = v {
                if v.partial_cmp(&0.0) != Some(Ordering::Greater) {
                    self.push(
                        Diagnostic::error(
                            "E402",
                            path.key(name),
                            format!("fill {name} must be greater than 0"),
                        )
                        .with_value(v),
                    );
                }
            }
        }
        let x = self.track_f64(Some(&f.x), &path.key("x"), 0.0, Ratio::ZERO, None, "fill x");
        let y = self.track_f64(Some(&f.y), &path.key("y"), 0.0, Ratio::ZERO, None, "fill y");
        Some(FillTrack {
            background,
            width: f.width.filter(|w| *w > 0.0),
            height: f.height.filter(|h| *h > 0.0),
            x,
            y,
        })
    }

    /// Parses every `@keyframes` rule once, so a rule used by ten clips
    /// is reported once and the clips only look it up.
    fn parse_keyframe_rules(&mut self) {
        let root = Path::root().key("keyframes");
        for (name, rule) in &self.tl.keyframes {
            let steps = self.parse_keyframe_rule(name, rule, &root.key(name));
            self.rules.insert(name.clone(), steps);
        }
    }

    /// Parses one rule's offsets and declaration blocks. `rpath` is what
    /// its diagnostics point at, which is the document for a `keyframes`
    /// entry and the source for an `@keyframes` inside markup.
    fn parse_keyframe_rule(
        &mut self,
        name: &str,
        rule: &BTreeMap<String, String>,
        rpath: &Path,
    ) -> Vec<(f64, AnimValues)> {
        {
            let rpath = rpath.clone();
            let mut steps: Vec<(f64, AnimValues)> = Vec::new();
            for (key, block) in rule {
                let kpath = rpath.key(key);
                let offset = match crate::animation::parse_offset(key) {
                    Ok(o) => o,
                    Err(e) => {
                        self.diags
                            .push(Diagnostic::error("E442", kpath, e).with_value(json!(key)));
                        continue;
                    }
                };
                match crate::animation::parse_declarations(block) {
                    Ok(v) if v.is_empty() => self.diags.push(
                        Diagnostic::warning("W440", kpath, format!("{key} sets nothing"))
                            .with_help(
                                "a keyframe sets transform, opacity, filter, color, text-shadow, \
letter-spacing, a size or background-position",
                            ),
                    ),
                    Ok(v) => steps.push((offset, v)),
                    Err(e) => self
                        .diags
                        .push(Diagnostic::error("E442", kpath, e).with_value(json!(block))),
                }
            }
            steps.sort_by(|a, b| a.0.total_cmp(&b.0));
            steps.retain(|(_, v)| !v.is_empty());
            if steps.windows(2).any(|w| w[0].0 == w[1].0) {
                self.diags.push(
                    Diagnostic::error(
                        "E442",
                        rpath.clone(),
                        format!("{name:?} has two offsets at the same place"),
                    )
                    .with_help(
                        "\"from\" and \"0%\" are the same offset, as are \"to\" and \"100%\"",
                    ),
                );
            }
            steps
        }
    }

    /// Expands a clip's `animation` into keyframes in clip-local seconds.
    fn resolve_animation(
        &mut self,
        spec: &AnimationSpec,
        path: &Path,
        length: Ratio,
        extra: &BTreeMap<String, Vec<(f64, AnimValues)>>,
        clip_box: (Option<f64>, Option<f64>),
    ) -> AnimationKnots {
        let mut knots = AnimationKnots::default();
        let animations = match parse_animation_spec(spec.shorthand.as_deref(), &spec.longhands) {
            Ok(a) => a,
            Err(e) => {
                self.push(
                    Diagnostic::error("E441", path.clone(), e)
                        .with_value(json!(spec.shorthand.clone().unwrap_or_default())),
                );
                return knots;
            }
        };
        for a in &animations {
            self.used_rules.insert(a.name.clone());
            // The document's rules shadow the markup's, being the outer
            // scope; a name in both is worth saying out loud.
            if self.rules.contains_key(&a.name) && extra.contains_key(&a.name) {
                self.push(
                    Diagnostic::warning(
                        "W451",
                        path.clone(),
                        format!("{:?} is a rule in the document and in the markup", a.name),
                    )
                    .with_help("the document's rule is the one played; rename one of them"),
                );
            }
            let Some(steps) = self.rules.get(&a.name).or_else(|| extra.get(&a.name)) else {
                let known: Vec<&str> = self
                    .tl
                    .keyframes
                    .keys()
                    .map(String::as_str)
                    .chain(extra.keys().map(String::as_str))
                    .collect();
                let help = if known.is_empty() {
                    "no \"keyframes\" are declared, in the document or in the markup".to_owned()
                } else {
                    format!("the rules in scope are {}", known.join(", "))
                };
                self.push(
                    Diagnostic::error(
                        "E440",
                        path.clone(),
                        format!("no keyframes named {:?}", a.name),
                    )
                    .with_value(json!(a.name))
                    .with_help(help),
                );
                continue;
            };
            if steps.iter().any(|(_, v)| v.beyond_a_clip()) {
                self.push(
                    Diagnostic::error(
                        "E442",
                        path.clone(),
                        format!(
                            "{:?} sets a property the clip cannot play: filter, color, \
text-shadow, letter-spacing, a size or background-position",
                            a.name
                        ),
                    )
                    .with_value(json!(a.name))
                    .with_help(
                        "the clip moves the whole picture with transform and opacity; put such \
an animation on an element inside the outermost one, which is played there",
                    ),
                );
                continue;
            }
            let steps = thinned(steps);
            if steps.len() < 2 {
                self.push(
                    Diagnostic::warning(
                        "W440",
                        path.clone(),
                        format!("nothing in {:?} interpolates", a.name),
                    )
                    .with_help(
                        "a rule a clip plays needs a property set at two offsets; one offset on \
its own holds a value rather than moving it",
                    ),
                );
                continue;
            }
            // A percentage in a translation is a share of the clip's own
            // box, which a picture does not have until it is opened.
            let mut unresolved: Option<Shift> = None;
            let steps: Vec<(f64, AnimValues)> = steps
                .iter()
                .map(|(offset, v)| {
                    let mut v = v.clone();
                    if let Some([x, y]) = v.translate {
                        match (x.to_px(clip_box.0), y.to_px(clip_box.1)) {
                            (Some(px), Some(py)) => {
                                v.translate = Some([Shift::Px(px), Shift::Px(py)]);
                            }
                            _ => {
                                unresolved = Some(if x.is_relative() && clip_box.0.is_none() {
                                    x
                                } else {
                                    y
                                });
                                v.translate = None;
                            }
                        }
                    }
                    (*offset, v)
                })
                .collect();
            if let Some(shift) = unresolved {
                let _ = shift;
                self.push(
                    Diagnostic::error(
                        "E442",
                        path.clone(),
                        format!(
                            "{:?} moves the clip by a percentage of a box whose size is not known yet",
                            a.name
                        ),
                    )
                    .with_help(
                        "a video, image or text clip is whatever size its content turns out to \
be; write the distance in pixels, or give the source a size",
                    ),
                );
            }
            expand(a, &steps, length.to_f64(), &mut knots);
        }
        knots.sort();
        for (times, what) in [
            (
                knots.translate.iter().map(|k| k.time).collect::<Vec<_>>(),
                "translate",
            ),
            (knots.scale.iter().map(|k| k.time).collect(), "scale"),
            (knots.rotate.iter().map(|k| k.time).collect(), "rotate"),
            (knots.opacity.iter().map(|k| k.time).collect(), "opacity"),
        ] {
            if let Some([at, _]) = times.windows(2).find(|w| w[0] == w[1]) {
                self.push(
                    Diagnostic::error(
                        "E444",
                        path.clone(),
                        format!("two animations set {what} at {at}s"),
                    )
                    .with_help("give them ranges that do not overlap, or merge them into one rule"),
                );
            }
        }
        knots
    }

    /// Lays an animation's keyframes over a track, which must be constant
    /// where the animation drives it.
    fn animated_over<T: geneva_anim::Interpolate + Copy>(
        &mut self,
        base: Track<T>,
        knots: &[Keyframe<T>],
        path: &Path,
        what: &str,
        combine: impl Fn(T, T) -> T,
    ) -> Track<T> {
        if knots.is_empty() {
            return base;
        }
        if !base.is_constant() {
            self.push(
                Diagnostic::error(
                    "E443",
                    path.clone(),
                    format!("{what} is set by keyframes and by an animation"),
                )
                .with_help("animate it one way or the other"),
            );
            return base;
        }
        let start = base.keyframes()[0].value;
        let keys: Vec<Keyframe<T>> = knots
            .iter()
            .map(|k| Keyframe {
                time: k.time,
                value: combine(start, k.value),
                easing: k.easing.clone(),
            })
            .collect();
        Track::new(keys).unwrap_or(base)
    }

    /// Reads, parses and styles an HTML source. Layout waits for the
    /// renderer, which is the only thing that can measure text.
    #[allow(clippy::too_many_arguments)]
    fn resolve_html(
        &mut self,
        html: Option<&str>,
        asset: Option<&str>,
        css: Option<&str>,
        width: Option<BoxSize>,
        height: Option<BoxSize>,
        spath: &Path,
        assets: &BTreeMap<String, ResolvedAsset>,
        frame: FrameSize,
    ) -> ResolvedSource {
        // The box defaults to the frame, so markup is laid out on a
        // surface the size of the picture and CSS can place things in it
        // the way it places them on a page.
        let side = |r: &mut Self, size: Option<BoxSize>, name: &str, reference: f64| match size {
            None => Some(reference),
            Some(BoxSize::Auto) => None,
            Some(BoxSize::Fixed(l)) => {
                Some(r.positive_length(l, &spath.key(name), reference, name))
            }
        };
        let w = side(self, width, "width", f64::from(frame.width));
        let h = side(self, height, "height", f64::from(frame.height));
        let css = css.unwrap_or_default().to_owned();

        // Paths inside markup are relative to the markup itself, the way
        // they are on a page, so a card that opens in a browser finds the
        // same files geneva does.
        let mut base = String::new();
        let markup = match (html, asset) {
            (Some(_), Some(_)) => {
                self.push(
                    Diagnostic::error(
                        "E450",
                        spath.clone(),
                        "an html source has both \"html\" and \"asset\"",
                    )
                    .with_help("write the markup in the document or point at a file, not both"),
                );
                None
            }
            (Some(h), None) => Some(h.to_owned()),
            (None, Some(id)) => {
                let apath = spath.key("asset");
                if self
                    .asset_ref(id, &apath, assets, &[AssetKind::Html])
                    .is_none()
                {
                    None
                } else {
                    // Without a reader the markup is checked at render
                    // time; `validate --probe` supplies one.
                    let src = assets.get(id).map_or("", |a| a.src.as_str());
                    base = std::path::Path::new(src)
                        .parent()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    match self.info.text(id, src) {
                        Some(t) => Some(t),
                        None => {
                            return ResolvedSource::Html(Box::new(ResolvedHtml {
                                html: String::new(),
                                css,
                                base,
                                linked: BTreeMap::new(),
                                width: w,
                                height: h,
                                motion: Vec::new(),
                            }));
                        }
                    }
                }
            }
            (None, None) => {
                self.push(
                    Diagnostic::error(
                        "E450",
                        spath.clone(),
                        "an html source has neither \"html\" nor \"asset\"",
                    )
                    .with_help(
                        "add \"html\": \"<div>...</div>\" or point \"asset\" at an html file",
                    ),
                );
                None
            }
        };

        let markup = markup.unwrap_or_default();

        // Stylesheets the markup links to are read here, so a missing one
        // is an error before anything is drawn.
        let mut linked = BTreeMap::new();
        let mut motion = Vec::new();
        if let Ok(hrefs) = geneva_html::stylesheet_links(&markup) {
            for href in hrefs {
                let Some(path) = self.markup_path(&base, &href, spath, "stylesheet") else {
                    continue;
                };
                match self.info.read(&path) {
                    Some(text) => {
                        linked.insert(href, text);
                    }
                    // Nothing read is either no reader at all (validation
                    // without files) or a file that is not there; only the
                    // reader can tell the two apart.
                    None => {
                        if self.info.exists(&path) == Some(false) {
                            self.push(
                                Diagnostic::error(
                                    "E452",
                                    spath.clone(),
                                    format!(
                                        "the stylesheet {href:?} the markup links to is not there"
                                    ),
                                )
                                .with_value(path)
                                .with_help(
                                    "the path is relative to the markup, as it is in a browser",
                                ),
                            );
                        }
                    }
                }
            }
        }

        // The parsed tree is checked here and thrown away: taffy's style
        // is not Sync, and a resolved composition crosses threads. The
        // renderer parses it again once per clip and caches the picture.
        self.from_markup = None;
        match geneva_html::prepare(&markup, &css, &linked) {
            Ok(p) => {
                for problem in &p.problems {
                    self.push(
                        Diagnostic::warning("W450", spath.clone(), problem.clone()).with_help(
                            "it is skipped and the rest is drawn; the timeline reference lists what geneva draws",
                        ),
                    );
                }
                for what in &p.inert {
                    self.push(
                        Diagnostic::warning("W454", spath.clone(), what.clone()).with_help(
                            "add \"position: relative\" to the box, or put it in a flex container",
                        ),
                    );
                }
                for rule in &p.unmatched {
                    self.push(
                        Diagnostic::warning("W452", spath.clone(), rule.clone()).with_help(
                            "check the spelling against the markup; a stylesheet linked from the markup is not checked, since it is written for more than one file",
                        ),
                    );
                }
                // The markup's own motion: `@keyframes` for the clip to
                // play, and the `animation` its outermost element carries,
                // so a file that moves in a browser moves here too.
                let mut rules = BTreeMap::new();
                for (name, rule) in &p.keyframes {
                    let steps = self.parse_keyframe_rule(name, rule, spath);
                    rules.insert(name.clone(), steps);
                }
                let (bw, bh) = p.animated_box(
                    w.unwrap_or(f64::from(frame.width)) as f32,
                    h.unwrap_or(f64::from(frame.height)) as f32,
                );
                // Pictures the markup points at: checked here, loaded
                // when it is drawn.
                for src in geneva_html::image_sources(&p) {
                    let Some(path) = self.markup_path(&base, &src, spath, "picture") else {
                        continue;
                    };
                    if self.info.exists(&path) == Some(false) {
                        self.push(
                            Diagnostic::error(
                                "E452",
                                spath.clone(),
                                format!("the picture {src:?} the markup draws is not there"),
                            )
                            .with_value(path)
                            .with_help("the path is relative to the markup, as it is in a browser"),
                        );
                    }
                }
                // Families the markup's CSS names, checked the same way as
                // a text source's `font`. The path is the source rather
                // than a field of it, since the family is in the markup.
                for family in geneva_html::font_families(&p) {
                    self.check_font(Some(&family), spath, assets, true);
                }
                // Animations on elements inside: each is the renderer's
                // to play, composited as a group of its own.
                for (node, spec) in p.inner_animations() {
                    let el = p.doc.nodes[node].element();
                    let where_ = el.map_or_else(String::new, |e| {
                        e.id.as_deref().map_or_else(
                            || format!("<{}>", e.tag),
                            |i| format!("<{} id=\"{i}\">", e.tag),
                        )
                    });
                    let animations =
                        match parse_animation_spec(spec.shorthand.as_deref(), &spec.longhands) {
                            Ok(a) => a,
                            Err(e) => {
                                self.push(
                                    Diagnostic::error(
                                        "E441",
                                        spath.clone(),
                                        format!("{where_}: {e}"),
                                    )
                                    .with_value(json!(spec.shorthand.clone().unwrap_or_default())),
                                );
                                continue;
                            }
                        };
                    let mut plays = Vec::new();
                    for a in animations {
                        self.used_rules.insert(a.name.clone());
                        let Some(steps) = self.rules.get(&a.name).or_else(|| rules.get(&a.name))
                        else {
                            self.push(
                                Diagnostic::error(
                                    "E440",
                                    spath.clone(),
                                    format!("{where_}: no keyframes named {:?}", a.name),
                                )
                                .with_value(json!(a.name)),
                            );
                            continue;
                        };
                        plays.push(Play {
                            animation: a,
                            frames: steps.clone(),
                        });
                    }
                    if !plays.is_empty() {
                        motion.push(NodeMotion { node, plays });
                    }
                }
                self.from_markup = Some(Markup {
                    rules,
                    animation: p.animation,
                    animated_box: (bw.map(f64::from), bh.map(f64::from)),
                });
            }
            Err(e) => {
                let field = match (html, asset) {
                    (Some(_), _) => spath.key("html"),
                    _ => spath.key("asset"),
                };
                let mut d = Diagnostic::error("E451", field, e.message().to_owned());
                if e.line() > 0 {
                    d = d.with_location(e.line(), e.column().unwrap_or(1));
                }
                self.push(d);
            }
        }

        ResolvedSource::Html(Box::new(ResolvedHtml {
            html: markup,
            css,
            base,
            linked,
            width: w,
            height: h,
            motion,
        }))
    }

    /// A path written inside markup, joined to the markup's own directory
    /// and checked against the rule every asset path follows.
    fn markup_path(&mut self, base: &str, src: &str, spath: &Path, what: &str) -> Option<String> {
        if src.trim().is_empty() {
            self.push(Diagnostic::error(
                "E452",
                spath.clone(),
                format!("a {what} in the markup has an empty path"),
            ));
            return None;
        }
        if Self::escapes_root(src) {
            self.push(
                Diagnostic::error(
                    "E452",
                    spath.clone(),
                    format!("the {what} path {src:?} leaves the asset root"),
                )
                .with_value(src)
                .with_help(
                    "markup takes a path relative to itself, without \"..\", a leading \"/\" \
or a URL; pass --assets to choose the root",
                ),
            );
            return None;
        }
        Some(if base.is_empty() {
            src.to_owned()
        } else {
            format!("{}/{src}", base.trim_end_matches('/'))
        })
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
                        easing: k.ease.clone().unwrap_or_default(),
                    });
                }
                let first = out[0].value.clone();
                Track::new(out).unwrap_or_else(|| Track::constant(first))
            }
        }
    }

    /// A loudness target within what a meter can mean: a target between
    /// -40 and -5 LUFS, a true-peak ceiling between -20 and 0 dBTP.
    fn check_loudness(&mut self, loudness: &Loudness, path: &Path) {
        let target = loudness.target_lufs;
        if !target.is_finite() || !(-40.0..=-5.0).contains(&target) {
            self.push(
                Diagnostic::error(
                    "E423",
                    path.key("target_lufs"),
                    format!("{target} is outside the -40 to -5 LUFS a target can be"),
                )
                .with_value(json!(target))
                .with_help(
                    "platforms normalise to -14 or ask for -16; broadcast asks for -23 or -24",
                ),
            );
        }
        if let Some(peak) = loudness.true_peak_dbtp {
            if !peak.is_finite() || !(-20.0..=0.0).contains(&peak) {
                self.push(
                    Diagnostic::error(
                        "E423",
                        path.key("true_peak_dbtp"),
                        format!("{peak} is outside the -20 to 0 dBTP a ceiling can be"),
                    )
                    .with_value(json!(peak))
                    .with_help("-1 is what platforms ask for; -2 leaves room for a lossy encoder"),
                );
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
            let speed = self.speed_of(clip.speed, &cpath.key("speed"));
            let src_len = match src_len {
                ClipLength::Fixed(l) => ClipLength::Fixed(l / speed),
                ClipLength::Open => ClipLength::Open,
            };
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
                speed,
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
