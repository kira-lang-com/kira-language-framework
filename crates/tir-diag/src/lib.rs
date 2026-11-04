#![forbid(unsafe_code)]

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Dialect {
    Tir,
    Tvisa,
}

impl Dialect {
    const fn name(self) -> &'static str {
        match self {
            Self::Tir => "TIR",
            Self::Tvisa => "TVISA",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Area {
    Encoding,
    Version,
    Verifier,
    Ownership,
    ControlFlow,
    Gpu,
    Architecture,
    Capability,
    Generics,
    Import,
    Package,
    Profile,
    Pipeline,
    Runtime,
    Jit,
    Finalizer,
    Platform,
    Audit,
}

impl Area {
    const fn name(self) -> &'static str {
        match self {
            Self::Encoding => "ENC",
            Self::Version => "VER",
            Self::Verifier => "VFY",
            Self::Ownership => "OWN",
            Self::ControlFlow => "CF",
            Self::Gpu => "GPU",
            Self::Architecture => "ARCH",
            Self::Capability => "CAP",
            Self::Generics => "GEN",
            Self::Import => "IMP",
            Self::Package => "PKG",
            Self::Profile => "PROF",
            Self::Pipeline => "PIPE",
            Self::Runtime => "RT",
            Self::Jit => "JIT",
            Self::Finalizer => "FIN",
            Self::Platform => "PLAT",
            Self::Audit => "AUD",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Code {
    dialect: Dialect,
    area: Area,
    number: u16,
}

impl Code {
    #[must_use]
    pub const fn new(dialect: Dialect, area: Area, number: u16) -> Self {
        Self {
            dialect,
            area,
            number,
        }
    }

    #[must_use]
    pub const fn dialect(self) -> Dialect {
        self.dialect
    }

    #[must_use]
    pub const fn area(self) -> Area {
        self.area
    }

    #[must_use]
    pub const fn number(self) -> u16 {
        self.number
    }
}

impl fmt::Display for Code {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}-{}-{:04}",
            self.dialect.name(),
            self.area.name(),
            self.number
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Note,
    Remark,
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
            Self::Remark => "remark",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEdit {
    pub span: Span,
    pub replacement: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Fix {
    pub title: String,
    pub edits: Vec<TextEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Code,
    pub message: String,
    pub primary_span: Option<Span>,
    pub labels: Vec<Label>,
    pub notes: Vec<Note>,
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: Code, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }

    #[must_use]
    pub fn new(severity: Severity, code: Code, message: impl Into<String>) -> Self {
        Self {
            severity,
            code,
            message: message.into(),
            primary_span: None,
            labels: Vec::new(),
            notes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_primary_span(mut self, span: Span) -> Self {
        self.primary_span = Some(span);
        self
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
    pub fn with_note(mut self, message: impl Into<String>) -> Self {
        self.notes.push(Note {
            span: None,
            message: message.into(),
        });
        self
    }

    #[must_use]
    pub fn with_note_at(mut self, span: Span, message: impl Into<String>) -> Self {
        self.notes.push(Note {
            span: Some(span),
            message: message.into(),
        });
        self
    }

    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fixes.push(fix);
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}[{}]: {}",
            self.severity, self.code, self.message
        )
    }
}
