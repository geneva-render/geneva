//! The manual, carried in the binary.
//!
//! A program or an agent that has `geneva` on its path usually does not
//! have the repository, and the release tarball ships the binary rather
//! than `docs/`. So the pages a caller needs are embedded here with
//! [`include_str!`] and printed by `geneva guide`, which means the
//! manual is the same artifact as the binary and cannot describe
//! another version. `geneva explain` answers the other half: every
//! diagnostic carries a stable code, so a caller that hit one has a
//! command to run rather than a search to make.
//!
//! Nothing is generated at build time. The tables in `docs/errors.md`
//! are the one description of the codes, read here as they are, and a
//! test walks the crates to check that every code the source can emit
//! appears in them.

use anyhow::{Result, bail};

/// A page of the manual.
pub struct Topic {
    /// What to pass to `geneva guide`.
    pub name: &'static str,
    /// One line for the list.
    pub about: &'static str,
    /// The page itself, as Markdown.
    pub text: &'static str,
}

/// The pages, the first being what `geneva guide` prints on its own.
///
/// Every `docs/` page that another page links to has to be here, or the
/// link is dead for anyone who has the binary and not the repository.
/// That is why `architecture.md` is carried: it describes how the engine
/// is built rather than how to drive it, and `timeline.md` and `cli.md`
/// both point at it. A test checks the closure.
pub const TOPICS: &[Topic] = &[
    Topic {
        name: "agents",
        about: "Driving geneva from code: the calling contract, the report shape, a working loop",
        text: include_str!("../../../docs/agents.md"),
    },
    Topic {
        name: "timeline",
        about: "The timeline format: every field of a document, with its defaults",
        text: include_str!("../../../docs/timeline.md"),
    },
    Topic {
        name: "cli",
        about: "Every command and flag, with examples",
        text: include_str!("../../../docs/cli.md"),
    },
    Topic {
        name: "errors",
        about: "What each diagnostic code means, and the exit codes",
        text: include_str!("../../../docs/errors.md"),
    },
    Topic {
        name: "color",
        about: "Colour handling: tags, the working space, what is inferred",
        text: include_str!("../../../docs/color.md"),
    },
    Topic {
        name: "architecture",
        about: "How the engine is put together: the crates, the passes, the renderers",
        text: include_str!("../../../docs/architecture.md"),
    },
    Topic {
        name: "farm",
        about: "Starting farm workers over SSH, in Docker, Kubernetes, ECS and Lambda",
        text: include_str!("../../../docs/farm.md"),
    },
];

/// The page called `name`.
pub fn topic(name: &str) -> Result<&'static Topic> {
    match TOPICS.iter().find(|t| t.name == name) {
        Some(t) => Ok(t),
        None => {
            let names: Vec<&str> = TOPICS.iter().map(|t| t.name).collect();
            bail!("no guide topic {name:?}; there is {}", names.join(", "))
        }
    }
}

/// One row of a table in `docs/errors.md`: what one code means in one
/// situation.
///
/// Each code has one meaning, so a program can act on it; the page
/// could still give one several rows, and [`explain`] answers with every
/// row that carries the code rather than the first.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// The code itself, upper case.
    pub code: String,
    /// `error`, `warning` or `note`, from the letter.
    pub severity: &'static str,
    /// The heading the row sits under, such as `Timing (E300-E399)`.
    pub section: String,
    /// The row's description.
    pub meaning: String,
}

/// Every documented code, in the order `docs/errors.md` lists them.
pub fn entries() -> Vec<Entry> {
    let errors = TOPICS
        .iter()
        .find(|t| t.name == "errors")
        .expect("the errors page is a topic");
    parse(errors.text)
}

/// The rows for one code, whatever its case. Empty when none match.
pub fn explain(code: &str) -> Vec<Entry> {
    let code = code.trim().to_ascii_uppercase();
    entries().into_iter().filter(|e| e.code == code).collect()
}

/// `error`, `warning` or `note` for a code's first letter.
fn severity(code: &str) -> Option<&'static str> {
    match code.as_bytes().first()? {
        b'E' => Some("error"),
        b'W' => Some("warning"),
        b'N' => Some("note"),
        _ => None,
    }
}

/// A code is a letter and three digits, and nothing else: the exit
/// code table in the same page has rows of bare numbers.
fn is_code(cell: &str) -> bool {
    cell.len() == 4
        && severity(cell).is_some()
        && cell.as_bytes()[1..].iter().all(u8::is_ascii_digit)
}

