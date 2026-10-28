//! Definition-owned diagnostic specifications registered in a session.

use klf_core::FileId;
use klf_diag::{DetailKind, RuntimeDiagnostic, Severity};
use klf_source::Span;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticSpec {
    pub code: i64,
    pub severity: Severity,
    pub title: String,
    pub explanation: String,
    /// The concrete next step, shown as a `= help:` line; empty when none.
    pub help: String,
}

/// The word a rendered diagnostic leads with, per section 31.4.
#[must_use]
pub const fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
        Severity::Remark => "remark",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRuntimeDiagnostic {
    pub code: i64,
    pub severity: Severity,
    pub title: String,
    pub explanation: String,
    pub help: String,
    pub span: Span,
    /// The name the headline quotes, when the report names one apart from its
    /// range.
    pub subject: Option<Span>,
    /// The notes the definition attached, after the registered explanation.
    pub notes: Vec<String>,
    /// The helps the definition attached; they replace the registered help,
    /// which can only be general.
    pub helps: Vec<String>,
    /// Further places the report shows with their own snippets, each with
    /// its label: where a name it is about is declared.
    pub places: Vec<(Span, String)>,
}

impl DiagnosticSpec {
    #[must_use]
    pub fn new(
        code: i64,
        severity: Severity,
        title: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity,
            title: title.into(),
            explanation: explanation.into(),
            help: String::new(),
        }
    }

    /// The same specification with its `= help:` text.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }

    #[must_use]
    pub fn resolve(
        &self,
        record: RuntimeDiagnostic,
        file: FileId,
    ) -> Option<ResolvedRuntimeDiagnostic> {
        let start = u32::try_from(record.start).ok()?;
        let end = u32::try_from(record.end).ok()?;
        if end < start {
            return None;
        }
        let mut explanation = self.explanation.clone();
        let mut notes = Vec::new();
        let mut helps = Vec::new();
        let mut places = Vec::new();
        for detail in record.details {
            match detail.kind {
                DetailKind::Note => notes.push(detail.text),
                DetailKind::Help => helps.push(detail.text),
                DetailKind::Label => explanation = detail.text,
                DetailKind::Place => {
                    let place = u32::try_from(detail.file)
                        .ok()
                        .zip(u32::try_from(detail.start).ok())
                        .zip(u32::try_from(detail.end).ok())
                        .filter(|((_, start), end)| end >= start)
                        .map(|((id, start), end)| Span::new(FileId::from_u32(id), start, end));
                    match place {
                        Some(place) => places.push((place, detail.text)),
                        None => notes.push(detail.text),
                    }
                }
            }
        }
        Some(ResolvedRuntimeDiagnostic {
            code: self.code,
            severity: self.severity,
            title: self.title.clone(),
            explanation,
            help: self.help.clone(),
            span: Span::new(file, start, end),
            subject: record.subject.and_then(|(subject_start, subject_end)| {
                let subject_start = u32::try_from(subject_start).ok()?;
                let subject_end = u32::try_from(subject_end).ok()?;
                (subject_end >= subject_start).then(|| Span::new(file, subject_start, subject_end))
            }),
            notes,
            helps,
            places,
        })
    }
}
