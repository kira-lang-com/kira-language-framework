use crate::text::{Token, TokenKind};
use crate::{TextItem, TextParseError};
use tir_diag::Span;

pub(super) fn split_ranges(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut part_start = start;
    let mut stack = Vec::new();
    for index in start..end {
        if let TokenKind::Punctuation(character) = tokens[index].kind {
            match character {
                '(' | '[' | '{' | '<' => stack.push(character),
                ')' | ']' | '}' | '>' => {
                    if stack.last().is_some_and(|open| {
                        matches!(
                            (*open, character),
                            ('(', ')') | ('[', ']') | ('{', '}') | ('<', '>')
                        )
                    }) {
                        stack.pop();
                    }
                }
                ',' if stack.is_empty() => {
                    ranges.push((part_start, index));
                    part_start = index + 1;
                }
                _ => {}
            }
        }
    }
    ranges.push((part_start, end));
    ranges
}

pub(super) fn matching(tokens: &[Token], open: usize, left: char, right: char) -> Option<usize> {
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if punctuation(token, left) {
            depth += 1;
        }
        if punctuation(token, right) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

pub(super) fn trim_range(tokens: &[Token], mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end && matches!(tokens[start].kind, TokenKind::Newline | TokenKind::End) {
        start += 1;
    }
    while end > start && matches!(tokens[end - 1].kind, TokenKind::Newline | TokenKind::End) {
        end -= 1;
    }
    (start, end)
}

pub(super) fn find_punctuation(tokens: &[Token], start: usize, value: char) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| punctuation(token, value).then_some(index))
}

pub(super) fn path_end(tokens: &[Token], mut index: usize) -> usize {
    let start = index;
    while index < tokens.len()
        && (atom_at(tokens, index).is_some() || punctuation(&tokens[index], '.'))
    {
        index += 1;
    }
    if index > start && punctuation(&tokens[index - 1], '.') {
        index - 1
    } else {
        index
    }
}

pub(super) fn token_text(
    source: &str,
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<String> {
    let first = tokens.get(start)?;
    let last = tokens.get(end.checked_sub(1)?)?;
    source
        .get(first.span.start..last.span.end)
        .map(str::trim)
        .map(str::to_owned)
}

pub(super) fn token_text_for_path(tokens: &[Token], start: usize, end: usize) -> String {
    let mut result = String::new();
    for token in tokens.get(start..end).unwrap_or_default() {
        match &token.kind {
            TokenKind::Atom(atom) => result.push_str(atom),
            TokenKind::Punctuation('.') => result.push('.'),
            _ => {}
        }
    }
    result
}

pub(super) fn number_after_key(tokens: &[Token], start: usize, key: &str) -> Option<u64> {
    let index = (start..tokens.len()).find(|index| atom_at(tokens, *index) == Some(key))?;
    number_at(tokens, index + 1)
}

pub(super) fn number_at(tokens: &[Token], index: usize) -> Option<u64> {
    atom_at(tokens, index).and_then(parse_u64)
}

pub(super) fn parse_u64(value: &str) -> Option<u64> {
    crate::numeric_literals::parse_unsigned_integer(value)
        .and_then(|value| u64::try_from(value).ok())
}

pub(super) fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    tokens.get(index).and_then(atom)
}

pub(super) fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

pub(super) fn punctuation(token: &Token, value: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == value)
}

pub(super) fn span_at(tokens: &[Token], index: usize) -> Span {
    tokens
        .get(index)
        .map_or(Span::new(0, 0), |token| token.span)
}

pub(super) fn item_error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
