//! Caption files: the cues a document draws, and where they come from.
//!
//! Three shapes arrive here. SubRip and WebVTT carry cues with times and
//! text, and WebVTT can say where a cue sits. A word file, the JSON a
//! speech recogniser writes, carries one entry per word, which is what
//! makes a word-by-word highlight possible; those are grouped into cues
//! here rather than in the renderer, so the resolver can lay each cue on
//! the timeline as its own clip and the copy planner can copy the gaps
//! between them.
//!
//! This lives beside the document model rather than beside the media
//! code because it is parsing, not decoding, and because the resolver
//! needs it before anything is drawn.

use std::fmt;

use crate::ratio::Ratio;

/// Where a WebVTT cue asked to sit. Everything is optional: a file that
/// says nothing leaves the document's own placement alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Placement {
    /// Vertical position as a percentage of the frame, 0 at the top, as
    /// WebVTT's `line:` writes it.
    pub line: Option<i32>,
    /// Horizontal position as a percentage, from `position:`.
    pub position: Option<i32>,
    /// Text alignment, from `align:`.
    pub align: Option<Align>,
    /// Width as a percentage of the frame, from `size:`.
    pub size: Option<i32>,
}

impl Placement {
    /// Whether the file said anything at all about where the cue goes.
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// How a cue's text lines up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Against the start edge.
    Left,
    /// Centred.
    Center,
    /// Against the end edge.
    Right,
}

/// One timed piece of text, with the words inside it when they are known.
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    /// When the text appears, in seconds.
    pub start: Ratio,
    /// When it disappears, in seconds.
    pub end: Ratio,
    /// The text, lines separated by `\n`; may carry simple `<i>`-style
    /// tags as written in the file.
    pub text: String,
    /// The words and their times, for a file that has them. A cue read
    /// from SubRip or WebVTT has none, so nothing can be picked out.
    pub words: Vec<Word>,
    /// Where the file asked the cue to sit.
    pub place: Placement,
}

impl Cue {
    /// A cue with nothing but a time and some text.
    pub fn plain(start: Ratio, end: Ratio, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            words: Vec::new(),
            place: Placement::default(),
        }
    }
}

/// One word and when it is spoken, in seconds from the start of the media.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// The word, with the surrounding space a recogniser leaves trimmed.
    pub text: String,
    /// When it starts.
    pub start: Ratio,
    /// When it ends.
    pub end: Ratio,
}

/// Why a caption file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionError(String);

impl fmt::Display for CaptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CaptionError {}

fn bad(reason: impl Into<String>) -> CaptionError {
    CaptionError(reason.into())
}

/// The file formats a caption source reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionFormat {
    /// SubRip.
    Srt,
    /// WebVTT.
    WebVtt,
    /// The JSON a speech recogniser writes, one entry per word.
    Words,
}

impl CaptionFormat {
    /// Picks the format from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "srt" => Some(Self::Srt),
            "vtt" => Some(Self::WebVtt),
            "json" => Some(Self::Words),
            _ => None,
        }
    }
}

/// Parses SubRip or WebVTT text, telling them apart by the `WEBVTT`
/// header. Cues come back in file order.
pub fn parse(text: &str) -> Result<Vec<Cue>, CaptionError> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with("WEBVTT") {
        parse_webvtt(text)
    } else {
        parse_srt(text)
    }
}

/// Parses SubRip text.
pub fn parse_srt(text: &str) -> Result<Vec<Cue>, CaptionError> {
    let mut cues = Vec::new();
    for block in blocks(text) {
        let mut lines = block.iter().copied();
        let Some(mut line) = lines.next() else {
            continue;
        };
        // The optional index line precedes the timing line.
        if !line.contains("-->") {
            line = match lines.next() {
                Some(l) => l,
                None => continue,
            };
        }
        let (start, end) = timing(line)?;
        let body: Vec<&str> = lines.collect();
        cues.push(Cue::plain(start, end, body.join("\n")));
    }
    Ok(cues)
}

