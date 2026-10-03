//! Diagnostic notes shared by every library (decisions 37 and 39).
//!
//! Libraries below layout (fonts, shaping, composition) don't know which
//! document node they are working on, so they report a [`Note`]: a severity,
//! a stable [`Code`], a message and optionally the source bytes it concerns.
//! Layout attaches the subject (a node, relation, range, ...) and publishes it
//! as a `reprise_layout::Diagnostic`.
//!
//! Tests and tools match on [`Code`], never on message text.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use serde::{Deserialize, Serialize};

/// How bad it is. Ordered from least to most severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Worth knowing; the output is what the author asked for.
    Info,
    /// The output differs from what the author asked for, e.g. a pushed-down
    /// note or a substituted font.
    Warning,
    /// Something was left out of the output.
    Error,
}

/// A stable, dotted identifier such as `compose.overflow`. The part before the
/// first dot names the library that reports it. Codes are part of the public
/// contract: renaming one is a breaking change.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Code(Cow<'static, str>);

impl Code {
    pub const fn new(code: &'static str) -> Code {
        Code(Cow::Borrowed(code))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<&str> for Code {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// A diagnostic without a subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub severity: Severity,
    pub code: Code,
    pub message: String,
    /// Source bytes the note concerns, in the text the reporting library was given.
    pub bytes: Option<Range<usize>>,
}

impl Note {
    pub fn new(severity: Severity, code: Code, message: impl Into<String>) -> Note {
        Note {
            severity,
            code,
            message: message.into(),
            bytes: None,
        }
    }

    pub fn info(code: Code, message: impl Into<String>) -> Note {
        Note::new(Severity::Info, code, message)
    }

    pub fn warning(code: Code, message: impl Into<String>) -> Note {
        Note::new(Severity::Warning, code, message)
    }

    pub fn error(code: Code, message: impl Into<String>) -> Note {
        Note::new(Severity::Error, code, message)
    }

    pub fn at(mut self, bytes: Range<usize>) -> Note {
        self.bytes = Some(bytes);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_as_plain_strings() {
        let note = Note::warning(Code::new("compose.overflow"), "too long").at(0..3);
        let json = serde_json::to_string(&note).unwrap();
        assert_eq!(
            json,
            r#"{"severity":"warning","code":"compose.overflow","message":"too long","bytes":{"start":0,"end":3}}"#
        );
        let back: Note = serde_json::from_str(&json).unwrap();
        assert_eq!(back, note);
        assert_eq!(back.code, "compose.overflow");
    }

    #[test]
    fn severities_order_by_badness() {
        assert!(Severity::Info < Severity::Warning && Severity::Warning < Severity::Error);
    }
}
