use std::collections::HashSet;

use tir_diag::Span;
use tir_types::Type;

use crate::text::{Token, TokenKind};

use super::BodyParseError;

pub(super) fn validate(
    source: &str,
    tokens: &[Token],
    opcode: &str,
    opcode_end: usize,
    tvisa: bool,
) -> Result<Option<super::TextDebugAttachment>, BodyParseError> {
    let Some(mut cursor) = tokens.iter().position(|token| punctuation(token, '!')) else {
        return missing_debug_attachment(opcode, tokens);
    };
    let mut seen = HashSet::new();
    let mut debug_attachment = None;
    let mut saw_generic_metadata = false;

    while cursor < tokens.len() {
        let marker = tokens[cursor].span;
        let (name, name_end) = attachment_name(tokens, cursor)?;
        if !seen.insert(name.clone()) {
            return Err(error(
                marker,
                format!("metadata !{name} occurs more than once on an instruction"),
            ));
        }
        let open = name_end + 1;
        if !tokens
            .get(open)
            .is_some_and(|token| punctuation(token, '('))
        {
            return Err(error(
                tokens.get(open).map_or(marker, |token| token.span),
                "metadata attachment requires parentheses",
            ));
        }
        let close = matching_parenthesis(tokens, open)?;
        let values = &tokens[open + 1..close];
        match name.as_str() {
            "var" => {
                if saw_generic_metadata {
                    return Err(error(marker, "!var must precede trailing metadata"));
                }
                if !matches!(opcode, "debug.value" | "debug.addr") {
                    return Err(error(
                        marker,
                        "!var is only valid on debug.value and debug.addr",
                    ));
                }
                let value = debug_value_operand(tokens, opcode, opcode_end, cursor)?;
                let (name, ty, scope) = parse_debug_variable(source, values, marker)?;
                debug_attachment = Some(super::TextDebugAttachment::Variable {
                    value,
                    name,
                    ty,
                    scope,
                });
            }
            "label" => {
                if saw_generic_metadata {
                    return Err(error(marker, "!label must precede trailing metadata"));
                }
                if opcode != "debug.label" || cursor != opcode_end {
                    return Err(error(
                        marker,
                        "!label is only valid as the operand of debug.label",
                    ));
                }
                let (name, scope) = parse_debug_label(values, marker)?;
                debug_attachment = Some(super::TextDebugAttachment::Label { name, scope });
            }
            _ => {
                saw_generic_metadata = true;
                if tvisa && name != "loc" {
                    return Err(error(
                        tokens[cursor + 1].span,
                        "TVISA instructions only permit !loc metadata",
                    ));
                }
                if name == "loc" {
                    validate_location(values, marker)?;
                } else {
                    validate_literal_list(values, marker)?;
                }
            }
        }
        cursor = close + 1;
        if cursor < tokens.len() && !punctuation(&tokens[cursor], '!') {
            return Err(error(
                tokens[cursor].span,
                "metadata attachments must follow instruction operands",
            ));
        }
    }
    if debug_attachment.is_none() && matches!(opcode, "debug.value" | "debug.addr" | "debug.label")
    {
        return Err(error(
            tokens.get(opcode_end.saturating_sub(1)).map_or_else(
                || tokens.last().map_or(Span::new(0, 0), |token| token.span),
                |token| token.span,
            ),
            format!("{opcode} requires a !var or !label debug attachment"),
        ));
    }
    Ok(debug_attachment)
}

fn missing_debug_attachment(
    opcode: &str,
    tokens: &[Token],
) -> Result<Option<super::TextDebugAttachment>, BodyParseError> {
    if matches!(opcode, "debug.value" | "debug.addr" | "debug.label") {
        return Err(error(
            tokens.last().map_or(Span::new(0, 0), |token| token.span),
            format!("{opcode} requires a !var or !label debug attachment"),
        ));
    }
    Ok(None)
}

