//! Destination targets for `--for`: what a file can be, decided from the
//! source and where the file is going.
//!
//! One table, printed by `geneva targets`, and a picker that writes its
//! choices into the explicit encode block, so `--show-timeline` shows
//! exactly what was chosen and a hand-written timeline can say the same
//! without the flag. What a particular viewer gets (a rendition chosen at
//! play time from a ladder) is the hosted product's job, not this table's.

use geneva_timeline::schema::{
    AudioCodec, AudioEncode, Encode, Fit, Source, Timeline, VideoCodec, VideoEncode,
};
use geneva_timeline::{Diagnostic, Fps, Ratio};

/// Quality tiers, as constant-rate-factor steps per target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Quality {
    /// Larger files, visually transparent.
    Best,
    /// The everyday choice.
    Good,
    /// Smaller files, softer picture.
    Eco,
}

/// A destination and what it needs.
pub struct Target {
    pub name: &'static str,
    pub about: &'static str,
    /// Resolution ceiling; a smaller source is never upscaled. For a
    /// portrait target the two are the width and height of the 9:16
    /// canvas.
    pub max_width: u32,
    pub max_height: u32,
    /// Fixed 9:16 orientation: a landscape source is fitted onto a
    /// portrait canvas.
    pub portrait: bool,
    pub max_fps: f64,
    /// CRF for best, good and eco at 1080p and below; two less at 4K.
    pub crf: [u8; 3],
    /// Bitrate ceilings in kb/s by the shorter side of the output, at up
    /// to 30 fps (half again above); the first entry at least as large
    /// as the output applies. Empty means no ceiling.
    pub caps: &'static [(u32, u32)],
    /// Seconds between keyframes.
    pub keyframe_interval: f64,
    /// AAC bitrate in kb/s.
    pub audio_kbps: u32,
    /// The platform's duration limit, warned about, never enforced.
    pub max_seconds: Option<f64>,
    /// The platform's size limit, which also caps the bitrate.
    pub max_bytes: Option<u64>,
    /// Where the numbers come from and when they were last checked.
    pub checked: &'static str,
}

/// Ceilings for playback on the open web, by the shorter side.
const WEB_CAPS: &[(u32, u32)] = &[
    (360, 1500),
    (480, 3000),
    (720, 6000),
    (1080, 10000),
    (1440, 20000),
    (2160, 45000),
];

/// YouTube's recommended upload bitrates (SDR, up to 30 fps).
const YOUTUBE_CAPS: &[(u32, u32)] = &[
    (360, 1000),
    (480, 2500),
    (720, 5000),
    (1080, 8000),
    (1440, 16000),
    (2160, 45000),
];

/// The table. Device classes first, then platforms.
pub const TARGETS: &[Target] = &[
    Target {
        name: "phone",
        about: "playback on a phone, fullscreen",
        max_width: 1920,
        max_height: 1080,
        portrait: false,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 128,
        max_seconds: None,
        max_bytes: None,
        checked: "device class: 1080p covers current phones fullscreen at 3x pixel density",
    },
    Target {
        name: "tablet",
        about: "playback on a tablet",
        max_width: 2560,
        max_height: 1600,
        portrait: false,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 160,
        max_seconds: None,
        max_bytes: None,
        checked: "device class",
    },
    Target {
        name: "desktop",
        about: "playback on a laptop or desktop screen",
        max_width: 2560,
        max_height: 1440,
        portrait: false,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 160,
        max_seconds: None,
        max_bytes: None,
        checked: "device class",
    },
    Target {
        name: "tv",
        about: "playback on a television, 4K sets included",
        max_width: 3840,
        max_height: 2160,
        portrait: false,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 192,
        max_seconds: None,
        max_bytes: None,
        checked: "device class",
    },
    Target {
        name: "web",
        about: "a page or player on the open web (HTML5 video)",
        max_width: 1920,
        max_height: 1080,
        portrait: false,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 128,
        max_seconds: None,
        max_bytes: None,
        checked: "device class",
    },
    Target {
        name: "youtube",
        about: "upload to YouTube (it re-encodes; upload quality high)",
        max_width: 3840,
        max_height: 2160,
        portrait: false,
        max_fps: 60.0,
        crf: [18, 20, 23],
        caps: YOUTUBE_CAPS,
        keyframe_interval: 0.5,
        audio_kbps: 384,
        max_seconds: Some(43_200.0),
        max_bytes: Some(256_000_000_000),
        checked: "2026-09, support.google.com/youtube/answer/1722171",
    },
    Target {
        name: "instagram",
        about: "Instagram Reels, 9:16",
        max_width: 1440,
        max_height: 2560,
        portrait: true,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 128,
        max_seconds: Some(900.0),
        max_bytes: Some(4_000_000_000),
        checked: "2026-09, facebook.com/business/ads-guide/update/video/instagram-reels",
    },
    Target {
        name: "tiktok",
        about: "TikTok, 9:16",
        max_width: 1080,
        max_height: 1920,
        portrait: true,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 128,
        max_seconds: Some(600.0),
        max_bytes: Some(500_000_000),
        checked: "2026-09, ads.tiktok.com/help/article/video-ads-specifications",
    },
    Target {
        name: "x",
        about: "a post on X",
        max_width: 1920,
        max_height: 1200,
        portrait: false,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 128,
        max_seconds: Some(140.0),
        max_bytes: Some(512_000_000),
        checked: "not re-checked (help.x.com refuses automated reads); as published: 2:20, 512 MB, 1920x1200",
    },
    Target {
        name: "linkedin",
        about: "a native video post on LinkedIn",
        max_width: 4096,
        max_height: 2304,
        portrait: false,
        max_fps: 60.0,
        crf: [20, 23, 26],
        caps: WEB_CAPS,
        keyframe_interval: 2.0,
        audio_kbps: 128,
        max_seconds: Some(600.0),
        max_bytes: Some(5_000_000_000),
        checked: "not re-checked (help pages not reachable); as published: 10 min, 5 GB, 4096x2304",
    },
    Target {
        name: "email",
        about: "an attachment that has to get through",
        max_width: 1280,
        max_height: 720,
        portrait: false,
        max_fps: 60.0,
        crf: [23, 26, 28],
        caps: &[(360, 800), (480, 1500), (720, 2500)],
        keyframe_interval: 2.0,
        audio_kbps: 96,
        max_seconds: None,
        max_bytes: Some(25_000_000),
        checked: "geneva's own budget: 25 MB, the common attachment limit",
    },
];

