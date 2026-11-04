use tir_diag::Span;

use crate::body::{
    BodyParseError, TextFunctionSignature, matching_paren, parse_entry_parameter, parse_error,
    split_parameters, token_atom,
};
use crate::text::{Token, TokenKind, lex};
use crate::{TypeParseError, parse_type};
use tir_types::Type;

const ENTRY_PARAMETER_KINDS: &[&str] = &[
    "root",
    "immediate",
    "payload",
    "attrs",
    "data",
    "record",
    "input",
];

/// Parses a callable TIR header's parameter, result, and failure types.
///
/// Function attributes are intentionally left to the verifier that owns them.
/// A function type may itself carry `fails`, `async`, or `abi` modifiers, so
/// candidate type slices are parsed from longest to shortest before the outer
/// function's optional failure result is interpreted.
pub fn parse_function_signature(
    source: &str,
) -> Result<Option<TextFunctionSignature>, BodyParseError> {
    let tokens = lex(source).map_err(|error| BodyParseError {
        span: error.span,
        message: error.message.to_owned(),
    })?;
    let Some(keyword) = tokens.first().and_then(token_atom) else {
        return Ok(None);
    };
    match keyword {
        "func" => parse_func_signature(source, &tokens).map(Some),
        "entry" => parse_entry_signature(source, &tokens).map(Some),
        _ => Ok(None),
    }
}

/// Returns the token index where declaration attributes begin for a function.
///
/// This is shared with the text-to-binary assembler so signature attributes are
/// parsed after the result and optional failure types have been consumed using
/// the same type-prefix rules as the public signature parser.
pub(super) fn function_attribute_start(source: &str) -> Result<Option<usize>, BodyParseError> {
    let tokens = lex(source).map_err(|error| BodyParseError {
        span: error.span,
        message: error.message.to_owned(),
    })?;
    if tokens.first().and_then(token_atom) != Some("func") {
        return Ok(None);
    }
    let (open, close) = parameter_parens(&tokens)?;
    let arrow = tokens[close + 1..]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Arrow))
        .map(|index| close + 1 + index)
        .ok_or_else(|| parse_error(tokens[close].span, "function signature is missing '->'"))?;
    let (_, result_end) = parse_type_prefix(source, &tokens, arrow + 1)?;
    let (_, attribute_start) = parse_optional_failure(source, &tokens, result_end)?;
    let _ = open;
    Ok(Some(attribute_start))
}

/// Reports whether a function declaration explicitly selects the C ABI.
///
/// Function types nested in parameters, results or failure types are parsed as
/// part of the signature and do not affect the declaration's own ABI.
pub fn function_uses_c_abi(source: &str) -> Result<Option<bool>, BodyParseError> {
    let tokens = lex(source).map_err(|error| BodyParseError {
        span: error.span,
        message: error.message.to_owned(),
    })?;
    if tokens.first().and_then(token_atom) != Some("func") {
        return Ok(None);
    }
    let (_, close) = parameter_parens(&tokens)?;
    let arrow = tokens[close + 1..]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Arrow))
        .map(|index| close + 1 + index)
        .ok_or_else(|| parse_error(tokens[close].span, "function signature is missing '->'"))?;
    let (_, result_end) = parse_type_prefix(source, &tokens, arrow + 1)?;
    let (_, attributes_start) = parse_optional_failure(source, &tokens, result_end)?;
    for (index, token) in tokens.iter().enumerate().skip(attributes_start) {
        if token_atom(token) != Some("abi") {
            continue;
        }
        let is_c_abi = tokens.get(index + 1..index + 4).is_some_and(|suffix| {
            matches!(suffix[0].kind, TokenKind::Punctuation('('))
                && token_atom(&suffix[1]) == Some("c")
                && matches!(suffix[2].kind, TokenKind::Punctuation(')'))
        });
        return Ok(Some(is_c_abi));
    }
    Ok(Some(false))
}

fn parse_func_signature(
    source: &str,
    tokens: &[Token],
) -> Result<TextFunctionSignature, BodyParseError> {
    let (open, close) = parameter_parens(tokens)?;
    let parameters = split_parameters(source, &tokens[open + 1..close], parse_type)?;
    let arrow = tokens[close + 1..]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Arrow))
        .map(|index| close + 1 + index)
        .ok_or_else(|| parse_error(tokens[close].span, "function signature is missing '->'"))?;
    let (result, result_end) = parse_type_prefix(source, tokens, arrow + 1)?;
    let (failure, _) = parse_optional_failure(source, tokens, result_end)?;
    Ok(TextFunctionSignature {
        parameters,
        result,
        failure,
    })
}

fn parse_entry_signature(
    source: &str,
    tokens: &[Token],
) -> Result<TextFunctionSignature, BodyParseError> {
    let (open, close) = parameter_parens(tokens)?;
    let parameters = parse_entry_parameters(source, &tokens[open + 1..close])?;
    let arrow = tokens[close + 1..]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Arrow))
        .map(|index| close + 1 + index);
    let result = if let Some(arrow) = arrow {
        parse_type_prefix(source, tokens, arrow + 1)?.0
    } else {
        Type::Unit
    };
    Ok(TextFunctionSignature {
        parameters,
        result,
        failure: None,
    })
}

