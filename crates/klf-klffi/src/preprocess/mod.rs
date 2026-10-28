//! The C preprocessor (C17 6.10) that `klf ffi gen` runs over a header before
//! the declaration parser reads it (KLF-RIDL section 26.5). It starts from the
//! predefined macros of a target, follows `#include` through the include
//! directories, evaluates conditionals, and expands macros. The output is the
//! header's code with the directives gone, and the object-like macros the
//! header defines with their values.

mod cond;
mod directive;
mod expand;
mod lex;
mod platform;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::preprocess::expand::{Macro, Macros, expand};
use crate::preprocess::lex::{Logical, logical_lines};

pub use platform::{Platform, Predefined};

pub(crate) use expand::render;
pub(crate) use lex::{Fault, Kind, Token, integer_literal, tokenize};

/// What a header is preprocessed for: its predefined macros and the
/// directories `#include` searches after the including file's own.
#[derive(Clone, Copy, Debug)]
pub struct Environment<'a> {
    pub predefined: &'a Predefined,
    pub include_dirs: &'a [PathBuf],
}

/// A preprocessing error at a line of a file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderError {
    pub path: PathBuf,
    pub line: usize,
    pub message: String,
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: error: {}",
            self.path.display(),
            self.line,
            self.message
        )
    }
}

impl std::error::Error for HeaderError {}

/// An object-like macro the header leaves defined, with its expanded value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Define {
    pub name: String,
    pub value: String,
}

/// The result of preprocessing one header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Preprocessed {
    /// The header's code with directives removed and macros expanded.
    pub text: String,
    /// The object-like macros the header defines and leaves defined, in the
    /// order they were first defined.
    pub defines: Vec<Define>,
    /// What was left out without stopping, such as an include that was not found.
    pub remarks: Vec<String>,
}

/// Preprocesses `text`, the contents of the file at `path`. `read` returns
/// the contents of an include candidate, or `None` when it does not exist.
///
/// # Errors
/// Returns [`HeaderError`] at the first malformed directive or expansion.
pub fn preprocess(
    text: &str,
    path: &Path,
    environment: Environment<'_>,
    read: &mut dyn FnMut(&Path) -> Option<String>,
) -> Result<Preprocessed, HeaderError> {
    let mut state = State::new(environment, read)?;
    state.file(path, text)?;
    Ok(state.finish())
}

/// One open `#if` group of the file being read.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Condition {
    pub(crate) line: usize,
    /// Whether the code around the `#if` is active.
    pub(crate) parent: bool,
    /// Whether the code of the current branch is active.
    pub(crate) active: bool,
    pub(crate) branches: Branches,
}

/// Where a `#if` group is: before its `#else`, with whether a branch has been
/// taken yet, or after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Branches {
    Open { taken: bool },
    Else,
}

pub(crate) struct State<'e, 'r> {
    pub(crate) environment: Environment<'e>,
    pub(crate) read: &'r mut dyn FnMut(&Path) -> Option<String>,
    pub(crate) macros: Macros,
    /// Macros the header defined, in first-definition order.
    pub(crate) defined: Vec<String>,
    /// The files being read, the header first.
    pub(crate) includes: Vec<PathBuf>,
    pub(crate) remarks: Vec<String>,
    /// Tokens of the active code lines since the last directive.
    pending: Vec<Token>,
    output: String,
}

impl<'e, 'r> State<'e, 'r> {
    fn new(
        environment: Environment<'e>,
        read: &'r mut dyn FnMut(&Path) -> Option<String>,
    ) -> Result<Self, HeaderError> {
        let mut macros: Macros = HashMap::new();
        for (name, value) in environment.predefined.definitions() {
            let body = tokenize(value, 0).map_err(|fault| HeaderError {
                path: PathBuf::from("<predefined>"),
                line: fault.line,
                message: fault.message,
            })?;
            let expanded = render(&body);
            macros.insert(
                name.clone(),
                Macro {
                    params: None,
                    variadic: false,
                    body,
                    value: expanded,
                },
            );
        }
        Ok(Self {
            environment,
            read,
            macros,
            defined: Vec::new(),
            includes: Vec::new(),
            remarks: Vec::new(),
            pending: Vec::new(),
            output: String::new(),
        })
    }

    fn file(&mut self, path: &Path, text: &str) -> Result<(), HeaderError> {
        self.includes.push(path.to_path_buf());
        let result = self.lines(text);
        self.includes.pop();
        result
    }

    fn lines(&mut self, text: &str) -> Result<(), HeaderError> {
        let logical = logical_lines(text).map_err(|fault| self.fault(fault))?;
        let mut conditions: Vec<Condition> = Vec::new();
        for Logical { line, text } in logical {
            let trimmed = text.trim_start();
            if let Some(directive) = trimmed.strip_prefix('#') {
                self.flush()?;
                self.directive(line, directive, &mut conditions)?;
            } else if conditions.last().is_none_or(|condition| condition.active)
                && !trimmed.is_empty()
            {
                let tokens = tokenize(trimmed, line).map_err(|fault| self.fault(fault))?;
                self.pending.extend(tokens);
            }
        }
        self.flush()?;
        if let Some(open) = conditions.last() {
            return Err(self.error(open.line, "`#if` group is not closed by `#endif`"));
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), HeaderError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let tokens = std::mem::take(&mut self.pending);
        let expanded = expand(tokens, &self.macros).map_err(|fault| self.fault(fault))?;
        self.output.push_str(&render(&expanded));
        self.output.push('\n');
        Ok(())
    }

    fn finish(self) -> Preprocessed {
        let defines = self
            .defined
            .iter()
            .filter_map(|name| {
                let definition = self.macros.get(name)?;
                definition.params.is_none().then(|| Define {
                    name: name.clone(),
                    value: definition.value.clone(),
                })
            })
            .collect();
        Preprocessed {
            text: self.output,
            defines,
            remarks: self.remarks,
        }
    }

    pub(crate) fn error(&self, line: usize, message: impl Into<String>) -> HeaderError {
        HeaderError {
            path: self.current(),
            line,
            message: message.into(),
        }
    }

    pub(crate) fn fault(&self, fault: Fault) -> HeaderError {
        self.error(fault.line, fault.message)
    }
}