/// The target of that name, if any.
pub fn find(name: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

/// The names, comma-separated, for messages.
pub fn names() -> String {
    TARGETS
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parses a size such as `25MB`, `1.5 GB`, `800KB` or `20000000` (bytes);
/// units are decimal (a megabyte is a million bytes, as platforms count).
pub fn parse_budget(text: &str) -> Result<u64, String> {
    let t = text.trim().to_ascii_lowercase();
    let (number, unit) = match t.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (t[..i].trim(), t[i..].trim()),
        None => (t.as_str(), ""),
    };
    let value: f64 = number
        .parse()
        .map_err(|_| format!("{text:?} is not a size such as 25MB"))?;
    let factor = match unit {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        _ => return Err(format!("{text:?}: unknown unit {unit:?}; use KB, MB or GB")),
    };
    let bytes = value * factor;
    if bytes.is_nan() || bytes < 1.0 {
        return Err(format!("{text:?} is not a positive size"));
    }
    Ok(bytes as u64)
}

/// Facts about the composition the target is applied to.
pub struct Facts {
    pub width: u32,
    pub height: u32,
    pub fps: Ratio,
    pub duration: Option<Ratio>,
    /// Whether an audio track will be written.
    pub audio: bool,
}

/// How the target is applied.
pub struct Options<'a> {
    pub target: &'a Target,
    pub quality: Option<Quality>,
    /// A size budget in bytes, on top of the target's own limit.
    pub budget: Option<u64>,
    /// Explicit choices that win over the target's.
    pub crf: Option<u8>,
    pub codec: Option<VideoCodec>,
    /// Whether the picture may be scaled to the ceiling (verbs) or must
    /// keep its size (`render`, whose layout is the author's).
    pub resize: bool,
    /// The output file's extension, to warn when it is not MP4.
    pub extension: Option<&'a str>,
}

fn even(v: f64) -> u32 {
    let n = v.round() as u32;
    (if n % 2 == 1 { n + 1 } else { n }).max(2)
}

/// The H.264 level a picture of `long`×`short` pixels at `fps` needs.
fn h264_level(long: u32, short: u32, fps: f64) -> &'static str {
    let px = u64::from(long) * u64::from(short);
    let hfr = fps > 30.5;
    if px <= 1280 * 720 {
        if hfr { "3.2" } else { "3.1" }
    } else if px <= 1920 * 1080 {
        if hfr { "4.2" } else { "4.0" }
    } else if px <= 2560 * 1600 {
        if hfr { "5.1" } else { "5.0" }
    } else if hfr {
        "5.2"
    } else {
        "5.1"
    }
}