/// Parses WebVTT text. Header, `NOTE`, `STYLE` and `REGION` blocks are
/// skipped; a cue's settings are kept.
pub fn parse_webvtt(text: &str) -> Result<Vec<Cue>, CaptionError> {
    let mut cues = Vec::new();
    let mut first = true;
    for block in blocks(text) {
        if first {
            first = false;
            if block.first().is_some_and(|l| l.starts_with("WEBVTT")) {
                continue;
            }
        }
        let head = block.first().copied().unwrap_or("");
        if head.starts_with("NOTE") || head.starts_with("STYLE") || head.starts_with("REGION") {
            continue;
        }
        let Some(pos) = block.iter().position(|l| l.contains("-->")) else {
            continue;
        };
        let (start, end) = timing(block[pos])?;
        let mut cue = Cue::plain(start, end, block[pos + 1..].join("\n"));
        cue.place = settings(block[pos]);
        cues.push(cue);
    }
    Ok(cues)
}

/// Reads a WebVTT cue's settings, which follow the timing on the same
/// line. Anything it does not understand (`vertical`, `region`, a line
/// given as a row number rather than a percentage) is left alone.
fn settings(line: &str) -> Placement {
    let mut p = Placement::default();
    let Some((_, rest)) = line.split_once("-->") else {
        return p;
    };
    // The first token after the arrow is the end timestamp.
    for token in rest.split_whitespace().skip(1) {
        let Some((name, value)) = token.split_once(':') else {
            continue;
        };
        // A value may carry an alignment after a comma, as in "line:90%,end".
        let value = value.split(',').next().unwrap_or(value);
        match name {
            "line" => p.line = percent(value),
            "position" => p.position = percent(value),
            "size" => p.size = percent(value),
            "align" => {
                p.align = match value {
                    "start" | "left" => Some(Align::Left),
                    "center" | "middle" => Some(Align::Center),
                    "end" | "right" => Some(Align::Right),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    p
}

fn percent(value: &str) -> Option<i32> {
    let body = value.strip_suffix('%')?;
    body.parse::<f64>().ok().map(|v| v.round() as i32)
}

/// Splits text into blank-line-separated blocks of trimmed lines.
fn blocks(text: &str) -> Vec<Vec<&str>> {
    let mut out = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Parses `start --> end` with either comma or dot milliseconds and an
/// optional hour field.
fn timing(line: &str) -> Result<(Ratio, Ratio), CaptionError> {
    let (a, rest) = line
        .split_once("-->")
        .ok_or_else(|| bad(format!("expected a timing line, found {line:?}")))?;
    let b = rest.split_whitespace().next().unwrap_or("");
    let start = timestamp(a.trim())?;
    let end = timestamp(b.trim())?;
    if end < start {
        return Err(bad(format!("cue ends before it starts: {line:?}")));
    }
    Ok((start, end))
}

/// Parses `hh:mm:ss,mmm`, `hh:mm:ss.mmm` or `mm:ss.mmm`.
fn timestamp(s: &str) -> Result<Ratio, CaptionError> {
    let err = || bad(format!("bad timestamp {s:?}"));
    let (clock, millis) = s.split_once([',', '.']).ok_or_else(err)?;
    let parts: Vec<&str> = clock.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, sec] => (*h, *m, *sec),
        [m, sec] => ("0", *m, *sec),
        _ => return Err(err()),
    };
    let h: i64 = h.parse().map_err(|_| err())?;
    let m: i64 = m.parse().map_err(|_| err())?;
    let sec: i64 = sec.parse().map_err(|_| err())?;
    let ms_digits = millis.trim();
    if ms_digits.is_empty() || ms_digits.len() > 3 || !ms_digits.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(err());
    }
    let mut ms: i64 = ms_digits.parse().map_err(|_| err())?;
    for _ in ms_digits.len()..3 {
        ms *= 10;
    }
    Ok(Ratio::new(((h * 60 + m) * 60 + sec) * 1000 + ms, 1000))
}

/// Formats seconds as `hh:mm:ss` plus milliseconds with `sep` before them.
pub fn stamp(t: Ratio, sep: char) -> String {
    let total_ms = (t.to_f64() * 1000.0).round().max(0.0) as i64;
    let (ms, s) = (total_ms % 1000, total_ms / 1000);
    format!(
        "{:02}:{:02}:{:02}{}{:03}",
        s / 3600,
        (s / 60) % 60,
        s % 60,
        sep,
        ms
    )
}

/// How words are gathered into cues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grouping {
    /// Most lines a cue shows at once. Two is the broadcast convention.
    pub max_lines: usize,
    /// Most characters on one line before it wraps.
    pub max_chars: usize,
    /// Shortest a cue may be. A recogniser will happily emit a word that
    /// lasts a fifth of a second, which nobody can read.
    pub min_duration: Ratio,
    /// A gap no longer than this between two cues is closed rather than
    /// blinking the caption off and straight back on.
    pub merge_gap: Ratio,
}

impl Default for Grouping {
    fn default() -> Self {
        Self {
            max_lines: 2,
            max_chars: 42,
            min_duration: Ratio::new(6, 5),
            merge_gap: Ratio::new(1, 10),
        }
    }
}

/// A silence between two words that always ends a cue.
pub const PAUSE: Ratio = Ratio::ONE;

/// Gathers words into cues.
///
/// A recogniser's own segments are the best phrase boundaries there are,
/// so when the file has them each one starts a new cue; `segment` is the
/// index the word came from. A pause of [`PAUSE`] or more starts one too,
/// so a cue never shows words through a silence before they are said.
/// Inside a segment, a cue is filled until it would take more than
/// `max_lines` lines and then started again, so a long segment becomes
/// several cues rather than one wall of words.
pub fn cues_from_words(words: &[(Word, usize)], rules: Grouping) -> Vec<Cue> {
    let budget = rules.max_lines.max(1) * rules.max_chars.max(8);
    let mut cues: Vec<Cue> = Vec::new();
    let mut current: Vec<Word> = Vec::new();
    let mut segment = usize::MAX;

    let flush = |cues: &mut Vec<Cue>, current: &mut Vec<Word>| {
        if current.is_empty() {
            return;
        }
        let text = current
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let start = current[0].start;
        let end = current[current.len() - 1].end;
        cues.push(Cue {
            start,
            end,
            text,
            words: std::mem::take(current),
            place: Placement::default(),
        });
    };

    for (word, seg) in words {
        let length: usize = current.iter().map(|w| w.text.chars().count() + 1).sum();
        let starts_a_phrase = *seg != segment
            || current
                .last()
                .is_some_and(|last| word.start - last.end >= PAUSE);
        let would_overflow = length + word.text.chars().count() > budget;
        if starts_a_phrase || would_overflow {
            flush(&mut cues, &mut current);
            segment = *seg;
        }
        current.push(word.clone());
    }
    flush(&mut cues, &mut current);

    // A cue nobody can read is held longer, and a gap too short to notice
    // is closed, so the caption does not blink between phrases.
    for i in 0..cues.len() {
        if cues[i].end - cues[i].start < rules.min_duration {
            cues[i].end = cues[i].start + rules.min_duration;
        }
        if let Some(next_start) = cues.get(i + 1).map(|c| c.start) {
            if next_start > cues[i].end && next_start - cues[i].end <= rules.merge_gap {
                cues[i].end = next_start;
            }
            // Holding a short cue must not push it over the next one.
            if cues[i].end > next_start {
                cues[i].end = next_start;
            }
        }
    }
    cues
}

/// Reads the JSON a speech recogniser writes.
///
/// The shape is whatever the tool produced: whisper nests `words` inside
/// `segments`, some tools hand back `{"words": [...]}`, some a bare list.
/// All three are read, and every key that is not the word or its times
/// (`probability`, `score`, `confidence`, `speaker`, `tokens`) is
/// ignored, so a file can usually go in as it came out.
///
/// Forgiving about keys, strict about structure: a file with no words in
/// it anywhere is an error rather than a caption track that draws
/// nothing.
pub fn parse_words(text: &str) -> Result<Vec<(Word, usize)>, CaptionError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| bad(format!("not valid JSON: {e}")))?;

    // Words, and which phrase each came from. Without segments every word
    // is its own phrase boundary candidate, so they share one index and
    // the grouping rules alone decide where cues break.
    let mut out: Vec<(Word, usize)> = Vec::new();
    let mut ms_suspect = false;

    let mut take = |list: &Vec<serde_json::Value>, segment: usize, out: &mut Vec<(Word, usize)>| {
        for entry in list {
            let Some(object) = entry.as_object() else {
                continue;
            };
            let text = object
                .get("word")
                .or_else(|| object.get("text"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            let (Some(start), Some(end)) = (number(object.get("start")), number(object.get("end")))
            else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            if start >= 1000.0 {
                ms_suspect = true;
            }
            out.push((
                Word {
                    text: text.to_owned(),
                    start: seconds(start),
                    end: seconds(end.max(start)),
                },
                segment,
            ));
        }
    };

    match &value {
        serde_json::Value::Array(list) => take(list, 0, &mut out),
        serde_json::Value::Object(root) => {
            if let Some(segments) = root.get("segments").and_then(|s| s.as_array()) {
                for (i, segment) in segments.iter().enumerate() {
                    if let Some(list) = segment.get("words").and_then(|w| w.as_array()) {
                        take(list, i, &mut out);
                    }
                }
            }
            if out.is_empty() {
                if let Some(list) = root.get("words").and_then(|w| w.as_array()) {
                    take(list, 0, &mut out);
                }
            }
        }
        _ => {}
    }

    if out.is_empty() {
        return Err(bad(
            "no words in the file; expected a list of {\"word\", \"start\", \"end\"}, \
or {\"words\": [...]}, or whisper's {\"segments\": [{\"words\": [...]}]}"
                .to_owned(),
        ));
    }
    if ms_suspect {
        return Err(bad(
            "the times look like milliseconds, not seconds: a word starts past 1000. \
Divide them by 1000; geneva reads seconds, as whisper and whisperx write them"
                .to_owned(),
        ));
    }
    out.sort_by(|a, b| a.0.start.cmp(&b.0.start));
    Ok(out)
}

fn number(value: Option<&serde_json::Value>) -> Option<f64> {
    value?.as_f64().filter(|v| v.is_finite() && *v >= 0.0)
}

/// Seconds as an exact rational, to the millisecond.
fn seconds(v: f64) -> Ratio {
    Ratio::new((v * 1000.0).round() as i64, 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(v: f64) -> Ratio {
        seconds(v)
    }

    #[test]
    fn reads_whisper_as_it_comes() {
        // Nested segments, a leading space on every word, and three keys
        // geneva has no use for.
        let text = r#"{
          "text": " Dragon is captured",
          "language": "en",
          "segments": [
            {"id": 0, "seek": 0, "start": 0.0, "end": 1.5, "text": " Dragon is",
             "tokens": [50364, 8532],
             "words": [
               {"word": " Dragon", "start": 0.0, "end": 0.5, "probability": 0.98},
               {"word": " is", "start": 0.5, "end": 0.8, "probability": 0.91}
             ]},
            {"id": 1, "start": 0.8, "end": 1.5, "text": " captured",
             "words": [{"word": " captured", "start": 0.8, "end": 1.5, "probability": 0.87}]}
          ]
        }"#;
        let words = parse_words(text).unwrap();
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].0.text, "Dragon");
        assert_eq!(words[0].0.start, secs(0.0));
        assert_eq!(words[0].0.end, secs(0.5));
        // The segment index rides along so cues break where the phrases do.
        assert_eq!(words[1].1, 0);
        assert_eq!(words[2].1, 1);
    }

    #[test]
    fn reads_the_other_shapes_and_key_names() {
        // whisperx: no leading space, "score" instead of "probability".
        let flat = r#"{"words": [{"word": "Dragon", "start": 0, "end": 0.5, "score": 0.9}]}"#;
        assert_eq!(parse_words(flat).unwrap()[0].0.text, "Dragon");

        // A bare list, with the text under "text".
        let bare = r#"[{"text": "Dragon", "start": 0, "end": 0.5, "speaker": "A"}]"#;
        assert_eq!(parse_words(bare).unwrap()[0].0.text, "Dragon");
    }

    #[test]
    fn refuses_a_file_with_no_words_in_it() {
        for empty in [
            r#"{"text": "no words here", "language": "en"}"#,
            r#"{"segments": [{"start": 0, "end": 1, "text": "hi"}]}"#,
            "[]",
            "{}",
        ] {
            let e = parse_words(empty).unwrap_err().to_string();
            assert!(e.contains("no words in the file"), "{empty}: {e}");
        }
        assert!(
            parse_words("not json")
                .unwrap_err()
                .to_string()
                .contains("not valid JSON")
        );
    }

    #[test]
    fn milliseconds_are_refused_rather_than_guessed() {
        // AssemblyAI and Deepgram write milliseconds; silently treating
        // them as seconds would put the caption three hours in.
        let ms = r#"[{"text": "Dragon", "start": 1200, "end": 1500}]"#;
        let e = parse_words(ms).unwrap_err().to_string();
        assert!(e.contains("milliseconds"), "{e}");
    }

    #[test]
    fn groups_words_into_cues_at_phrase_boundaries() {
        let words = parse_words(
            r#"{"segments": [
                {"words": [{"word": "Dragon", "start": 0, "end": 0.5},
                           {"word": "is", "start": 0.5, "end": 0.8}]},
                {"words": [{"word": "captured", "start": 2.0, "end": 2.6}]}
            ]}"#,
        )
        .unwrap();
        let cues = cues_from_words(&words, Grouping::default());
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Dragon is");
        assert_eq!(cues[1].text, "captured");
        assert_eq!(cues[0].words.len(), 2);
    }

    #[test]
    fn a_long_phrase_becomes_several_cues() {
        let mut words = Vec::new();
        for i in 0..40 {
            words.push((
                Word {
                    text: "word".to_owned(),
                    start: secs(f64::from(i) * 0.3),
                    end: secs(f64::from(i) * 0.3 + 0.25),
                },
                0,
            ));
        }
        let cues = cues_from_words(&words, Grouping::default());
        assert!(cues.len() > 1, "40 words should not be one cue");
        for cue in &cues {
            assert!(cue.text.chars().count() <= 2 * 42 + 8, "{}", cue.text);
        }
    }

    #[test]
    fn short_cues_are_held_and_small_gaps_closed() {
        let words = vec![
            (
                Word {
                    text: "hi".to_owned(),
                    start: secs(0.0),
                    end: secs(0.2),
                },
                0,
            ),
            (
                Word {
                    text: "there".to_owned(),
                    start: secs(5.0),
                    end: secs(5.4),
                },
                1,
            ),
        ];
        let cues = cues_from_words(&words, Grouping::default());
        // The first is held to the minimum rather than flashing for 0.2s.
        assert_eq!(cues[0].end - cues[0].start, Ratio::new(6, 5));

        // A gap of a twentieth of a second is closed instead of blinking.
        let close = vec![
            (
                Word {
                    text: "hi".to_owned(),
                    start: secs(0.0),
                    end: secs(2.0),
                },
                0,
            ),
            (
                Word {
                    text: "there".to_owned(),
                    start: secs(2.05),
                    end: secs(4.0),
                },
                1,
            ),
        ];
        let cues = cues_from_words(&close, Grouping::default());
        assert_eq!(cues[0].end, cues[1].start);
    }

    #[test]
    fn webvtt_cue_settings_are_kept() {
        let vtt = "WEBVTT\n\n00:01.500 --> 00:03.000 line:90% position:40% align:start size:80%\n\
                   Hello\n\n00:04.000 --> 00:05.000\nPlain\n";
        let cues = parse(vtt).unwrap();
        assert_eq!(cues[0].place.line, Some(90));
        assert_eq!(cues[0].place.position, Some(40));
        assert_eq!(cues[0].place.align, Some(Align::Left));
        assert_eq!(cues[0].place.size, Some(80));
        // A cue that says nothing leaves the document's placement alone.
        assert!(cues[1].place.is_empty());
    }

    #[test]
    fn srt_still_reads_the_way_it_did() {
        let srt = "1\n00:00:01,000 --> 00:00:02,500\nHello\nworld\n\n2\n00:00:03,000 --> 00:00:04,000\nBye\n";
        let cues = parse(srt).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Hello\nworld");
        assert_eq!(cues[0].start, Ratio::new(1, 1));
        assert!(cues[0].words.is_empty());
        assert!(parse("1\n00:00:05,000 --> 00:00:04,000\nx\n").is_err());
    }
}
