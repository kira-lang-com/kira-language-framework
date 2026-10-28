//! The kernel's formatting engine (`KLF-FORMAT.md`).
//!
//! A definition decides its style by describing a layout: the tokens of a
//! file in order, with groups, indentation and line breaks between them.
//! The kernel owns everything a style does not: matching each token against
//! the source, placing the comments the definition's tree dropped, keeping
//! blank lines, fitting groups to the line width, and turning the result
//! into edits that change nothing but whitespace and comment placement.

mod builder;
mod edit;
mod print;
mod trivia;

#[cfg(test)]
mod tests;

pub use builder::Builder;
pub use edit::{TextEdit, edits, restrict};
pub use print::print;
pub use trivia::Syntax;

use std::fmt;

/// What a line break is when its group fits on one line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineKind {
    /// Nothing.
    Soft,
    /// A space.
    Space,
    /// Always a line break; the enclosing groups break too.
    Hard,
    /// Always a space, never a break.
    Fixed,
}

impl LineKind {
    #[must_use]
    pub const fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(Self::Soft),
            1 => Some(Self::Space),
            2 => Some(Self::Hard),
            3 => Some(Self::Fixed),
            _ => None,
        }
    }
}

/// A layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Doc {
    /// Text taken from the source at `at`. `separated` records whether the
    /// source had whitespace or a comment before it.
    Token {
        text: String,
        at: usize,
        separated: bool,
    },
    /// Text the kernel adds: a space, or a comment.
    Text(String),
    Line(LineKind),
    /// Printed flat when it fits, otherwise with every line of its own level
    /// broken. `forced` holds when it contains a hard break.
    Group {
        docs: Vec<Doc>,
        forced: bool,
    },
    Indent {
        docs: Vec<Doc>,
        forced: bool,
    },
    /// Printed before the next line break: a trailing line comment.
    Suffix(String),
    /// A comment on its own line in the source.
    Leading(String),
    /// An empty line before the next line, if it starts one.
    Blank,
}

impl Doc {
    /// Whether the doc forces the groups around it to break.
    #[must_use]
    pub fn forced(&self) -> bool {
        match self {
            Self::Token { text, .. } | Self::Text(text) => text.contains('\n'),
            Self::Line(kind) => *kind == LineKind::Hard,
            Self::Group { forced, .. } | Self::Indent { forced, .. } => *forced,
            Self::Suffix(_) | Self::Leading(_) => true,
            Self::Blank => false,
        }
    }
}

/// How a layout is printed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options {
    pub width: usize,
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub final_newline: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 100,
            tab_size: 4,
            insert_spaces: true,
            final_newline: true,
        }
    }
}

/// Why a layout cannot be applied to its source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatError {
    /// A token did not match the source at `at`.
    Mismatch { at: usize, expected: String },
    /// A span is outside the source, or behind tokens already laid out.
    Span { start: usize, end: usize },
    /// Source text at `at` was never laid out.
    Unconsumed { at: usize },
    /// A group or indentation was closed without being opened, or left open.
    Unbalanced,
    /// The layout joins or splits tokens at `at`.
    Joined { at: usize },
}

impl FormatError {
    /// The source offset the error is about, if any.
    #[must_use]
    pub const fn at(&self) -> Option<usize> {
        match self {
            Self::Mismatch { at, .. } | Self::Unconsumed { at } | Self::Joined { at } => Some(*at),
            Self::Span { start, .. } => Some(*start),
            Self::Unbalanced => None,
        }
    }
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mismatch { at, expected } => {
                write!(
                    formatter,
                    "the layout has `{expected}` where the source at {at} does not"
                )
            }
            Self::Span { start, end } => {
                write!(
                    formatter,
                    "the layout takes {start}..{end}, which is not next in the source"
                )
            }
            Self::Unconsumed { at } => {
                write!(formatter, "the layout leaves out the source from {at}")
            }
            Self::Unbalanced => {
                formatter.write_str("the layout's groups and indents do not balance")
            }
            Self::Joined { at } => {
                write!(formatter, "the layout joins or splits tokens at {at}")
            }
        }
    }
}

impl std::error::Error for FormatError {}

/// A byte order mark, which a source may start with. The builder starts
/// after it and the printed text keeps it.
pub const BOM: char = '\u{feff}';

/// Prints `doc`, laid out over `source`, keeping the source's byte order mark.
///
/// # Errors
/// Returns [`FormatError`] when printing joins or splits tokens.
pub fn print_source(source: &str, doc: &Doc, options: &Options) -> Result<String, FormatError> {
    let text = print(doc, options)?;
    Ok(if source.starts_with(BOM) {
        format!("{BOM}{text}")
    } else {
        text
    })
}

/// Prints a finished layout and answers the edits that turn `source` into it.
///
/// # Errors
/// Returns the first way the layout changes more than whitespace and comment
/// placement.
pub fn format(
    source: &str,
    doc: &Doc,
    options: &Options,
) -> Result<(String, Vec<TextEdit>), FormatError> {
    let text = print_source(source, doc, options)?;
    let edits = edits(source, &text);
    Ok((text, edits))
}