/// Applies the target to `tl` and returns the report: one note with
/// every choice and its reason, and warnings for limits the encode
/// cannot satisfy.
pub fn apply(tl: &mut Timeline, facts: &Facts, opts: &Options<'_>) -> Vec<Diagnostic> {
    let t = opts.target;
    let mut why: Vec<String> = Vec::new();
    let mut diagnostics = Vec::new();
    let (w, h) = (facts.width, facts.height);
    // A `cover` fit asked for on the verb crops instead of leaving bars.
    let cover = tl
        .layers
        .iter()
        .flat_map(|l| &l.clips)
        .any(|c| matches!(c.source, Source::Video { .. }) && c.fit == Some(Fit::Cover));

    // Size: never upscaled; fitted onto a portrait canvas when the
    // target's orientation is fixed. With bars the canvas keeps the
    // source's width; cropped, it keeps the source's height.
    let (ow, oh) = if !opts.resize {
        (w, h)
    } else if t.portrait && w > h && cover {
        let ch = h.min(t.max_height);
        (even(f64::from(ch) * 9.0 / 16.0), even(f64::from(ch)))
    } else if t.portrait && w > h {
        let cw = w.min(t.max_width);
        (even(f64::from(cw)), even(f64::from(cw) * 16.0 / 9.0))
    } else {
        let (long, short) = (w.max(h), w.min(h));
        let (max_long, max_short) = (t.max_width.max(t.max_height), t.max_width.min(t.max_height));
        let scale = (f64::from(max_long) / f64::from(long))
            .min(f64::from(max_short) / f64::from(short))
            .min(1.0);
        (even(f64::from(w) * scale), even(f64::from(h) * scale))
    };
    if (ow, oh) != (w, h) {
        tl.output.width = ow;
        tl.output.height = oh;
        for layer in &mut tl.layers {
            for clip in &mut layer.clips {
                if matches!(clip.source, Source::Video { .. }) && clip.fit.is_none() {
                    clip.fit = Some(Fit::Contain);
                }
            }
        }
        if t.portrait && w > h && cover {
            why.push(format!(
                "{w}×{h} cropped to its center {ow}×{oh} (9:16, never upscaled)"
            ));
        } else if t.portrait && w > h {
            why.push(format!(
                "{w}×{h} fitted onto a {ow}×{oh} portrait canvas (9:16, never upscaled; --fit cover crops instead)"
            ));
        } else {
            why.push(format!(
                "{w}×{h} scaled to {ow}×{oh} (ceiling {}×{})",
                t.max_width, t.max_height
            ));
        }
    } else if opts.resize {
        why.push(format!(
            "{w}×{h} kept (ceiling {}×{}, never upscaled)",
            t.max_width, t.max_height
        ));
    } else {
        why.push(format!("{w}×{h} as the timeline says"));
    }

    // Frame rate: capped only at the target's maximum.
    let mut fps = facts.fps.to_f64();
    if fps > t.max_fps + 0.01 {
        tl.output.fps = Fps(Ratio::from_int(t.max_fps.round() as i64));
        why.push(format!("{fps:.3} fps capped at {} fps", t.max_fps.round()));
        fps = t.max_fps;
    }

    // Codec, quality and level.
    let codec = opts.codec.unwrap_or(VideoCodec::H264);
    let tier = match opts.quality.unwrap_or(Quality::Good) {
        Quality::Best => 0,
        Quality::Good => 1,
        Quality::Eco => 2,
    };
    let short = ow.min(oh);
    let mut crf = t.crf[tier];
    if short >= 2000 {
        crf = crf.saturating_sub(2);
    }
    let crf = match opts.crf {
        Some(c) => {
            why.push(format!("CRF {c} (yours)"));
            c
        }
        None => {
            why.push(format!("CRF {crf} ({})", ["best", "good", "eco"][tier]));
            crf
        }
    };
    let level = matches!(codec, VideoCodec::H264 | VideoCodec::H265)
        .then(|| h264_level(ow.max(oh), short, fps).to_owned());
    match (&opts.codec, &level) {
        (Some(c), Some(l)) => why.push(format!("{c:?} (yours) level {l}")),
        (Some(c), None) => why.push(format!("{c:?} (yours)")),
        (None, Some(l)) => why.push(format!("H.264 High level {l}")),
        (None, None) => {}
    }

    // Bitrate ceiling: the target's for this size, raised for high frame
    // rates, and lowered to fit a size budget when there is one.
    let hfr = fps > 30.5;
    let mut cap = t
        .caps
        .iter()
        .find(|(lines, _)| *lines >= short)
        .or(t.caps.last())
        .map(|(_, kbps)| if hfr { kbps * 3 / 2 } else { *kbps });
    if let Some(c) = cap {
        why.push(format!("capped at {c} kb/s"));
    }
    let budget = match (opts.budget, t.max_bytes) {
        (Some(b), Some(m)) => Some(b.min(m)),
        (b, m) => b.or(m),
    };
    if let (Some(bytes), Some(duration)) = (budget, facts.duration) {
        let secs = duration.to_f64();
        if secs > 0.0 {
            let audio = if facts.audio {
                f64::from(t.audio_kbps)
            } else {
                0.0
            };
            let kbps = (bytes as f64 * 8.0 * 0.92 / secs / 1000.0 - audio).floor();
            if kbps < 200.0 {
                diagnostics.push(
                    Diagnostic::warning(
                        "W414",
                        "/output/encode",
                        format!(
                            "{} in {:.0} s leaves under 200 kb/s for the picture; the budget cannot be met at this length",
                            human_size(bytes),
                            secs
                        ),
                    )
                    .with_help("shorten the video, or raise the budget"),
                );
            } else if cap.is_none_or(|c| f64::from(c) > kbps) {
                cap = Some(kbps as u32);
                why.push(format!(
                    "capped at {} kb/s so that {:.0} s fits {}",
                    kbps as u32,
                    secs,
                    human_size(bytes)
                ));
            }
        }
    }

    why.push(format!("keyframes every {} s", t.keyframe_interval));
    why.push("fast start".to_owned());
    if facts.audio {
        why.push(format!("AAC {} kb/s 48 kHz stereo", t.audio_kbps));
    }

    // Into the encode block, explicit.
    let encode = tl.output.encode.get_or_insert(Encode {
        container: None,
        video: None,
        audio: None,
        fast_start: None,
    });
    let video = encode.video.get_or_insert(VideoEncode {
        codec: None,
        crf: None,
        preset: None,
        hardware: None,
        profile: None,
        keyframe_interval: None,
        max_bitrate_kbps: None,
        level: None,
    });
    video.codec = Some(codec);
    video.crf = Some(crf);
    video.keyframe_interval = Some(t.keyframe_interval);
    video.max_bitrate_kbps = cap;
    video.level = level;
    encode.fast_start = Some(true);
    if facts.audio {
        let audio = encode.audio.get_or_insert(AudioEncode {
            codec: None,
            bitrate_kbps: None,
        });
        audio.codec = Some(AudioCodec::Aac);
        audio.bitrate_kbps = Some(t.audio_kbps);
    }

    diagnostics.push(Diagnostic::note(
        "N410",
        "/output/encode",
        format!("target {}: {}", t.name, why.join("; ")),
    ));
    if let (Some(max), Some(duration)) = (t.max_seconds, facts.duration) {
        let secs = duration.to_f64();
        if secs > max {
            diagnostics.push(
                Diagnostic::warning(
                    "W411",
                    "/output/duration",
                    format!(
                        "{:.1} s is longer than {} allows ({})",
                        secs,
                        t.name,
                        human_seconds(max)
                    ),
                )
                .with_help("trim the video; geneva never shortens it on its own"),
            );
        }
    }
    if let Some(ext) = opts.extension {
        if !matches!(ext.to_ascii_lowercase().as_str(), "mp4" | "m4v" | "mov") {
            diagnostics.push(
                Diagnostic::warning(
                    "W413",
                    "/output/encode/container",
                    format!("{} expects an MP4 file; the output is .{ext}", t.name),
                )
                .with_help("name the output .mp4"),
            );
        }
    }
    diagnostics
}

