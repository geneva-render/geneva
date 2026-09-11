//! Subtitle files: SubRip (`.srt`) and WebVTT (`.vtt`) parsing and
//! writing, and the cue payloads the containers store.

use std::fmt::Write as _;

use geneva_timeline::Ratio;

use crate::MediaError;

/// One timed piece of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    /// When the text appears, in seconds.
    pub start: Ratio,
    /// When it disappears, in seconds.
    pub end: Ratio,
    /// The text, lines separated by `\n`; may carry simple `<i>`-style
    /// tags as written in the file.
    pub text: String,
}

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

fn bad(reason: impl Into<String>) -> MediaError {
    MediaError::Codec {
        context: "subtitle file".to_owned(),
        reason: reason.into(),
    }
}

/// Parses SubRip or WebVTT text, telling them apart by the `WEBVTT`
/// header. Cues come back in file order.
pub fn parse(text: &str) -> Result<Vec<Cue>, MediaError> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with("WEBVTT") {
        parse_webvtt(text)
    } else {
        parse_srt(text)
    }
}

/// Parses SubRip text.
pub fn parse_srt(text: &str) -> Result<Vec<Cue>, MediaError> {
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
        cues.push(Cue {
            start,
            end,
            text: body.join("\n"),
        });
    }
    Ok(cues)
}

/// Parses WebVTT text. Header, `NOTE`, `STYLE` and `REGION` blocks are
/// skipped; cue identifiers and settings are dropped.
pub fn parse_webvtt(text: &str) -> Result<Vec<Cue>, MediaError> {
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
        cues.push(Cue {
            start,
            end,
            text: block[pos + 1..].join("\n"),
        });
    }
    Ok(cues)
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
fn timing(line: &str) -> Result<(Ratio, Ratio), MediaError> {
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
fn timestamp(s: &str) -> Result<Ratio, MediaError> {
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
fn stamp(t: Ratio, sep: char) -> String {
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
        assert_eq!(a, b);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].start, Ratio::new(3, 2));
        assert_eq!(a[0].text, "Hello\nworld");
        assert_eq!(a[1].end, Ratio::new(62_250, 1000));
        assert_eq!(strip_tags(&a[1].text), "Bye");
    }

    #[test]
    fn writers_round_trip() {
        let cues = vec![Cue {
            start: Ratio::new(3, 2),
            end: Ratio::from_int(3),
            text: "Hi".to_owned(),
        }];
        assert_eq!(parse(&write_srt(&cues)).unwrap(), cues);
        assert_eq!(parse(&write_webvtt(&cues)).unwrap(), cues);
        assert!(write_webvtt(&cues).starts_with("WEBVTT"));
    }

    #[test]
    fn shifting_drops_and_clamps() {
        let mut cues = vec![
            Cue {
                start: Ratio::ZERO,
                end: Ratio::from_int(1),
                text: "a".into(),
            },
            Cue {
                start: Ratio::from_int(2),
                end: Ratio::from_int(4),
                text: "b".into(),
            },
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