fn attachment_name(tokens: &[Token], cursor: usize) -> Result<(String, usize), BodyParseError> {
    let marker = tokens[cursor].span;
    let first_name = tokens
        .get(cursor + 1)
        .and_then(atom)
        .ok_or_else(|| error(marker, "metadata attachment requires a name"))?;
    let mut name = first_name.to_owned();
    let mut name_end = cursor + 1;
    while tokens
        .get(name_end + 1)
        .is_some_and(|token| punctuation(token, '.'))
    {
        let component = tokens
            .get(name_end + 2)
            .and_then(atom)
            .ok_or_else(|| error(tokens[name_end + 1].span, "malformed metadata name"))?;
        name.push('.');
        name.push_str(component);
        name_end += 2;
    }
    Ok((name, name_end))
}

fn debug_value_operand(
    tokens: &[Token],
    opcode: &str,
    opcode_end: usize,
    marker_index: usize,
) -> Result<String, BodyParseError> {
    let marker = tokens[marker_index].span;
    let value_token = tokens
        .get(opcode_end)
        .ok_or_else(|| error(marker, format!("{opcode} requires one value operand")))?;
    let name_token = tokens
        .get(opcode_end + 1)
        .ok_or_else(|| error(marker, format!("{opcode} requires one value operand")))?;
    if !punctuation(value_token, '%')
        || !tokens
            .get(opcode_end + 2)
            .is_some_and(|token| punctuation(token, ','))
        || marker_index != opcode_end + 3
    {
        return Err(error(
            marker,
            format!("{opcode} requires the form `%value, !var(...)`"),
        ));
    }
    atom(name_token)
        .map(str::to_owned)
        .ok_or_else(|| error(name_token.span, "debug value operand requires a name"))
}

fn parse_debug_variable(
    source: &str,
    tokens: &[Token],
    span: Span,
) -> Result<(String, Type, String), BodyParseError> {
    let fields = split_arguments(tokens, span)?;
    if fields.len() != 3 {
        return Err(error(span, "!var requires a name, a type and a scope"));
    }
    let name = parse_nonempty_string(fields[0], "!var name", span)?;
    let scope = parse_nonempty_string(fields[2], "!var scope", span)?;
    let ty_tokens = fields[1];
    let first = ty_tokens
        .first()
        .ok_or_else(|| error(span, "!var requires a source type"))?;
    let last = ty_tokens.last().unwrap_or(first);
    let ty_source = source
        .get(first.span.start..last.span.end)
        .ok_or_else(|| error(first.span, "invalid !var source type span"))?;
    let ty = crate::parse_type(ty_source).map_err(|type_error| BodyParseError {
        span: Span::new(
            first.span.start + type_error.span.start,
            first.span.start + type_error.span.end,
        ),
        message: type_error.message,
    })?;
    Ok((name, ty, scope))
}

fn parse_debug_label(tokens: &[Token], span: Span) -> Result<(String, String), BodyParseError> {
    let fields = split_arguments(tokens, span)?;
    if fields.len() != 2 {
        return Err(error(span, "!label requires a name and a scope"));
    }
    Ok((
        parse_nonempty_string(fields[0], "!label name", span)?,
        parse_nonempty_string(fields[1], "!label scope", span)?,
    ))
}

fn parse_nonempty_string(
    tokens: &[Token],
    field: &str,
    span: Span,
) -> Result<String, BodyParseError> {
    let [
        Token {
            kind: TokenKind::String(raw),
            span: token_span,
        },
    ] = tokens
    else {
        return Err(error(span, format!("{field} must be a quoted string")));
    };
    let value = crate::text::decode_kira_string_literal_contents(raw)
        .map_err(|escape_error| error(*token_span, escape_error.to_string()))?;
    if value.is_empty() {
        return Err(error(*token_span, format!("{field} must not be empty")));
    }
    Ok(value)
}