/// `25 MB`, `1.5 GB`, `800 KB`.
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

fn human_seconds(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{} h", s / 3600)
    } else if s >= 60 && s % 60 == 0 {
        format!("{} min", s / 60)
    } else if s >= 60 {
        format!("{}:{:02} min", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// The table as rows for the report.
pub fn table() -> Vec<serde_json::Value> {
    TARGETS
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "about": t.about,
                "max_width": t.max_width,
                "max_height": t.max_height,
                "portrait": t.portrait,
                "max_fps": t.max_fps,
                "crf": { "best": t.crf[0], "good": t.crf[1], "eco": t.crf[2] },
                "bitrate_caps_kbps": t.caps.iter().map(|(l, k)| serde_json::json!({ "lines": l, "kbps": k })).collect::<Vec<_>>(),
                "keyframe_interval": t.keyframe_interval,
                "audio_kbps": t.audio_kbps,
                "max_seconds": t.max_seconds,
                "max_bytes": t.max_bytes,
                "checked": t.checked,
            })
        })
        .collect()
}

/// The table as text.
pub fn print_table() {
    println!("target     ceiling     crf b/g/e   audio    limits     about");
    for t in TARGETS {
        let mut limits = Vec::new();
        if let Some(s) = t.max_seconds {
            limits.push(human_seconds(s));
        }
        if let Some(b) = t.max_bytes {
            limits.push(human_size(b));
        }
        println!(
            "{:<10} {:<11} {:<11} {:<8} {:<10} {}",
            t.name,
            format!("{}x{}", t.max_width, t.max_height),
            format!("{}/{}/{}", t.crf[0], t.crf[1], t.crf[2]),
            format!("{} kb/s", t.audio_kbps),
            if limits.is_empty() {
                "-".to_owned()
            } else {
                limits.join(", ")
            },
            t.about
        );
        println!("{:<10} checked: {}", "", t.checked);
    }
}