fn parameter_parens(tokens: &[Token]) -> Result<(usize, usize), BodyParseError> {
    let mut nesting = [0i32; 3];
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '[' => nesting[0] += 1,
                ']' => nesting[0] -= 1,
                '<' => nesting[1] += 1,
                '>' => nesting[1] -= 1,
                '(' if nesting == [0; 3] => {
                    return matching_paren(tokens, index).map(|close| (index, close));
                }
                _ => {}
            }
        }
    }
    Err(parse_error(
        tokens.first().map_or(Span::new(0, 0), |token| token.span),
        "callable signature is missing its parameter list",
    ))
}

fn parse_entry_parameters(
    source: &str,
    tokens: &[Token],
) -> Result<Vec<crate::body::TextParameter>, BodyParseError> {
    let mut parameters = Vec::new();
    for part in split_top_level(tokens, ',')? {
        let Some(name_index) = part
            .iter()
            .position(|token| matches!(token.kind, TokenKind::Punctuation('%')))
        else {
            let span = part.first().map_or(Span::new(0, 0), |token| token.span);
            return Err(parse_error(
                span,
                "entry parameter is missing its value name",
            ));
        };
        if name_index != 1
            || !part
                .first()
                .and_then(token_atom)
                .is_some_and(|kind| ENTRY_PARAMETER_KINDS.contains(&kind))
        {
            return Err(parse_error(
                part.first().map_or(Span::new(0, 0), |token| token.span),
                "entry parameter is missing its kind",
            ));
        }
        let parameter = parse_entry_parameter(source, &part[name_index..], parse_type)?;
        if parameter.context || parameter.convention.is_some() {
            return Err(parse_error(
                parameter.span,
                "entry parameters do not have function parameter modifiers",
            ));
        }
        parameters.push(parameter);
    }
    Ok(parameters)
}

fn split_top_level(tokens: &[Token], delimiter: char) -> Result<Vec<&[Token]>, BodyParseError> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = [0i32; 4];
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => depth[0] += 1,
                ')' => depth[0] -= 1,
                '[' => depth[1] += 1,
                ']' => depth[1] -= 1,
                '<' => depth[2] += 1,
                '>' => depth[2] -= 1,
                '{' => depth[3] += 1,
                '}' => depth[3] -= 1,
                value if value == delimiter && depth == [0; 4] => {
                    if index == start {
                        return Err(parse_error(
                            token.span,
                            "empty parameter in entry signature",
                        ));
                    }
                    parts.push(&tokens[start..index]);
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    if start == tokens.len() {
        return Err(parse_error(
            tokens.last().map_or(Span::new(0, 0), |token| token.span),
            "trailing comma in entry parameter list",
        ));
    }
    parts.push(&tokens[start..]);
    Ok(parts)
}

fn parse_type_prefix(
    source: &str,
    tokens: &[Token],
    start: usize,
) -> Result<(Type, usize), BodyParseError> {
    let limit = header_end(tokens, start);
    let Some(first) = tokens.get(start).filter(|_| start < limit) else {
        return Err(parse_error(
            tokens
                .get(start.saturating_sub(1))
                .map_or(Span::new(0, 0), |token| token.span),
            "callable signature is missing a result type",
        ));
    };
    let mut best = None;
    let mut last_error = None;
    for end in start + 1..=limit {
        let last = &tokens[end - 1];
        let slice_start = first.span.start;
        let slice_end = last.span.end;
        let slice = &source[slice_start..slice_end];
        match parse_type(slice) {
            Ok(ty) => best = Some((ty, end)),
            Err(error) => last_error = Some((error, slice_start)),
        }
    }
    if let Some(best) = best {
        return Ok(best);
    }
    if let Some((error, offset)) = last_error {
        return Err(map_type_error(error, offset));
    }
    Err(parse_error(first.span, "expected a result type"))
}

fn parse_optional_failure(
    source: &str,
    tokens: &[Token],
    cursor: usize,
) -> Result<(Option<Type>, usize), BodyParseError> {
    if tokens.get(cursor).and_then(token_atom) != Some("fails") {
        return Ok((None, cursor));
    }
    let (failure, end) = parse_type_prefix(source, tokens, cursor + 1)?;
    Ok((Some(failure), end))
}

fn header_end(tokens: &[Token], start: usize) -> usize {
    let mut depth = [0i32; 3];
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => depth[0] += 1,
                ')' => depth[0] -= 1,
                '[' => depth[1] += 1,
                ']' => depth[1] -= 1,
                '<' => depth[2] += 1,
                '>' => depth[2] -= 1,
                '{' if depth == [0; 3] => return index,
                _ => {}
            }
        }
        if matches!(token.kind, TokenKind::End | TokenKind::Newline) && depth == [0; 3] {
            return index;
        }
    }
    tokens.len()
}

fn map_type_error(error: TypeParseError, offset: usize) -> BodyParseError {
    BodyParseError {
        span: Span::new(
            offset.saturating_add(error.span.start),
            offset.saturating_add(error.span.end),
        ),
        message: error.message,
    }
}
