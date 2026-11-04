use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_types::{FloatType, Type};

use super::{atom, punctuation, top_level_commas, type_range};
use crate::ssa::text_instruction_operands_start;

pub(super) fn specialization_constant_type(
    source: &str,
    instruction: &TextInstruction,
) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(tokens);
    let commas = top_level_commas(tokens, start);
    let (&first, &second) = (commas.first()?, commas.get(1)?);
    if commas.len() != 2 {
        return None;
    }
    let id = tir_format::numeric_literals::parse_unsigned_integer(atom(tokens.get(start)?)?)?;
    if u32::try_from(id).is_err() {
        return None;
    }
    let ty = type_range(source, tokens, first + 1, second)?;
    if tokens.get(second + 1).is_none() {
        return None;
    }
    Some(ty)
}

pub(super) fn specialization_constant_key(
    source: &str,
    instruction: &TextInstruction,
) -> Option<(u32, Type, Vec<String>)> {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(tokens);
    let commas = top_level_commas(tokens, start);
    let &second = commas.get(1)?;
    if instruction.opcode != "gpu.spec_const" || commas.len() != 2 {
        return None;
    }
    let id = tir_format::numeric_literals::parse_unsigned_integer(atom(tokens.get(start)?)?)?;
    let id = u32::try_from(id).ok()?;
    let ty = specialization_constant_type(source, instruction)?;
    if !specialization_constant_default_is_valid(source, instruction) {
        return None;
    }
    let default_tokens = tokens_after_comma(tokens, second)?;
    let values = match &ty {
        Type::FixedVector { element, .. } => vector_literal_elements(default_tokens)?
            .into_iter()
            .map(|tokens| scalar_literal_key(source, tokens, element))
            .collect::<Option<Vec<_>>>()?,
        scalar => vec![scalar_literal_key(source, default_tokens, scalar)?],
    };
    Some((id, ty, values))
}

pub(super) fn specialization_constant_default_is_valid(
    source: &str,
    instruction: &TextInstruction,
) -> bool {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(tokens);
    let commas = top_level_commas(tokens, start);
    let (&first, &second) = match (commas.first(), commas.get(1)) {
        (Some(first), Some(second)) if commas.len() == 2 => (first, second),
        _ => return false,
    };
    let Some(ty) = type_range(source, tokens, first + 1, second) else {
        return false;
    };
    let Some(default_tokens) = tokens_after_comma(tokens, second) else {
        return false;
    };
    match &ty {
        Type::FixedVector { lanes, element } => {
            if !matches!(*lanes, 2 | 3 | 4) || !specialization_lane_type_supported(element) {
                return false;
            }
            let Some(elements) = vector_literal_elements(default_tokens) else {
                return false;
            };
            elements.len() == *lanes as usize
                && elements.iter().all(|element_tokens| {
                    scalar_literal_matches_type(source, element_tokens, element)
                })
        }
        scalar if specialization_scalar_type_supported(scalar) => {
            scalar_literal_matches_type(source, default_tokens, scalar)
        }
        _ => false,
    }
}

fn specialization_scalar_type_supported(ty: &Type) -> bool {
    matches!(ty, Type::Integer(1..=64))
        || matches!(
            ty,
            Type::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64)
        )
}

fn specialization_lane_type_supported(ty: &Type) -> bool {
    specialization_scalar_type_supported(ty)
        || matches!(
            ty,
            Type::Float(
                FloatType::F8E4M3
                    | FloatType::F8E5M2
                    | FloatType::F6E2M3
                    | FloatType::F6E3M2
                    | FloatType::F4E2M1
                    | FloatType::E8M0
            )
        )
}

fn scalar_literal_matches_type(source: &str, tokens: &[Token], ty: &Type) -> bool {
    if let Type::Integer(1) = ty {
        return tokens.len() == 1 && matches!(atom(&tokens[0]), Some("true" | "false"));
    }
    let Some(literal) = token_text(source, tokens) else {
        return false;
    };
    match ty {
        Type::Integer(bits) => tir_format::numeric_literals::integer_literal_fits(literal, *bits),
        Type::Float(float_type) => {
            tir_format::numeric_literals::float_literal_is_exactly_representable(
                literal,
                *float_type,
            )
        }
        _ => false,
    }
}

fn scalar_literal_key(source: &str, tokens: &[Token], ty: &Type) -> Option<String> {
    let literal = token_text(source, tokens)?;
    match ty {
        Type::Integer(1) => match literal {
            "false" => Some("boolean:0".to_owned()),
            "true" => Some("boolean:1".to_owned()),
            _ => None,
        },
        Type::Integer(bits) => tir_format::numeric_literals::integer_literal_bits(literal, *bits)
            .map(|value| format!("integer:{value}")),
        Type::Float(float_type) => {
            tir_format::numeric_literals::float_literal_canonical_value(literal, *float_type)
                .map(|value| format!("float:{value}"))
        }
        _ => None,
    }
}

fn tokens_after_comma(tokens: &[Token], comma: usize) -> Option<&[Token]> {
    let end = tokens
        .iter()
        .rposition(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    (comma + 1 <= end).then_some(&tokens[comma + 1..=end])
}

fn vector_literal_elements(tokens: &[Token]) -> Option<Vec<&[Token]>> {
    let first = tokens
        .iter()
        .position(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    let last = tokens
        .iter()
        .rposition(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    if !punctuation(&tokens[first], '[') || !punctuation(&tokens[last], ']') {
        return None;
    }
    let mut depth = 0i32;
    let mut segment_start = first + 1;
    let mut elements = Vec::new();
    for index in first..=last {
        if let TokenKind::Punctuation(character) = tokens[index].kind {
            match character {
                '[' | '(' | '{' | '<' => depth += 1,
                ']' | ')' | '}' | '>' => {
                    depth -= 1;
                    if depth < 0 {
                        return None;
                    }
                    if depth == 0 {
                        if index != last {
                            return None;
                        }
                        if segment_start < index {
                            elements.push(&tokens[segment_start..index]);
                        } else if !elements.is_empty() {
                            return None;
                        }
                    }
                }
                ',' if depth == 1 => {
                    if segment_start == index {
                        return None;
                    }
                    elements.push(&tokens[segment_start..index]);
                    segment_start = index + 1;
                }
                _ => {}
            }
        }
    }
    (depth == 0 && !elements.is_empty()).then_some(elements)
}

fn token_text<'a>(source: &'a str, tokens: &[Token]) -> Option<&'a str> {
    let first = tokens
        .iter()
        .find(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    let last = tokens
        .iter()
        .rev()
        .find(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))?;
    source.get(first.span.start..last.span.end).map(str::trim)
}
