use std::collections::HashMap;

use tir_format::text::{Token, TokenKind};
use tir_format::{TextInstruction, parse_tvisa_type};
use tir_types::Type;

use crate::calls::opcode_end;

pub(super) fn infer_result_types(
    source: &str,
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Vec<Type> {
    let operands = value_operand_types(instruction, value_types);
    let result = match instruction.opcode.as_str() {
        "vec.splat" => type_after_atom(source, instruction, "to"),
        "vec.step" | "vec.load" | "vec.gather" => explicit_type(source, instruction),
        "vec.extract" => operands
            .first()
            .and_then(crate::vector_types::vector_element),
        "vec.insert" | "vec.compress" | "vec.expand" | "vec.dot" | "vec.zip" | "vec.unzip"
        | "vec.reverse" | "vec.slide" => operands.first().cloned(),
        "vec.shuffle" => shuffle_lane_count(instruction)
            .zip(
                operands
                    .first()
                    .and_then(crate::vector_types::vector_element),
            )
            .and_then(|(lanes, element)| {
                crate::vector_types::valid_lanes(lanes).then_some(Type::FixedVector {
                    lanes,
                    element: Box::new(element),
                })
            }),
        "vec.reduce" => operands
            .first()
            .and_then(crate::vector_types::vector_element),
        "vec.mask.active" => Some(Type::ScalableVector(Box::new(Type::Integer(1)))),
        "vec.mask.count" | "vec.mask.first" => Some(Type::Integer(64)),
        "vec.concat" => operands
            .first()
            .zip(operands.get(1))
            .and_then(|(left, right)| crate::vector_types::concatenate_type(left, right)),
        "vec.split" => operands.first().and_then(crate::vector_types::split_type),
        "vec.table" => operands
            .get(1)
            .and_then(crate::vector_types::table_result_type),
        _ => None,
    };
    result.into_iter().collect()
}

pub(super) fn operand_type_argument(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let start = opcode_end(instruction);
    let first = instruction.tokens.get(start)?;
    let mut depth = 0i32;
    for token in instruction.tokens.iter().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => {
                    return parse_tvisa_type(source.get(first.span.start..token.span.start)?).ok();
                }
                _ => {}
            }
        }
    }
    None
}

pub(super) fn explicit_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let start = opcode_end(instruction);
    let first = instruction.tokens.get(start)?;
    let mut depth = 0i32;
    for token in instruction.tokens.iter().skip(start) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => {
                    return parse_tvisa_type(source.get(first.span.start..token.span.start)?).ok();
                }
                _ => {}
            }
        }
    }
    parse_tvisa_type(source.get(first.span.start..instruction.tokens.last()?.span.end)?).ok()
}

pub(super) fn type_after_atom(
    source: &str,
    instruction: &TextInstruction,
    marker: &str,
) -> Option<Type> {
    let index = instruction
        .tokens
        .iter()
        .position(|token| atom(token) == Some(marker))?
        + 1;
    let first = instruction.tokens.get(index)?;
    let end = if let Some(token) = instruction
        .tokens
        .iter()
        .skip(index + 1)
        .find(|token| matches!(token.kind, TokenKind::Punctuation('[' | ',')))
    {
        token.span.start
    } else {
        instruction.tokens.last()?.span.end
    };
    parse_tvisa_type(source.get(first.span.start..end)?).ok()
}

fn value_operand_types(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Vec<Type> {
    instruction.tokens[opcode_end(instruction)..]
        .windows(2)
        .filter_map(|window| {
            if !matches!(window[0].kind, TokenKind::Punctuation('%')) {
                return None;
            }
            let TokenKind::Atom(name) = &window[1].kind else {
                return None;
            };
            value_types.get(name).cloned()
        })
        .collect()
}

pub(super) fn shuffle_lane_count(instruction: &TextInstruction) -> Option<u32> {
    Some(shuffle_indices(instruction)?.len() as u32)
}

pub(super) fn shuffle_indices(instruction: &TextInstruction) -> Option<Vec<u32>> {
    let open = instruction
        .tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('[')))?;
    let close = instruction.tokens[open + 1..]
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation(']')))?
        + open
        + 1;
    instruction.tokens[open + 1..close]
        .iter()
        .filter_map(atom)
        .map(|value| {
            tir_format::numeric_literals::parse_unsigned_integer(value)
                .and_then(|number| u32::try_from(number).ok())
        })
        .collect()
}

pub(super) fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}
