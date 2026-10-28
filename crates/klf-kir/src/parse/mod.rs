//! The textual KIR reader (appendix A.4).

mod function;
mod items;
mod operation;
pub(crate) mod tables;

use crate::{BlockTarget, Module, Stage};
use klf_core::{FileId, Name};
use klf_source::Span;
use klf_target::Target;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "KLF-KIR-0001 at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Parses one textual KIR module.
///
/// # Errors
///
/// Returns [`ParseError`] when the input does not conform to the textual KIR
/// grammar implemented by this release.
pub fn parse_module(text: &str) -> Result<Module, ParseError> {
    let mut lines = Lines::new(text);
    let (header_offset, header) = lines.next_required("module header")?;
    let mut module = parse_header(header_offset, header)?;
    while let Some((offset, line)) = lines.next() {
        parse_item(&mut module, offset, line, &mut lines)?;
    }
    Ok(module)
}

fn parse_item(
    module: &mut Module,
    offset: usize,
    line: &str,
    lines: &mut Lines<'_>,
) -> Result<(), ParseError> {
    if line.starts_with("type ") {
        let (_, policy) = lines.next_required("type policy record")?;
        module
            .types
            .push(crate::declaration::parse_type_declaration(
                line, policy, offset,
            )?);
    } else if line.starts_with("func ") {
        module
            .functions
            .push(function::parse_function(offset, line, lines)?);
    } else if line.starts_with("global ") {
        module.globals.push(items::parse_global(offset, line)?);
    } else if line.starts_with("witness ") {
        module.witnesses.push(items::parse_witness(offset, line)?);
    } else if line.starts_with("attribute ") {
        module
            .attributes
            .push(items::parse_attribute(offset, line)?);
    } else if line.starts_with("vtable ") {
        module.vtables.push(items::parse_vtable(offset, line)?);
    } else if line.starts_with("trait ") {
        module.traits.push(items::parse_trait(offset, line)?);
    } else if line.starts_with("conformance ") {
        module
            .conformances
            .push(items::parse_conformance(offset, line)?);
    } else if line.starts_with("import ") {
        module.imports.push(items::parse_import(offset, line)?);
    } else if line.starts_with("export ") {
        module.exports.push(items::parse_export(offset, line)?);
    } else {
        return Err(error(offset, "expected a module item"));
    }
    Ok(())
}

fn parse_header(offset: usize, line: &str) -> Result<Module, ParseError> {
    let rest = line
        .strip_prefix("module @")
        .ok_or_else(|| error(offset, "expected `module @Name`"))?;
    let (name_text, rest) = rest
        .split_once(" lang ")
        .ok_or_else(|| error(offset, "module header is missing `lang`"))?;
    let (language, rest) = take_quoted(rest, offset)?;
    let rest = rest
        .strip_prefix(" target ")
        .ok_or_else(|| error(offset, "module header is missing `target`"))?;
    let (target, rest) = take_quoted(rest, offset)?;
    let stage = match rest.trim().strip_prefix("stage ") {
        Some("raw") => Stage::Raw,
        Some("canonical") => Stage::Canonical,
        _ => return Err(error(offset, "module stage must be `raw` or `canonical`")),
    };
    let target = target
        .parse::<Target>()
        .map_err(|cause| error(offset, cause.to_string()))?;
    Ok(Module::empty(
        symbol(name_text, offset)?,
        language,
        target,
        stage,
    ))
}

pub(crate) fn parse_target(text: &str, offset: usize) -> Result<BlockTarget, ParseError> {
    let text = text.trim();
    let Some(open) = text.find('(') else {
        return Ok(BlockTarget {
            block: Name::new(text),
            arguments: Vec::new(),
        });
    };
    let close = matching_close(text, open, '(', ')')
        .ok_or_else(|| error(offset, "unclosed block arguments"))?;
    if close + 1 != text.len() {
        return Err(error(offset, "unexpected text after block target"));
    }
    let body = &text[open + 1..close];
    let arguments = if body.trim().is_empty() {
        Vec::new()
    } else {
        split_top_level(body, ',')
            .into_iter()
            .map(|value| parse_value(value, offset))
            .collect::<Result<_, _>>()?
    };
    Ok(BlockTarget {
        block: Name::new(&text[..open]),
        arguments,
    })
}

pub(crate) fn parse_two_values(text: &str, offset: usize) -> Result<(Name, Name), ParseError> {
    let (lhs, rhs) = split_once_required(text, ",", offset)?;
    Ok((parse_value(lhs, offset)?, parse_value(rhs, offset)?))
}

pub(crate) fn strip_optional_mode(text: &str) -> &str {
    let trimmed = text.trim_start();
    match trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
    {
        Some((_, after)) => after.trim_start(),
        None => trimmed,
    }
}

pub(crate) fn parse_value(text: &str, offset: usize) -> Result<Name, ParseError> {
    text.trim()
        .strip_prefix('%')
        .filter(|name| !name.is_empty())
        .map(Name::new)
        .ok_or_else(|| error(offset, "SSA value must start with `%`"))
}

