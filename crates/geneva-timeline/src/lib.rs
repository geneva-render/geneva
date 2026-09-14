//! The Geneva timeline format.
//!
//! A timeline is a JSON document describing a composition: output settings,
//! a table of assets, visual layers made of clips, and audio tracks. This
//! crate parses that document, validates it with precise diagnostics, and
//! resolves it into a [`Composition`] whose times are exact and whose
//! animated properties are ready to sample.
//!
//! ```
//! let text = r##"{
//!   "geneva": "0.1",
//!   "output": { "width": 640, "height": 360, "fps": 30, "duration": "2s" },
//!   "layers": [ { "clips": [ { "source": { "kind": "solid", "color": "#336699" } } ] } ]
//! }"##;
//! let loaded = geneva_timeline::load(text);
//! assert!(loaded.diagnostics.iter().all(|d| !d.is_error()));
//! let comp = loaded.composition.unwrap();
//! assert_eq!(comp.frame_count(), 60);
//! ```

#![forbid(unsafe_code)]

mod animated;
mod color;
pub mod css;
mod diagnostic;
mod json_schema;
mod length;
mod parse;
mod ratio;
mod resolve;
pub mod schema;
mod time;

pub use animated::{Animated, KeyframeSpec};
pub use color::ColorValue;
pub use diagnostic::{Diagnostic, Path, Severity, Summary, summarize};
pub use json_schema::{json_schema, schema_url};
pub use length::{Length, Point, Scale};
pub use parse::parse;
pub use ratio::Ratio;
pub use resolve::{
    AssetInfo, Composition, NoAssetInfo, ResolvedAsset, ResolvedAudioClip, ResolvedAudioTrack,
    ResolvedClip, ResolvedComposition, ResolvedEffect, ResolvedLayer, ResolvedMask, ResolvedOutput,
    ResolvedSource, ResolvedSubtitleTrack, ResolvedText, resolve, resolve_with,
};
pub use schema::{FORMAT_VERSION, Timeline};
pub use time::{Fps, Time};

/// The outcome of loading a timeline: diagnostics plus, when there were no
/// errors, the resolved composition.
#[derive(Debug)]
pub struct Loaded {
    /// The parsed document, when parsing succeeded.
    pub timeline: Option<Timeline>,
    /// The resolved composition, when there were no errors.
    pub composition: Option<Composition>,
    /// Everything found along the way, errors first.
    pub diagnostics: Vec<Diagnostic>,
}

impl Loaded {
    /// True when no diagnostic is an error.
    pub fn is_ok(&self) -> bool {
        self.diagnostics.iter().all(|d| !d.is_error())
    }
}

/// Parses, validates and resolves timeline JSON in one step, without
/// reading any asset files.
pub fn load(text: &str) -> Loaded {
    load_with(text, &NoAssetInfo)
}

/// Like [`load`], with asset information for closing open-ended media
/// clips at the end of their files.
pub fn load_with(text: &str, info: &dyn AssetInfo) -> Loaded {
    let timeline = match parse(text) {
        Ok(tl) => tl,
        Err(d) => {
            return Loaded {
                timeline: None,
                composition: None,
                diagnostics: vec![d],
            };
        }
    };
    let (composition, mut diagnostics) = resolve_with(&timeline, info);
    diagnostics.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.path.cmp(&b.path))
    });
    Loaded {
        timeline: Some(timeline),
        composition,
        diagnostics,
    }
}

/// Validates a parsed timeline, returning every diagnostic found.
pub fn validate(timeline: &Timeline) -> Vec<Diagnostic> {
    resolve(timeline).1
}
