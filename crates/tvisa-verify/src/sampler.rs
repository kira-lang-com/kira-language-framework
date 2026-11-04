use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_types::FloatType;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

struct Field {
    name: String,
    value: String,
    span: Span,
}

pub(super) fn verify(source: &str, instruction: &TextInstruction) -> Vec<Diagnostic> {
    let fields = match parse_fields(source, instruction) {
        Ok(fields) => fields,
        Err(diagnostic) => return vec![diagnostic],
    };
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();
    for field in fields {
        if !seen.insert(field.name.clone()) {
            diagnostics.push(error(
                field.span,
                format!("const.sampler repeats field '{}'", field.name),
            ));
            continue;
        }
        let valid = match field.name.as_str() {
            "min" | "mag" | "mip" => matches!(field.value.as_str(), "nearest" | "linear"),
            "address_u" | "address_v" | "address_w" => matches!(
                field.value.as_str(),
                "clamp_to_edge"
                    | "repeat"
                    | "mirrored_repeat"
                    | "clamp_to_border"
                    | "mirror_clamp_to_edge"
            ),
            "border" => matches!(
                field.value.as_str(),
                "transparent_black" | "opaque_black" | "opaque_white"
            ),
            "max_anisotropy" => tir_format::numeric_literals::parse_unsigned_integer(&field.value)
                .is_some_and(|value| (1..=16).contains(&value)),
            "compare" => matches!(
                field.value.as_str(),
                "never"
                    | "less"
                    | "equal"
                    | "less_equal"
                    | "greater"
                    | "not_equal"
                    | "greater_equal"
                    | "always"
            ),
            "lod_min" | "lod_max" | "lod_bias" => {
                tir_format::numeric_literals::float_literal_is_exactly_representable(
                    &field.value,
                    FloatType::F32,
                )
            }
            "reduction" => {
                matches!(field.value.as_str(), "weighted_average" | "min" | "max")
            }
            "normalized_coordinates" => matches!(field.value.as_str(), "true" | "false"),
            _ => {
                diagnostics.push(error(
                    field.span,
                    format!("unknown const.sampler field '{}'", field.name),
                ));
                continue;
            }
        };
        if !valid {
            diagnostics.push(error(
                field.span,
                format!("invalid value for const.sampler field '{}'", field.name),
            ));
        }
    }
    diagnostics
}

fn parse_fields(source: &str, instruction: &TextInstruction) -> Result<Vec<Field>, Diagnostic> {
    let tokens = &instruction.tokens;
    let start = crate::calls::opcode_end(instruction);
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| is_punctuation(token, '{').then_some(index))
    else {
        return Err(error(
            instruction.span,
            "const.sampler requires a field object",
        ));
    };
    let mut cursor = open + 1;
    let mut fields = Vec::new();
    loop {
        skip_newlines(tokens, &mut cursor);
        if is_punctuation_at(tokens, cursor, '}') {
            cursor += 1;
            break;
        }
        let Some(Token {
            kind: TokenKind::Atom(name),
            span: name_span,
        }) = tokens.get(cursor)
        else {
            return Err(error(
                instruction.span,
                "const.sampler expected a field name",
            ));
        };
        let name = name.clone();
        let start_span = *name_span;
        cursor += 1;
        skip_newlines(tokens, &mut cursor);
        if !is_punctuation_at(tokens, cursor, ':') {
            return Err(error(start_span, "const.sampler field requires ':'"));
        }
        cursor += 1;
        skip_newlines(tokens, &mut cursor);
        let Some(first) = tokens.get(cursor) else {
            return Err(error(start_span, "const.sampler field has no value"));
        };
        let value_start = first.span.start;
        let mut last_span = first.span;
        while cursor < tokens.len()
            && !is_punctuation_at(tokens, cursor, ',')
            && !is_punctuation_at(tokens, cursor, '}')
        {
            if !matches!(tokens[cursor].kind, TokenKind::Newline | TokenKind::End) {
                last_span = tokens[cursor].span;
            }
            cursor += 1;
        }
        let Some(value) = source
            .get(value_start..last_span.end)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(error(start_span, "const.sampler field has no value"));
        };
        fields.push(Field {
            name,
            value: value.to_owned(),
            span: Span::new(start_span.start, last_span.end),
        });
        if is_punctuation_at(tokens, cursor, ',') {
            cursor += 1;
            skip_newlines(tokens, &mut cursor);
            if is_punctuation_at(tokens, cursor, '}') {
                return Err(error(
                    tokens[cursor].span,
                    "const.sampler does not allow a trailing comma",
                ));
            }
        } else if is_punctuation_at(tokens, cursor, '}') {
            cursor += 1;
            break;
        } else {
            return Err(error(
                instruction.span,
                "const.sampler field object is not closed",
            ));
        }
    }
    if tokens[cursor..]
        .iter()
        .any(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
    {
        return Err(error(
            instruction.span,
            "unexpected tokens after const.sampler",
        ));
    }
    Ok(fields)
}

fn skip_newlines(tokens: &[Token], cursor: &mut usize) {
    while tokens
        .get(*cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Newline))
    {
        *cursor += 1;
    }
}

fn is_punctuation_at(tokens: &[Token], index: usize, expected: char) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| is_punctuation(token, expected))
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn error(span: Span, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(VFY3, message.into()).with_primary_span(span)
}
