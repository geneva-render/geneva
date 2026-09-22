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

#[test]
fn cue_times_are_relative_to_the_clip_start() {
    // A word file is timed from the start of the media it was made from,
    // so a clip that plays that media later in the output has to carry
    // its cues with it. Timing rule 6: every time in a document is
    // relative to the clip it is written on.
    let text = r#"{"geneva":"0.4","output":{"width":1280,"height":720,"fps":30,"duration":"20s"},
        "assets":{"c":{"src":"w.json"}},
        "layers":[{"id":"captions","clips":[
            {"source":{"kind":"captions","asset":"c"},"start":"4.4s"}]}]}"#;
    let timeline = geneva_timeline::parse(text).unwrap();
    let (composition, diagnostics) = resolve_with(&timeline, &Files(vec![("w.json", WHISPER)]));
    assert!(
        !diagnostics
            .iter()
            .any(geneva_timeline::Diagnostic::is_error),
        "{diagnostics:?}"
    );
    let layer = &composition.unwrap().layers[0];
    assert_eq!(layer.clips.len(), 2, "one clip per cue");

    // The cues sit at 1s and 4s in the file, so at 5.4s and 8.4s here.
    assert_eq!(layer.clips[0].start, Ratio::new(54, 10));
    assert_eq!(layer.clips[1].start, Ratio::new(84, 10));

    // The offset moves a cue, it does not stretch it: this one spans
    // 0.8s in the file and is held to the 1.2s `min_duration` either way.
    assert_eq!(
        layer.clips[0].end - layer.clips[0].start,
        Ratio::new(12, 10)
    );
}

#[test]
fn max_chars_sets_the_line_budget() {
    // Grouping runs before `style` and has no font to measure with, so
    // the budget is `max_lines` lines of `max_chars` characters and
    // nothing else. A narrow line has to give more cues than a wide one
    // over the same words.
    let words = r#"{"words": [
        {"word": "the", "start": 0.0, "end": 0.4},
        {"word": "quick", "start": 0.4, "end": 0.8},
        {"word": "brown", "start": 0.8, "end": 1.2},
        {"word": "fox", "start": 1.2, "end": 1.6},
        {"word": "jumps", "start": 1.6, "end": 2.0},
        {"word": "over", "start": 2.0, "end": 2.4},
        {"word": "the", "start": 2.4, "end": 2.8},
        {"word": "lazy", "start": 2.8, "end": 3.2},
        {"word": "dog", "start": 3.2, "end": 3.6}
    ]}"#;
    let count = |source: &str| {
        let l = load("w.json", source, vec![("w.json", words)]);
        assert!(l.is_ok(), "{:?}", l.diagnostics);
        l.composition.unwrap().layers[0].clips.len()
    };

    let wide = count(r#"{"kind":"captions","asset":"c","max_chars":40}"#);
    let narrow = count(r#"{"kind":"captions","asset":"c","max_chars":10}"#);
    assert!(
        narrow > wide,
        "a 10-character line should cut more cues than a 40-character one: {narrow} vs {wide}"
    );

    // Left out, it is the broadcast convention of 42, which over these
    // words agrees with asking for 40.
    assert_eq!(count(r#"{"kind":"captions","asset":"c"}"#), wide);
}

#[test]
fn a_line_too_short_to_hold_a_word_is_refused() {
    let l = load(
        "w.json",
        r#"{"kind":"captions","asset":"c","max_chars":3}"#,
        vec![("w.json", WHISPER)],
    );
    assert!(codes(&l).contains(&"E402"), "{:?}", codes(&l));
}

#[test]
fn the_cue_count_says_what_budget_grouped_it() {
    let l = load(
        "w.json",
        r#"{"kind":"captions","asset":"c","max_lines":1,"max_chars":20}"#,
        vec![("w.json", WHISPER)],
    );
    let note = l
        .diagnostics
        .iter()
        .find(|d| d.code == "N453")
        .expect("a cue count");
    assert!(
        note.message.contains("1 line of 20 characters"),
        "{}",
        note.message
    );

    // A cue file is not grouped, so there is no budget to report.
    let cues = load(
        "c.srt",
        r#"{"kind":"captions","asset":"c"}"#,
        vec![("c.srt", SRT)],
    );
    let note = cues
        .diagnostics
        .iter()
        .find(|d| d.code == "N453")
        .expect("a cue count");
    assert!(!note.message.contains("grouped at"), "{}", note.message);
}