/// The code rows of a Markdown page, each under the `##` heading above
/// it.
fn parse(page: &str) -> Vec<Entry> {
    let mut section = String::new();
    let mut out = Vec::new();
    for line in page.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading.trim().to_string();
            continue;
        }
        let Some(row) = line.strip_prefix("| ") else {
            continue;
        };
        let mut cells = row.split('|').map(str::trim);
        let (Some(code), Some(meaning)) = (cells.next(), cells.next()) else {
            continue;
        };
        if !is_code(code) {
            continue;
        }
        // A few W codes are reported as notes, and the table says so.
        let (severity, meaning) = match meaning.strip_prefix("(note) ") {
            Some(rest) => ("note", rest),
            None => (severity(code).expect("checked by is_code"), meaning),
        };
        out.push(Entry {
            code: code.to_string(),
            severity,
            section: section.clone(),
            meaning: meaning.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_topic_is_the_agent_guide() {
        assert_eq!(TOPICS[0].name, "agents");
        assert!(TOPICS.iter().all(|t| !t.text.is_empty()));
    }

    /// A page that points at another `docs/` page is only useful if that
    /// page is here too. Someone holding the binary has no `docs/`
    /// directory to fall back on, so a link to a page that is not carried
    /// is a dead end with no way out.
    #[test]
    fn every_page_a_carried_page_links_to_is_carried() {
        let mut carried: Vec<String> = TOPICS.iter().map(|t| format!("{}.md", t.name)).collect();
        // Not a manual page: it is the licence roll-up, which the release
        // tarball puts beside the binary. It has no place under `guide`.
        carried.push("THIRD-PARTY-NOTICES.md".to_owned());
        let mut dead = Vec::new();
        for topic in TOPICS {
            for (i, _) in topic.text.match_indices(".md") {
                // Walk back over the file name to whatever precedes it.
                let start = topic.text[..i]
                    .rfind(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                    .map_or(0, |b| b + 1);
                let name = format!("{}.md", &topic.text[start..i]);
                if name != ".md" && !carried.contains(&name) {
                    dead.push(format!("{} links to {name}", topic.name));
                }
            }
        }
        dead.sort();
        dead.dedup();
        assert!(
            dead.is_empty(),
            "carry these pages or drop the links: {dead:?}"
        );
    }

    #[test]
    fn an_unknown_topic_lists_the_ones_there_are() {
        // `err()` rather than `unwrap_err()`: a `Topic` holds a whole
        // page, and nothing should be able to print one as a value.
        let message = topic("nonsense").err().unwrap().to_string();
        assert!(
            message.contains("agents") && message.contains("timeline"),
            "{message}"
        );
    }

    #[test]
    fn a_row_becomes_an_entry_under_its_heading() {
        let page = "## Timing (E300-E399)\n\n| Code | Meaning |\n| --- | --- |\n| E302 | Clips overlap. |\n";
        assert_eq!(
            parse(page),
            vec![Entry {
                code: "E302".into(),
                severity: "error",
                section: "Timing (E300-E399)".into(),
                meaning: "Clips overlap.".into(),
            }]
        );
    }

    #[test]
    fn the_exit_code_table_is_not_read_as_diagnostics() {
        let page = "## Exit codes\n\n| Code | Meaning |\n| --- | --- |\n| 0 | Success. |\n";
        assert!(parse(page).is_empty());
        assert!(!is_code("0") && !is_code("E30") && !is_code("X302") && is_code("N600"));
    }

    #[test]
    fn every_code_has_one_meaning() {
        let all = entries();
        let mut seen = std::collections::BTreeSet::new();
        for e in &all {
            assert!(seen.insert(&e.code), "{} has two rows", e.code);
        }
    }

    #[test]
    fn a_warning_the_page_marks_as_a_note_is_explained_as_one() {
        let rows = explain("w201");
        assert_eq!(rows[0].severity, "note");
        assert!(!rows[0].meaning.starts_with("(note)"));
    }

    #[test]
    fn every_severity_letter_is_read_from_the_page() {
        let all = entries();
        assert!(all.iter().any(|e| e.severity == "error"));
        assert!(all.iter().any(|e| e.severity == "warning"));
        assert!(all.iter().any(|e| e.severity == "note"));
        assert!(all.len() > 50, "{} rows", all.len());
    }
}
