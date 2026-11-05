use std::fmt;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatError {
    pub diagnostic: Diagnostic,
    pub offset: Option<usize>,
}

impl FormatError {
    pub(crate) fn malformed(offset: Option<usize>, message: impl Into<String>) -> Self {
        Self::coded(offset, 1, message)
    }

    pub(crate) fn noncanonical(offset: Option<usize>, message: impl Into<String>) -> Self {
        Self::coded(offset, 2, message)
    }

    pub(crate) fn unknown_section(offset: Option<usize>, message: impl Into<String>) -> Self {
        Self::coded(offset, 4, message)
    }

    pub(crate) fn unsupported_type(offset: Option<usize>, message: impl Into<String>) -> Self {
        Self::coded(offset, 3, message)
    }

    fn coded(offset: Option<usize>, number: u16, message: impl Into<String>) -> Self {
        let mut diagnostic =
            Diagnostic::error(Code::new(Dialect::Tvisa, Area::Encoding, number), message);
        if let Some(offset) = offset {
            diagnostic = diagnostic.with_primary_span(Span::new(offset, offset.saturating_add(1)));
        }
        Self { diagnostic, offset }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.diagnostic, formatter)
    }
}

impl std::error::Error for FormatError {}
