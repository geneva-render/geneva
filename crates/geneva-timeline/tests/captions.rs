//! The captions source: a file in, one clip per cue out.

use geneva_timeline::{AssetInfo, Ratio, ResolvedSource, resolve_with};

/// An asset reader holding files in memory, as `--probe` holds them.
struct Files(Vec<(&'static str, &'static str)>);

impl AssetInfo for Files {
    fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
        None
    }

    fn text(&self, _: &str, src: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(name, _)| *name == src)
            .map(|(_, body)| (*body).to_owned())
    }

    fn exists(&self, path: &str) -> Option<bool> {
        Some(self.0.iter().any(|(name, _)| *name == path))
    }
}

const WHISPER: &str = r#"{"segments": [
  {"words": [{"word": " Dragon", "start": 1.0, "end": 1.5, "probability": 0.9},
             {"word": " is", "start": 1.5, "end": 1.8, "probability": 0.9}]},
  {"words": [{"word": " captured", "start": 4.0, "end": 4.8, "probability": 0.9}]}
]}"#;

const SRT: &str = "1\n00:00:01,000 --> 00:00:02,000\nDragon is\n\n\
                   2\n00:00:04,000 --> 00:00:05,000\ncaptured\n";

fn load(
    src: &str,
    source: &str,
    files: Vec<(&'static str, &'static str)>,
) -> geneva_timeline::Loaded {
    let text = format!(
        r#"{{"geneva":"0.3","output":{{"width":1280,"height":720,"fps":30,"duration":"8s"}},
        "assets":{{"c":{{"src":"{src}"}}}},
        "layers":[{{"id":"captions","clips":[{{"source":{source}}}]}}]}}"#
    );
    let timeline = geneva_timeline::parse(&text).unwrap();
    let (composition, diagnostics) = resolve_with(&timeline, &Files(files));
    geneva_timeline::Loaded {
        timeline: Some(timeline),
        composition,
        diagnostics,
    }
}

fn codes(l: &geneva_timeline::Loaded) -> Vec<&'static str> {
    l.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn a_word_file_becomes_one_clip_per_cue() {
    let l = load(
        "w.json",
        r#"{"kind":"captions","asset":"c"}"#,
        vec![("w.json", WHISPER)],
    );
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    let layer = &l.composition.unwrap().layers[0];
    assert_eq!(layer.clips.len(), 2, "one clip per cue");

    // Times come from the file, not from start and duration.
    assert_eq!(layer.clips[0].start, Ratio::from_int(1));
    assert_eq!(layer.clips[1].start, Ratio::from_int(4));

    // The gap between them is a hole the copy planner can walk through.
    assert!(layer.clips[0].end < layer.clips[1].start);

    let ResolvedSource::Text(t) = &layer.clips[0].source else {
        panic!("expected text");
    };
    assert_eq!(t.text, "Dragon is");
    // Word times are relative to the clip, which starts where the cue does.
    assert_eq!(t.words[0].1, Ratio::ZERO);
    assert_eq!(t.words[1].1, Ratio::new(1, 2));
}

#[test]
fn captions_sit_inside_the_title_safe_area() {
    let l = load(
        "w.json",
        r#"{"kind":"captions","asset":"c"}"#,
        vec![("w.json", WHISPER)],
    );
    let clip = &l.composition.unwrap().layers[0].clips[0];
    // Anchored at its bottom edge, 5% of 720 up from the frame's.
    assert_eq!(clip.anchor.y.to_px(720.0), 720.0);
    assert_eq!(clip.position.sample(0.0)[1], 720.0 - 36.0);

    let top = load(
        "w.json",
        r#"{"kind":"captions","asset":"c","position":"top","margin":50}"#,
        vec![("w.json", WHISPER)],
    );
    let clip = &top.composition.unwrap().layers[0].clips[0];
    assert_eq!(clip.anchor.y.to_px(720.0), 0.0);
    assert_eq!(clip.position.sample(0.0)[1], 50.0);
}

#[test]
fn a_subtitle_file_works_but_cannot_be_highlighted() {
    let l = load(
        "c.srt",
        r#"{"kind":"captions","asset":"c","style":{"highlight":{"color":"red"}}}"#,
        vec![("c.srt", SRT)],
    );
    assert!(l.is_ok(), "{:?}", l.diagnostics);
    assert_eq!(l.composition.as_ref().unwrap().layers[0].clips.len(), 2);
    assert!(codes(&l).contains(&"W453"), "{:?}", codes(&l));

    let ResolvedSource::Text(t) = &l.composition.unwrap().layers[0].clips[0].source else {
        panic!("expected text");
    };
    assert!(t.words.is_empty(), "a cue has no word times to pick from");
}

#[test]
fn webvtt_placement_is_followed_unless_the_document_says_otherwise() {
    let vtt = "WEBVTT\n\n00:01.000 --> 00:02.000 line:20% position:30%\nUp here\n";
    let l = load(
        "c.vtt",
        r#"{"kind":"captions","asset":"c"}"#,
        vec![("c.vtt", vtt)],
    );
    let clip = &l.composition.unwrap().layers[0].clips[0];
    assert_eq!(clip.position.sample(0.0), [1280.0 * 0.3, 720.0 * 0.2]);

    let fixed = load(
        "c.vtt",
        r#"{"kind":"captions","asset":"c","follow_file":false}"#,
        vec![("c.vtt", vtt)],
    );
    let clip = &fixed.composition.unwrap().layers[0].clips[0];
    assert_eq!(clip.position.sample(0.0), [640.0, 720.0 - 36.0]);
}

#[test]
fn a_file_that_is_not_there_or_will_not_read_says_so() {
    let missing = load(
        "gone.json",
        r#"{"kind":"captions","asset":"c"}"#,
        vec![("other.json", WHISPER)],
    );
    assert!(codes(&missing).contains(&"E453"), "{:?}", codes(&missing));

    let empty = load(
        "w.json",
        r#"{"kind":"captions","asset":"c"}"#,
        vec![("w.json", r#"{"text": "no words here"}"#)],
    );
    assert!(codes(&empty).contains(&"E453"), "{:?}", codes(&empty));
}
