use tir_diag::Diagnostic;
use tir_format::TextInstruction;
use tir_types::{MatrixScope, Type};

use super::helpers::{
    cooperative_element, error, has_modifier, is_float_type, is_integer_type, matrix_type,
    type_after_atom, verify_words, words_after_opcode,
};

pub(super) fn verify_coop_convert(
    instruction: &TextInstruction,
    source: &str,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some((input_scope, input_rows, input_columns, _)) = operands.first().and_then(matrix_type)
    else {
        error(
            diagnostics,
            instruction,
            "coop.convert requires a cooperative matrix",
        );
        return;
    };
    let Some(target) = type_after_atom(source, instruction, "to") else {
        error(
            diagnostics,
            instruction,
            "coop.convert requires a target matrix type",
        );
        return;
    };
    let Some((target_scope, target_rows, target_columns, _)) = matrix_type(&target) else {
        error(
            diagnostics,
            instruction,
            "coop.convert target must be a cooperative matrix",
        );
        return;
    };
    if (input_scope, input_rows, input_columns) != (target_scope, target_rows, target_columns) {
        error(
            diagnostics,
            instruction,
            "coop.convert must preserve matrix scope and dimensions",
        );
    }
}

pub(super) fn verify_mma(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let (
        Some((a_scope, a_rows, a_columns, a_use)),
        Some((b_scope, b_rows, b_columns, b_use)),
        Some((c_scope, c_rows, c_columns, c_use)),
    ) = (
        operands.first().and_then(matrix_type),
        operands.get(1).and_then(matrix_type),
        operands.get(2).and_then(matrix_type),
    )
    else {
        error(
            diagnostics,
            instruction,
            "coop.mma requires three cooperative matrix operands",
        );
        return;
    };
    if a_use != "a" || b_use != "b" || c_use != "acc" {
        error(
            diagnostics,
            instruction,
            "coop.mma operands must use a, b, and acc roles",
        );
    }
    if a_scope != b_scope || a_scope != c_scope {
        error(
            diagnostics,
            instruction,
            "coop.mma operands must have the same scope",
        );
    }
    if a_columns != b_rows || a_rows != c_rows || b_columns != c_columns {
        error(
            diagnostics,
            instruction,
            "coop.mma matrix dimensions are incompatible",
        );
    }
    if operands.first().and_then(cooperative_element)
        != operands.get(1).and_then(cooperative_element)
    {
        error(
            diagnostics,
            instruction,
            "coop.mma input matrices must have the same element type",
        );
    }
    let accumulator_is_integer = operands
        .get(2)
        .and_then(cooperative_element)
        .is_some_and(is_integer_type);
    if has_modifier(instruction, "saturate") && !accumulator_is_integer {
        error(
            diagnostics,
            instruction,
            "coop.mma saturate requires an integer accumulator",
        );
    }
    if has_modifier(instruction, "unsigned")
        && !operands
            .first()
            .and_then(cooperative_element)
            .is_some_and(is_integer_type)
    {
        error(
            diagnostics,
            instruction,
            "coop.mma unsigned requires integer inputs",
        );
    }
}

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
    let words = words_after_opcode(instruction);
    verify_words(
        instruction,
        &words,
        &[
            "add.s", "add.u", "add.f", "mul.s", "mul.u", "mul.f", "min.s", "min.u", "min.f",
            "max.s", "max.u", "max.f", "and", "or", "xor", "row", "column", "quad2x2",
        ],
        "coop.reduce",
        diagnostics,
    );
    let operation = words
        .iter()
        .map(String::as_str)
        .find(|word| operations.contains(word));
    let operation_count = words
        .iter()
        .filter(|word| operations.contains(&word.as_str()))
        .count();
    if operation_count > 1 {
        error(
            diagnostics,
            instruction,
            "coop.reduce selects more than one operator",
        );
    }
    let valid_type = match operation {
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
            "coop.reduce operator is missing or incompatible with the matrix element type",
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
    let quad = quad_count != 0;
    if row_count + column_count + quad_count > 1 {
        error(
            diagnostics,
            instruction,
            "coop.reduce selects more than one axis",
        );
    }
    if quad
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
