use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};

use crate::calls::opcode_end;

pub(super) fn query_name(instruction: &TextInstruction) -> Option<&str> {
    let start = opcode_end(instruction);
    let comma = instruction.tokens[start..]
        .iter()
        .position(|token| is_punctuation(token, ','))?
        + start;
    atom(instruction.tokens.get(comma + 1)?)
}

pub(super) fn has_sparse_modifier(instruction: &TextInstruction) -> bool {
    instruction.tokens.iter().enumerate().any(|(index, token)| {
        atom(token) == Some("sparse")
            && (index == 0 || !is_punctuation(&instruction.tokens[index - 1], '%'))
    })
}

pub(super) fn has_keyword(instruction: &TextInstruction, keyword: &str) -> bool {
    let start = opcode_end(instruction);
    instruction.tokens[start..]
        .iter()
        .enumerate()
        .any(|(index, token)| {
            atom(token) == Some(keyword)
                && (index == 0 || !is_punctuation(&instruction.tokens[start + index - 1], '%'))
        })
}

pub(super) fn keyword_value<'a>(
    instruction: &'a TextInstruction,
    keyword: &str,
) -> Option<&'a str> {
    let start = opcode_end(instruction);
    instruction.tokens[start..].windows(2).find_map(|pair| {
        (atom(&pair[0]) == Some(keyword))
            .then(|| atom(&pair[1]))
            .flatten()
    })
}

pub(super) fn keyword_atom(
    instruction: &TextInstruction,
    options: &[&'static str],
) -> Option<&'static str> {
    options
        .iter()
        .copied()
        .find(|option| has_keyword(instruction, option))
}

pub(super) fn offset_components(instruction: &TextInstruction) -> Option<Vec<i128>> {
    let start = opcode_end(instruction);
    let keyword = instruction.tokens[start..]
        .iter()
        .enumerate()
        .find(|(index, token)| {
            atom(token) == Some("offset")
                && (*index == 0 || !is_punctuation(&instruction.tokens[start + index - 1], '%'))
        })?
        .0
        + start;
    let open = instruction
        .tokens
        .iter()
        .enumerate()
        .skip(keyword + 1)
        .find(|(_, token)| is_punctuation(token, '['))?
        .0;
    let close = instruction
        .tokens
        .iter()
        .enumerate()
        .skip(open + 1)
        .find(|(_, token)| is_punctuation(token, ']'))?
        .0;
    let mut values = Vec::new();
    let mut index = open + 1;
    while index < close {
        if is_punctuation(&instruction.tokens[index], ',') {
            index += 1;
            continue;
        }
        if is_punctuation(&instruction.tokens[index], '-') {
            let magnitude = atom(instruction.tokens.get(index + 1)?)?;
            let value = tir_format::numeric_literals::parse_unsigned_integer(magnitude)?;
            values.push(-i128::try_from(value).ok()?);
            index += 2;
            continue;
        }
        let literal = atom(&instruction.tokens[index])?;
        values.push(tir_format::numeric_literals::parse_signed_integer(literal)?);
        index += 1;
    }
    Some(values)
}

pub(super) fn has_offset_marker(instruction: &TextInstruction) -> bool {
    has_keyword(instruction, "offset")
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