fn split_arguments<'a>(
    tokens: &'a [Token],
    span: Span,
) -> Result<Vec<&'a [Token]>, BodyParseError> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut fields = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Punctuation('(' | '[' | '<' | '{') => depth += 1,
            TokenKind::Punctuation(')' | ']' | '>' | '}') => {
                let Some(next) = depth.checked_sub(1) else {
                    return Err(error(
                        token.span,
                        "unmatched closing delimiter in debug attachment",
                    ));
                };
                depth = next;
            }
            TokenKind::Punctuation(',') if depth == 0 => {
                if start == index {
                    return Err(error(
                        token.span,
                        "debug attachment arguments must not be empty",
                    ));
                }
                fields.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(error(span, "unterminated type in debug attachment"));
    }
    if start == tokens.len() {
        return Err(error(span, "debug attachment arguments must not be empty"));
    }
    fields.push(&tokens[start..]);
    Ok(fields)
}

fn validate_location(tokens: &[Token], span: Span) -> Result<(), BodyParseError> {
    let valid = matches!(tokens, [Token { kind: TokenKind::String(_), .. }, colon1, line, colon2, column]
        if matches!(&tokens[0].kind, TokenKind::String(path) if crate::text::decode_kira_string_literal_contents(path).is_ok())
            && punctuation(colon1, ':')
            && unsigned_integer(line).is_some_and(|value| value > 0)
            && punctuation(colon2, ':')
            && unsigned_integer(column).is_some_and(|value| value > 0));
    if valid {
        Ok(())
    } else {
        Err(error(
            span,
            "!loc requires a valid escaped path and positive line and column numbers",
        ))
    }
}

fn validate_literal_list(tokens: &[Token], span: Span) -> Result<(), BodyParseError> {
    if tokens.is_empty() {
        return Ok(());
    }
    let mut start = 0;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Punctuation('(') => depth += 1,
            TokenKind::Punctuation(')') => {
                let Some(next) = depth.checked_sub(1) else {
                    return Err(error(token.span, "unmatched ')' in metadata"));
                };
                depth = next;
            }
            TokenKind::Punctuation(',') if depth == 0 => {
                if !is_literal(&tokens[start..index]) {
                    return Err(error(token.span, "metadata values must be literals"));
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(error(span, "unterminated literal in metadata"));
    }
    if !is_literal(&tokens[start..]) {
        return Err(error(span, "metadata values must be literals"));
    }
    Ok(())
}

fn is_literal(tokens: &[Token]) -> bool {
    match tokens {
        [
            Token {
                kind: TokenKind::String(_),
                ..
            },
        ] => true,
        [
            Token {
                kind: TokenKind::Atom(value),
                ..
            },
        ] => is_scalar_literal(value),
        [
            Token {
                kind: TokenKind::Atom(prefix),
                ..
            },
            Token {
                kind: TokenKind::String(_),
                ..
            },
        ] => prefix == "b",
        [
            Token {
                kind: TokenKind::Atom(value),
                ..
            },
            Token {
                kind: TokenKind::Punctuation('('),
                ..
            },
            rest @ ..,
            Token {
                kind: TokenKind::Punctuation(')'),
                ..
            },
        ] if value == "nan" => signed_integer_tokens(rest).is_some(),
        _ => false,
    }
}

fn is_scalar_literal(value: &str) -> bool {
    matches!(value, "true" | "false" | "null" | "inf" | "-inf")
        || crate::numeric_literals::parse_unsigned_integer(value).is_some()
        || crate::numeric_literals::parse_signed_integer(value).is_some()
        || value.parse::<f64>().is_ok()
}

fn signed_integer_tokens(tokens: &[Token]) -> Option<i128> {
    let [
        Token {
            kind: TokenKind::Atom(value),
            ..
        },
    ] = tokens
    else {
        return None;
    };
    crate::numeric_literals::parse_signed_integer(value)
}

fn unsigned_integer(token: &Token) -> Option<u128> {
    atom(token).and_then(crate::numeric_literals::parse_unsigned_integer)
}

fn matching_parenthesis(tokens: &[Token], open: usize) -> Result<usize, BodyParseError> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::Punctuation('(') => depth += 1,
            TokenKind::Punctuation(')') => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| error(token.span, "unmatched ')' in metadata attachment"))?;
                if depth == 0 {
                    return Ok(index);
                }
            }
            _ => {}
        }
    }
    Err(error(tokens[open].span, "unterminated metadata attachment"))
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

fn error(span: Span, message: impl Into<String>) -> BodyParseError {
    BodyParseError {
        span,
        message: message.into(),
    }
}
