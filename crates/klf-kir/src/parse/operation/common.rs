//! Helpers shared by the per-family instruction parsers.

use crate::Type;
use crate::parse::{ParseError, error, matching_close, parse_value, split_top_level};
use klf_core::Name;

/// A family parser: `Ok(None)` means the text is not in this family.
pub(super) type Parsed = Result<Option<crate::Operation>, ParseError>;

/// The index of the first `target` that is not nested in brackets.
pub(super) fn find_top_level(text: &str, target: char) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, character) in crate::parse::unquoted(text) {
        if character == target && depth == 0 {
            return Some(index);
        }
        match character {
            '(' | '[' | '<' => depth += 1,
            '>' if index > 0 && text.as_bytes()[index - 1] == b'-' => {}
            ')' | ']' | '>' => depth -= 1,
            _ => {}
        }
    }
    None
}

pub(super) fn split_top_level_once(text: &str, delimiter: char) -> Option<(&str, &str)> {
    let index = find_top_level(text, delimiter)?;
    Some((&text[..index], &text[index + delimiter.len_utf8()..]))
}

pub(super) fn parse_values(text: &str, offset: usize) -> Result<Vec<Name>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|value| parse_value(value, offset))
        .collect()
}

pub(super) fn parse_index<T: std::str::FromStr>(
    text: &str,
    offset: usize,
) -> Result<T, ParseError> {
    text.trim()
        .parse()
        .map_err(|_| error(offset, format!("invalid index `{}`", text.trim())))
}

pub(super) fn parse_type(text: &str, offset: usize) -> Result<Type, ParseError> {
    crate::type_parse::parse_type(text.trim(), offset)
}

/// Splits `head(arg, arg)tail` at the first top-level parenthesis.
pub(super) fn split_call<'a>(
    text: &'a str,
    offset: usize,
    what: &str,
) -> Result<(&'a str, Vec<Name>, &'a str), ParseError> {
    let open = find_top_level(text, '(')
        .ok_or_else(|| error(offset, format!("`{what}` requires an argument list")))?;
    let close = matching_close(text, open, '(', ')')
        .ok_or_else(|| error(offset, format!("unclosed `{what}` arguments")))?;
    Ok((
        text[..open].trim(),
        parse_values(&text[open + 1..close], offset)?,
        text[close + 1..].trim(),
    ))
}

/// Parses the ` -> <type>` result suffix used by the dispatching calls.
pub(super) fn parse_result_suffix(
    text: &str,
    offset: usize,
    what: &str,
) -> Result<Type, ParseError> {
    let result = text
        .trim_start()
        .strip_prefix("->")
        .ok_or_else(|| error(offset, format!("`{what}` requires `-> <type>`")))?;
    parse_type(result, offset)
}
