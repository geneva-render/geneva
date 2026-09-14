use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational; the timeline is fine.
    Note,
    /// The timeline will render, but probably not as intended.
    Warning,
    /// The timeline cannot be rendered.
    Error,
}

/// A problem found while parsing, validating or rendering a timeline.
///
/// Every diagnostic carries a stable code, a JSON pointer to the location in
/// the document, a one-sentence message, and where possible the offending
/// value and a suggested fix.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// Stable code such as `E302`; see the diagnostics reference.
    pub code: &'static str,
    /// JSON pointer (RFC 6901) to the location, for example
    /// `/layers/0/clips/1/start`. Empty for document-level problems.
    pub path: String,
    /// What is wrong.
    pub message: String,
    /// The offending value, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// How to fix it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Line and column in the source text, when known (1-based).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<(usize, usize)>,
}

impl Diagnostic {
    /// Creates an error.
    pub fn error(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, path, message)
    }

    /// Creates a warning.
    pub fn warning(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(Severity::Warning, code, path, message)
    }

    /// Creates a note.
    pub fn note(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(Severity::Note, code, path, message)
    }

    fn new(
        severity: Severity,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code,
            path: path.into(),
            message: message.into(),
            value: None,
            help: None,
            location: None,
        }
    }

    /// Attaches the offending value.
    pub fn with_value(mut self, value: impl Into<Value>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Attaches a suggested fix.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Attaches a source location.
    pub fn with_location(mut self, line: usize, column: usize) -> Self {
        self.location = Some((line, column));
        self
    }

    /// Returns true for errors.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diagnostic {
    /// Renders in a compiler-like layout:
    ///
    /// ```text
    /// error[E302]: clips overlap in layer "main"
    ///   --> /layers/0/clips/1/start = "1.5s"
    ///    = help: start this clip at 2s or later
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.severity {
            Severity::Note => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        writeln!(f, "{label}[{}]: {}", self.code, self.message)?;
        let location = self
            .location
            .map(|(line, col)| format!(" (line {line}, column {col})"))
            .unwrap_or_default();
        // A diagnostic about the document as a whole, with no value and
        // no position, has nothing to point at; the arrow line would say
        // only "(document)".
        let anchored =
            !self.path.is_empty() || self.value.is_some() || !location.is_empty();
        if anchored {
            let path = if self.path.is_empty() {
                "(document)".to_owned()
            } else {
                self.path.clone()
            };
            match &self.value {
                Some(v) => writeln!(f, "  --> {path} = {v}{location}")?,
                None => writeln!(f, "  --> {path}{location}")?,
            }
        }
        if let Some(help) = &self.help {
            writeln!(f, "   = help: {help}")?;
        }
        Ok(())
    }
}

/// Builds JSON pointers while walking a document.
#[derive(Debug, Clone, Default)]
pub struct Path {
    segments: Vec<String>,
}

impl Path {
    /// The document root.
    pub fn root() -> Self {
        Self::default()
    }

    /// Appends a property name.
    pub fn key(&self, name: &str) -> Self {
        let mut p = self.clone();
        p.segments.push(name.replace('~', "~0").replace('/', "~1"));
        p
    }

    /// Appends an array index.
    pub fn index(&self, i: usize) -> Self {
        let mut p = self.clone();
        p.segments.push(i.to_string());
        p
    }

    /// The pointer as a string, for example `/layers/0/clips`.
    pub fn pointer(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for s in &self.segments {
            write!(f, "/{s}")?;
        }
        Ok(())
    }
}

impl From<Path> for String {
    fn from(p: Path) -> Self {
        p.pointer()
    }
}

impl From<&Path> for String {
    fn from(p: &Path) -> Self {
        p.pointer()
    }
}

/// Summarizes a list of diagnostics.
pub fn summarize(diagnostics: &[Diagnostic]) -> Summary {
    let mut s = Summary::default();
    for d in diagnostics {
        match d.severity {
            Severity::Error => s.errors += 1,
            Severity::Warning => s.warnings += 1,
            Severity::Note => s.notes += 1,
        }
    }
    s
}

/// Counts per severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Number of errors.
    pub errors: usize,
    /// Number of warnings.
    pub warnings: usize,
    /// Number of notes.
    pub notes: usize,
}

impl Summary {
    /// True when there are no errors.
    pub fn is_ok(&self) -> bool {
        self.errors == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers_escape_special_characters() {
        let p = Path::root().key("assets").key("a/b~c").index(3);
        assert_eq!(p.pointer(), "/assets/a~1b~0c/3");
        assert_eq!(Path::root().pointer(), "");
    }

    #[test]
    fn display_layout() {
        let d = Diagnostic::error("E302", "/layers/0/clips/1/start", "clips overlap")
            .with_value("1.5s")
            .with_help("start later");
        assert_eq!(
            d.to_string(),
            "error[E302]: clips overlap\n  --> /layers/0/clips/1/start = \"1.5s\"\n   = help: start later\n"
        );
    }
}