pub(crate) fn split_top_level(text: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, character) in unquoted(text) {
        match character {
            '(' | '[' | '<' => depth += 1,
            '>' if index > 0 && text.as_bytes()[index - 1] == b'-' => {}
            ')' | ']' | '>' => depth -= 1,
            value if value == delimiter && depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + value.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

pub(crate) fn matching_close(
    text: &str,
    open: usize,
    opening: char,
    closing: char,
) -> Option<usize> {
    let mut depth = 0_u32;
    for (relative, character) in unquoted(&text[open..]) {
        if character == opening {
            depth += 1;
        } else if character == closing {
            depth -= 1;
            if depth == 0 {
                return Some(open + relative);
            }
        }
    }
    None
}

/// The characters of `text` outside the quoted symbols and strings it holds.
pub(crate) fn unquoted(text: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut resume = 0;
    text.char_indices().filter(move |&(index, character)| {
        if index < resume {
            return false;
        }
        if character == '"' {
            resume = quoted_end(text, index);
            return false;
        }
        true
    })
}

/// The index just past the closing quote of the quoted text opening at
/// `open`, or the end of `text` when it is unclosed.
pub(crate) fn quoted_end(text: &str, open: usize) -> usize {
    let bytes = text.as_bytes();
    let mut index = open + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

/// The first `target` of `text` outside quoted text.
pub(crate) fn find_unquoted(text: &str, target: char) -> Option<usize> {
    unquoted(text)
        .find(|&(_, character)| character == target)
        .map(|(index, _)| index)
}

/// The symbol an `@` introduces: spelled bare, or quoted when it holds a
/// character a bare symbol may not.
pub(crate) fn symbol(text: &str, offset: usize) -> Result<Name, ParseError> {
    let text = text.trim();
    if text.starts_with('"') {
        return Ok(Name::new(unquote(text, offset)?));
    }
    Ok(Name::new(text))
}

pub(crate) fn unquote(text: &str, offset: usize) -> Result<String, ParseError> {
    let inner = text
        .trim()
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| error(offset, "expected a quoted string"))?;
    let mut result = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => return Err(error(offset, "unescaped quote in string")),
            '\\' => {
                let escaped = characters
                    .next()
                    .ok_or_else(|| error(offset, "unfinished string escape"))?;
                let decoded = match escaped {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '0' => '\0',
                    '\\' => '\\',
                    '"' => '"',
                    'x' => {
                        let digits: String = characters.by_ref().take(2).collect();
                        if digits.len() != 2 {
                            return Err(error(offset, "short hexadecimal escape"));
                        }
                        let byte = u8::from_str_radix(&digits, 16)
                            .map_err(|_| error(offset, "invalid hexadecimal escape"))?;
                        if byte > 127 {
                            return Err(error(offset, "hexadecimal escape is not ASCII"));
                        }
                        char::from(byte)
                    }
                    'u' => {
                        if characters.next() != Some('{') {
                            return Err(error(offset, "expected `{` in Unicode escape"));
                        }
                        let mut digits = String::new();
                        let mut closed = false;
                        for digit in characters.by_ref() {
                            if digit == '}' {
                                closed = true;
                                break;
                            }
                            if !digit.is_ascii_hexdigit() || digits.len() >= 6 {
                                return Err(error(offset, "invalid Unicode escape"));
                            }
                            digits.push(digit);
                        }
                        if !closed {
                            return Err(error(offset, "unfinished Unicode escape"));
                        }
                        let code = u32::from_str_radix(&digits, 16)
                            .map_err(|_| error(offset, "invalid Unicode escape"))?;
                        char::from_u32(code)
                            .ok_or_else(|| error(offset, "invalid Unicode scalar"))?
                    }
                    _ => return Err(error(offset, "unknown string escape")),
                };
                result.push(decoded);
            }
            character => result.push(character),
        }
    }
    Ok(result)
}

pub(crate) fn take_quoted(text: &str, offset: usize) -> Result<(String, &str), ParseError> {
    let rest = text
        .strip_prefix('"')
        .ok_or_else(|| error(offset, "expected a quoted string"))?;
    let end = rest
        .find('"')
        .ok_or_else(|| error(offset, "unterminated quoted string"))?;
    Ok((rest[..end].to_owned(), &rest[end + 1..]))
}

pub(crate) fn split_once_required<'a>(
    text: &'a str,
    delimiter: &str,
    offset: usize,
) -> Result<(&'a str, &'a str), ParseError> {
    text.split_once(delimiter)
        .ok_or_else(|| error(offset, format!("expected `{delimiter}`")))
}

pub(crate) fn span(offset: usize, length: usize) -> Span {
    Span::new(
        FileId::from_u32(0),
        u32::try_from(offset).expect("KIR source exceeds 4 GiB"),
        u32::try_from(offset + length).expect("KIR source exceeds 4 GiB"),
    )
}

pub(crate) fn error(offset: usize, message: impl Into<String>) -> ParseError {
    ParseError {
        offset,
        message: message.into(),
    }
}

pub(crate) struct Lines<'a> {
    source: &'a str,
    cursor: usize,
    peeked: Option<(usize, &'a str)>,
}

impl<'a> Lines<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            cursor: 0,
            peeked: None,
        }
    }

    fn next(&mut self) -> Option<(usize, &'a str)> {
        if let Some(line) = self.peeked.take() {
            return Some(line);
        }
        self.read_next()
    }

    fn peek(&mut self) -> Option<(usize, &'a str)> {
        if self.peeked.is_none() {
            self.peeked = self.read_next();
        }
        self.peeked
    }

    fn next_required(&mut self, expected: &str) -> Result<(usize, &'a str), ParseError> {
        self.next()
            .ok_or_else(|| error(self.source.len(), format!("expected {expected}")))
    }

    fn read_next(&mut self) -> Option<(usize, &'a str)> {
        while self.cursor < self.source.len() {
            let start = self.cursor;
            let remaining = &self.source[start..];
            let length = remaining.find('\n').unwrap_or(remaining.len());
            self.cursor = start + length + usize::from(length < remaining.len());
            let line = strip_line_comment(&remaining[..length]).trim();
            if !line.is_empty() {
                return Some((start, line));
            }
        }
        None
    }
}

fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quoted = false;
    let mut escaped = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b'/' && index + 1 < bytes.len() && bytes[index + 1] == b'/' {
            return &line[..index];
        }
        index += 1;
    }
    line
}
