//! Structured diagnostics shared by the KLF kernel.

pub mod names;
pub mod progress;
pub mod report;
pub mod status;
pub mod timeline;

use klf_source::Span;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Remark,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Note {
    pub span: Option<Span>,
    pub message: String,
}

/// One edit a quick fix makes: a byte range to replace and the text to put
/// there (`KLF-EDITOR.md` section 11.10 item 1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fix {
    /// What the action is called; the kernel titles it when a definition
    /// gave none.
    pub title: String,
    /// The ranges to replace, and what replaces each.
    pub edits: Vec<(Span, String)>,
}

/// What a [`RuntimeDetail`] adds to the diagnostic it follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailKind {
    /// A `= note:` line after the registered one.
    Note,
    /// A `= help:` line; the first one replaces the registered help, which
    /// can only be general.
    Help,
    /// The label under the primary range, in place of the registered
    /// explanation.
    Label,
    /// A further place, shown with its own snippet and this text as its
    /// label: where a name the report is about is declared.
    Place,
}

impl DetailKind {
    /// The kind a definition passes as a number to `klf.diag.detail`.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Note),
            1 => Some(Self::Help),
            2 => Some(Self::Label),
            3 => Some(Self::Place),
            _ => None,
        }
    }

    /// The number `from_code` reads.
    #[must_use]
    pub const fn code(self) -> i64 {
        match self {
            Self::Note => 0,
            Self::Help => 1,
            Self::Label => 2,
            Self::Place => 3,
        }
    }
}

/// Text a definition attached to the diagnostic it reported last: the values
/// and declarations involved and why, which a registered code alone cannot
/// say.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDetail {
    pub kind: DetailKind,
    /// For a [`DetailKind::Place`], the file id and byte range it points at.
    pub file: i64,
    pub start: i64,
    pub end: i64,
    pub text: String,
}

impl RuntimeDetail {
    /// The line a phase that runs as its own process prints after the
    /// `DIAG` line of the diagnostic this detail belongs to: the numbers,
    /// then the text in hexadecimal so it may hold any byte.
    #[must_use]
    pub fn protocol_line(&self) -> String {
        let mut line = format!("DIAGD {} {} {} {} ", self.kind.code(), self.file, self.start, self.end);
        for byte in self.text.bytes() {
            line.push_str(&format!("{byte:02x}"));
        }
        line
    }

    /// The detail a [`RuntimeDetail::protocol_line`] wrote, without its
    /// `DIAGD ` prefix.
    #[must_use]
    pub fn from_protocol(rest: &str) -> Option<Self> {
        let mut values = rest.trim_end().splitn(5, ' ');
        let kind = DetailKind::from_code(values.next()?.parse().ok()?)?;
        let file = values.next()?.parse().ok()?;
        let start = values.next()?.parse().ok()?;
        let end = values.next()?.parse().ok()?;
        let hex = values.next().unwrap_or("");
        if hex.len() % 2 != 0 {
            return None;
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()?;
        Some(Self {
            kind,
            file,
            start,
            end,
            text: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
}

/// A diagnostic a running definition reported: a registered code and the byte
/// range it points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDiagnostic {
    pub code: i64,
    pub start: i64,
    pub end: i64,
    /// The file the range is in, as a file id, or negative for the file the
    /// phase is running on. A whole-program check runs on one unit but points
    /// at another, so the record carries the file when it knows it.
    pub file: i64,
    /// The content hash of a registered source the range is in, or zero. A
    /// definition that splices an included header into a unit knows the
    /// header only by its text, so the session finds the file by this hash.
    pub origin: i64,
    /// The byte range of the name the report is about, in the same file as
    /// the range above, or none. The headline quotes this name.
    pub subject: Option<(i64, i64)>,
    /// What the definition attached after reporting it (`klf.diag.detail`).
    pub details: Vec<RuntimeDetail>,
}

impl RuntimeDiagnostic {
    /// A record against the file the phase is running on.
    #[must_use]
    pub const fn new(code: i64, start: i64, end: i64) -> Self {
        Self {
            code,
            start,
            end,
            file: -1,
            origin: 0,
            subject: None,
            details: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub primary_span: Option<Span>,
    pub labels: Vec<Label>,
    pub notes: Vec<Note>,
    /// The quick fixes it offers (section 11.10).
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            primary_span: None,
            labels: Vec::new(),
            notes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    #[must_use]
    pub fn error_at(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self {
            primary_span: Some(span),
            ..Self::error(code, message)
        }
    }

    #[must_use]
    pub fn with_primary_span(mut self, span: Span) -> Self {
        self.primary_span = Some(span);
        self
    }

    #[must_use]
    pub const fn primary_span(&self) -> Option<Span> {
        self.primary_span
    }

    #[must_use]
    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        if self.primary_span.is_none() {
            self.primary_span = Some(span);
        }
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(Note {
            span: None,
            message: note.into(),
        });
        self
    }

    #[must_use]
    pub fn with_note_at(mut self, span: Span, note: impl Into<String>) -> Self {
        self.notes.push(Note {
            span: Some(span),
            message: note.into(),
        });
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
            Severity::Remark => "remark",
        };
        write!(formatter, "{severity}[{}]: {}", self.code, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klf_core::FileId;

    #[test]
    fn primary_and_note_spans_are_typed() {
        let primary = Span::new(FileId::from_u32(3), 4, 7);
        let note = Span::new(FileId::from_u32(3), 12, 15);
        let diagnostic = Diagnostic::error_at("KLF-TEST-0001", primary, "bad input")
            .with_note_at(note, "related input");

        assert_eq!(diagnostic.primary_span(), Some(primary));
        assert_eq!(diagnostic.notes[0].span, Some(note));
        assert_eq!(diagnostic.notes[0].message, "related input");
    }

    #[test]
    fn first_label_becomes_the_primary_span() {
        let span = Span::new(FileId::from_u32(0), 1, 2);
        let diagnostic = Diagnostic::error("KLF-TEST-0002", "bad input").with_label(span, "here");

        assert_eq!(diagnostic.primary_span(), Some(span));
    }
}
