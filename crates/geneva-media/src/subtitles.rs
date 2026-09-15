//! Subtitle files as containers store them: writing SubRip and WebVTT,
//! language codes, and the cue payloads a muxer wants.
//!
//! Reading them is [`geneva_timeline::captions`], which is where the
//! resolver needs it; this module re-exports what it produces so that
//! nothing here has to know the difference.

use std::fmt::Write as _;

use geneva_timeline::Ratio;
use geneva_timeline::captions;

use crate::MediaError;

pub use geneva_timeline::captions::{Cue, stamp};

/// A subtitle file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    /// SubRip.
    Srt,
    /// WebVTT.
    WebVtt,
}

impl SubtitleFormat {
    /// Picks the format from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "srt" => Some(Self::Srt),
            "vtt" => Some(Self::WebVtt),
            _ => None,
        }
    }
}

fn wrap(e: &captions::CaptionError) -> MediaError {
    MediaError::Codec {
        context: "subtitle file".to_owned(),
        reason: e.to_string(),
    }
}

/// Parses SubRip or WebVTT text, telling them apart by the `WEBVTT`
/// header. Cues come back in file order.
pub fn parse(text: &str) -> Result<Vec<Cue>, MediaError> {
    captions::parse(text).map_err(|e| wrap(&e))
}

/// Parses SubRip text.
pub fn parse_srt(text: &str) -> Result<Vec<Cue>, MediaError> {
    captions::parse_srt(text).map_err(|e| wrap(&e))
}

/// Parses WebVTT text.
pub fn parse_webvtt(text: &str) -> Result<Vec<Cue>, MediaError> {
    captions::parse_webvtt(text).map_err(|e| wrap(&e))
}

/// Writes cues as SubRip text.
pub fn write_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, cue) in cues.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}\n{} --> {}\n{}\n",
            i + 1,
            stamp(cue.start, ','),
            stamp(cue.end, ','),
            cue.text
        );
    }
    out
}

/// Writes cues as WebVTT text.
pub fn write_webvtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        let _ = writeln!(
            out,
            "{} --> {}\n{}\n",
            stamp(cue.start, '.'),
            stamp(cue.end, '.'),
            cue.text
        );
    }
    out
}

/// ISO 639-1 codes and their ISO 639-2/B equivalents.
const ISO639_TABLE: &[(&str, &str)] = &[
    ("ar", "ara"),
    ("bg", "bul"),
    ("ca", "cat"),
    ("cs", "cze"),
    ("da", "dan"),
    ("de", "ger"),
    ("el", "gre"),
    ("en", "eng"),
    ("es", "spa"),
    ("et", "est"),
    ("fa", "per"),
    ("fi", "fin"),
    ("fr", "fre"),
    ("he", "heb"),
    ("hi", "hin"),
    ("hr", "hrv"),
    ("hu", "hun"),
    ("id", "ind"),
    ("it", "ita"),
    ("ja", "jpn"),
    ("ko", "kor"),
    ("lt", "lit"),
    ("lv", "lav"),
    ("ms", "may"),
    ("nl", "dut"),
    ("no", "nor"),
    ("pl", "pol"),
    ("pt", "por"),
    ("ro", "rum"),
    ("ru", "rus"),
    ("sk", "slo"),
    ("sl", "slv"),
    ("sr", "srp"),
    ("sv", "swe"),
    ("th", "tha"),
    ("tr", "tur"),
    ("uk", "ukr"),
    ("vi", "vie"),
    ("zh", "chi"),
];

/// The three-letter ISO 639-2 code containers store, from a two-letter
/// ISO 639-1 code or a BCP 47 tag such as `pt-BR`; codes that are already
/// three letters or unknown pass through unchanged.
pub fn iso639_2(code: &str) -> String {
    let base = code
        .split(['-', '_'])
        .next()
        .unwrap_or(code)
        .to_ascii_lowercase();
    if base.len() != 2 {
        return base;
    }
    ISO639_TABLE
        .iter()
        .find(|(two, _)| *two == base)
        .map_or(base, |(_, three)| (*three).to_owned())
}

/// Removes `<...>` markup, for containers whose text format has no tags.
pub fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Shifts every cue by `offset`, dropping what ends before zero and
/// clamping what starts before it.
pub fn shift(cues: &mut Vec<Cue>, offset: Ratio) {
    if offset == Ratio::ZERO {
        return;
    }
    cues.retain_mut(|c| {
        c.start = c.start + offset;
        c.end = c.end + offset;
        if c.end <= Ratio::ZERO {
            return false;
        }
        c.start = c.start.max(Ratio::ZERO);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_and_webvtt_parse_to_the_same_cues() {
        let srt = "1\n00:00:01,500 --> 00:00:03,000\nHello\nworld\n\n2\n00:01:00,000 --> 00:01:02,250\n<i>Bye</i>\n";
        let vtt = "WEBVTT - test\n\nNOTE a note\n\nintro\n00:01.500 --> 00:03.000 line:90%\nHello\nworld\n\n00:01:00.000 --> 00:01:02.250\n<i>Bye</i>\n";
        let a = parse(srt).unwrap();
        let b = parse(vtt).unwrap();
        // The same times and text either way. SubRip cannot say where a
        // cue sits and WebVTT can, so only the placement differs.
        assert!(
            a.iter()
                .zip(&b)
                .all(|(x, y)| x.start == y.start && x.end == y.end && x.text == y.text)
        );
        assert_eq!(b[0].place.line, Some(90));
        assert!(a[0].place.is_empty());
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].start, Ratio::new(3, 2));
        assert_eq!(a[0].text, "Hello\nworld");
        assert_eq!(a[1].end, Ratio::new(62_250, 1000));
        assert_eq!(strip_tags(&a[1].text), "Bye");
    }

    #[test]
    fn writers_round_trip() {
        let cues = vec![Cue::plain(Ratio::new(3, 2), Ratio::from_int(3), "Hi")];
        assert_eq!(parse(&write_srt(&cues)).unwrap(), cues);
        assert_eq!(parse(&write_webvtt(&cues)).unwrap(), cues);
        assert!(write_webvtt(&cues).starts_with("WEBVTT"));
    }

    #[test]
    fn shifting_drops_and_clamps() {
        let mut cues = vec![
            Cue::plain(Ratio::ZERO, Ratio::from_int(1), "a"),
            Cue::plain(Ratio::from_int(2), Ratio::from_int(4), "b"),
        ];
        shift(&mut cues, Ratio::from_int(-3));
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, Ratio::ZERO);
        assert_eq!(cues[0].end, Ratio::from_int(1));
    }

    #[test]
    fn language_codes_become_three_letters() {
        assert_eq!(iso639_2("en"), "eng");
        assert_eq!(iso639_2("pt-BR"), "por");
        assert_eq!(iso639_2("eng"), "eng");
        assert_eq!(iso639_2("xx"), "xx");
    }

    #[test]
    fn bad_timing_is_an_error() {
        assert!(parse("1\n00:00:01 --> 00:00:03\nx\n").is_err());
        assert!(parse("1\n00:00:05,000 --> 00:00:03,000\nx\n").is_err());
    }
}
