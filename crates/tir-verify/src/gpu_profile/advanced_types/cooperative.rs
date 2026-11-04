use tir_diag::Diagnostic;
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};
use tir_types::{MatrixScope, Type};

use super::{cooperative_element, error, is_integer_type, matrix_type};
use crate::ssa::text_instruction_operands_start;

pub(super) fn verify_coop_reduce(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(element) = operands.first().and_then(cooperative_element) else {
        error(
            diagnostics,
            instruction,
            "coop.reduce requires a cooperative matrix",
        );
        return;
    };
    let operations = [
        "add.s", "add.u", "add.f", "mul.s", "mul.u", "mul.f", "min.s", "min.u", "min.f", "max.s",
        "max.u", "max.f", "and", "or", "xor",
    ];
    let axes = ["row", "column", "quad2x2"];
    let words = words_after_opcode(instruction);
    if words
        .iter()
        .any(|word| !operations.contains(&word.as_str()) && !axes.contains(&word.as_str()))
    {
        error(
            diagnostics,
            instruction,
            "coop.reduce contains an unsupported operator or axis",
        );
    }
    let selected = words
        .iter()
        .map(String::as_str)
        .filter(|word| operations.contains(word))
        .collect::<Vec<_>>();
    if selected.len() != 1 {
        error(
            diagnostics,
            instruction,
            "coop.reduce must select exactly one reduction operator",
        );
    }
    let valid_type = match selected.first().copied() {
        Some("add.f" | "mul.f" | "min.f" | "max.f") => is_float_type(element),
        Some(
            "add.s" | "add.u" | "mul.s" | "mul.u" | "min.s" | "min.u" | "max.s" | "max.u" | "and"
            | "or" | "xor",
        ) => is_integer_type(element),
        _ => false,
    };
    if !valid_type {
        error(
            diagnostics,
            instruction,
            "coop.reduce operator is incompatible with the matrix element type",
        );
    }
    let row_count = words.iter().filter(|word| word.as_str() == "row").count();
    let column_count = words
        .iter()
        .filter(|word| word.as_str() == "column")
        .count();
    let quad_count = words
        .iter()
        .filter(|word| word.as_str() == "quad2x2")
        .count();
    if row_count + column_count + quad_count > 1 {
        error(
            diagnostics,
            instruction,
            "coop.reduce selects more than one axis",
        );
    }
    if quad_count != 0
        && !operands
            .first()
            .and_then(matrix_type)
            .is_some_and(|(scope, rows, columns, _)| {
                scope == MatrixScope::Subgroup && rows % 2 == 0 && columns % 2 == 0
            })
    {
        error(
            diagnostics,
            instruction,
            "coop.reduce quad2x2 requires an even subgroup matrix shape",
        );
    }
}

fn is_float_type(ty: &Type) -> bool {
    matches!(ty, Type::Float(_))
}

pub(super) fn words_after_opcode(instruction: &TextInstruction) -> Vec<String> {
    let mut words = Vec::new();
    let mut index = text_instruction_operands_start(&instruction.tokens);
    while index < instruction.tokens.len() {
        if is_punctuation(&instruction.tokens[index], '%') {
            index += 2;
            continue;
        }
        if let Some(value) = instruction.tokens.get(index).and_then(atom) {
            if instruction
                .tokens
                .get(index + 1)
                .is_some_and(|token| is_punctuation(token, '.'))
                && let Some(suffix) = instruction.tokens.get(index + 2).and_then(atom)
            {
                words.push(format!("{value}.{suffix}"));
                index += 3;
                continue;
            }
            words.push(value.to_owned());
        }
        index += 1;
    }
    words
}

pub(super) fn has_word(instruction: &TextInstruction, expected: &str) -> bool {
    words_after_opcode(instruction)
        .iter()
        .any(|word| word == expected)
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
