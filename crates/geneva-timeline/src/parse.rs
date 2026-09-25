use serde_path_to_error::Segment;

use crate::diagnostic::Diagnostic;
use crate::schema::Timeline;

/// Parses timeline JSON into the document model.
///
/// Structural problems (syntax, unknown or missing fields, wrong types) are
/// reported as a single diagnostic with the JSON pointer of the failing
/// element and the line and column in the text.
pub fn parse(text: &str) -> Result<Timeline, Diagnostic> {
    let mut de = serde_json::Deserializer::from_str(text);
    match serde_path_to_error::deserialize::<_, Timeline>(&mut de) {
        Ok(tl) => Ok(tl),
        Err(err) => Err(structural_diagnostic(&err)),
    }
}

fn structural_diagnostic(err: &serde_path_to_error::Error<serde_json::Error>) -> Diagnostic {
    let path = pointer(err.path());
    let inner = err.inner();
    let (line, column) = (inner.line(), inner.column());
    let message = strip_location(&inner.to_string());

    let (code, help) = if inner.is_syntax() || inner.is_eof() {
        ("E100", Some("the file is not valid JSON; check for a missing comma, quote or bracket near the reported line".to_owned()))
    } else if message.starts_with("unknown field") {
        (
            "E101",
            Some(
                "remove the field or fix its spelling; the message lists the fields allowed here"
                    .to_owned(),
            ),
        )
    } else if message.starts_with("missing field") {
        ("E102", Some("add the missing field".to_owned()))
    } else if message.starts_with("duplicate field") {
        (
            "E104",
            Some("keep only one occurrence of the field".to_owned()),
        )
    } else if message.starts_with("unknown variant") {
        ("E105", Some("use one of the listed values".to_owned()))
    } else {
        ("E103", None)
    };

    let mut d = Diagnostic::error(code, path, message);
    if line > 0 {
        d = d.with_location(line, column);
    }
    if let Some(h) = help {
        d = d.with_help(h);
    }
    d
}

/// Converts a serde path into an RFC 6901 pointer.
fn pointer(path: &serde_path_to_error::Path) -> String {
    let mut out = String::new();
    for seg in path.iter() {
        match seg {
            Segment::Map { key } => {
                out.push('/');
                out.push_str(&key.replace('~', "~0").replace('/', "~1"));
            }
            Segment::Seq { index } => {
                out.push('/');
                out.push_str(&index.to_string());
            }
            Segment::Enum { .. } | Segment::Unknown => {}
        }
    }
    out
}

/// serde_json appends " at line N column M" to messages; the location is
/// reported separately, so it is removed from the message text.
fn strip_location(msg: &str) -> String {
    match msg.rfind(" at line ") {
        Some(i) => msg[..i].to_owned(),
        None => msg.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{"geneva":"1.0","output":{"width":16,"height":16,"fps":30}}"#;

    #[test]
    fn parses_minimal_document() {
        let tl = parse(MINIMAL).unwrap();
        assert_eq!(tl.output.width, 16);
        assert!(tl.layers.is_empty());
    }

    #[test]
    fn syntax_errors_are_e100_with_location() {
        let d = parse("{\"geneva\": \"1.0\",\n  \"output\": {").unwrap_err();
        assert_eq!(d.code, "E100");
        assert!(d.location.is_some());
    }

    #[test]
    fn unknown_fields_point_at_their_location() {
        let text = r##"{"geneva":"1.0","output":{"width":16,"height":16,"fps":30},
          "layers":[{"clips":[{"source":{"kind":"solid","color":"#fff"},"opacty":0.5}]}]}"##;
        let d = parse(text).unwrap_err();
        assert_eq!(d.code, "E101");
        assert_eq!(d.path, "/layers/0/clips/0/opacty");
        assert!(d.message.contains("opacty"));
        assert!(d.message.contains("opacity"));
    }

    #[test]
    fn wrong_types_deep_in_keyframes_keep_their_path() {
        let text = r##"{"geneva":"1.0","output":{"width":16,"height":16,"fps":30},
          "layers":[{"clips":[{"source":{"kind":"solid","color":"#fff"},
            "opacity":{"keyframes":[{"t":0,"v":0},{"t":"1s","v":"one"}]}}]}]}"##;
        let d = parse(text).unwrap_err();
        assert_eq!(d.code, "E103");
        assert_eq!(d.path, "/layers/0/clips/0/opacity/keyframes/1/v");
    }

    #[test]
    fn missing_source_kind_is_reported_as_missing_field() {
        let text = r##"{"geneva":"1.0","output":{"width":16,"height":16,"fps":30},
          "layers":[{"clips":[{"source":{"color":"#fff"}}]}]}"##;
        let d = parse(text).unwrap_err();
        assert_eq!(d.code, "E102");
        assert_eq!(d.path, "/layers/0/clips/0/source");
        assert!(d.message.contains("kind"));
    }

    #[test]
    fn bad_time_strings_are_explained() {
        let text =
            r#"{"geneva":"1.0","output":{"width":16,"height":16,"fps":30,"duration":"3 sec"}}"#;
        let d = parse(text).unwrap_err();
        assert_eq!(d.code, "E103");
        assert_eq!(d.path, "/output/duration");
        assert!(d.message.contains("3 sec"));
    }
}
