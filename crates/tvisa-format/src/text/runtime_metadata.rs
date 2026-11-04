use tir_diag::Span;
use tir_format::numeric_literals::parse_unsigned_integer;
use tir_format::text::{Token, TokenKind, lex};

use super::{TextItem, TextItemKind, TextParseError, error};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextRuntimeMetadata {
    pub identifiers: Vec<u16>,
    pub span: Span,
}

pub(super) fn parse(
    source: &str,
    items: &[TextItem],
) -> Result<Vec<TextRuntimeMetadata>, TextParseError> {
    let mut metadata = Vec::new();
    for item in items
        .iter()
        .filter(|item| item.kind == TextItemKind::Metadata)
    {
        let text = source
            .get(item.span.start..item.span.end)
            .unwrap_or_default();
        let tokens = lex(text).map_err(|cause| TextParseError {
            span: shift(cause.span, item.span.start),
            message: cause.message.to_owned(),
        })?;
        let tokens: Vec<_> = tokens
            .iter()
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
            .collect();
        if tokens.len() < 2
            || atom(tokens[0]) != Some("metadata")
            || atom(tokens[1]) != Some("runtime")
        {
            continue;
        }
        metadata.push(parse_item(&tokens, item.span)?);
    }
    Ok(metadata)
}

fn parse_item(tokens: &[&Token], item_span: Span) -> Result<TextRuntimeMetadata, TextParseError> {
    if tokens.len() < 4
        || !punctuation(tokens[2], '[')
        || !punctuation(tokens[tokens.len() - 1], ']')
    {
        return Err(error(item_span, "malformed metadata runtime item"));
    }
    let mut identifiers = Vec::new();
    let mut cursor = 3;
    let end = tokens.len() - 1;
    while cursor < end {
        let Some(literal) = atom(tokens[cursor]) else {
            return Err(error(item_span, "malformed metadata runtime identifier"));
        };
        if !literal.starts_with("0x") && !literal.starts_with("0X") {
            return Err(error(item_span, "runtime identifiers must be hexadecimal"));
        }
        let identifier = parse_unsigned_integer(literal)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| error(item_span, "runtime identifier is outside the 16-bit range"))?;
        if identifiers
            .last()
            .is_some_and(|previous| *previous >= identifier)
        {
            return Err(error(
                item_span,
                "runtime identifiers must be strictly increasing",
            ));
        }
        identifiers.push(identifier);
        cursor += 1;
        if cursor < end {
            if !punctuation(tokens[cursor], ',') {
                return Err(error(item_span, "expected `,` between runtime identifiers"));
            }
            cursor += 1;
            if cursor == end {
                return Err(error(item_span, "expected runtime identifier after `,`"));
            }
        }
    }
    Ok(TextRuntimeMetadata {
        identifiers,
        span: item_span,
    })
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn shift(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
