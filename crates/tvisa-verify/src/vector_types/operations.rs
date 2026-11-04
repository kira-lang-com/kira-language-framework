use tir_diag::Diagnostic;
use tir_format::TextInstruction;
use tir_types::Type;

use crate::calls::opcode_end;
use crate::vector_inference::{atom, operand_type_argument, shuffle_indices, shuffle_lane_count};

use super::{
    fixed_lanes, is_integer_vector, is_pointer_vector, mask_matches, push, same_lanes, valid_lanes,
    vector_element,
};

pub(super) fn verify_shuffle(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(Type::FixedVector { lanes, element }) = operands.first() else {
        push(
            diagnostics,
            instruction,
            "vec.shuffle requires fixed-vector operands",
        );
        return;
    };
    if operands.get(1) != operands.first() {
        push(
            diagnostics,
            instruction,
            "vec.shuffle operands must have the same type",
        );
    }
    let Some(indices) = shuffle_indices(instruction) else {
        push(
            diagnostics,
            instruction,
            "vec.shuffle requires a constant index list",
        );
        return;
    };
    if indices
        .iter()
        .any(|index| *index >= lanes.saturating_mul(2))
    {
        push(
            diagnostics,
            instruction,
            "vec.shuffle index exceeds the concatenated input vectors",
        );
    }
    let count = shuffle_lane_count(instruction).unwrap_or_default();
    if !valid_lanes(count) || element.as_ref() == &Type::Integer(1) && count > 64 {
        push(
            diagnostics,
            instruction,
            "vec.shuffle result has an invalid lane count",
        );
    }
}

pub(super) fn verify_reduce(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(element) = operands.first().and_then(vector_element) else {
        push(
            diagnostics,
            instruction,
            "vec.reduce requires a vector operand",
        );
        return;
    };
    if let Some(mask) = operands.get(1)
        && !mask_matches(mask, operands.first().expect("checked vector operand"))
    {
        push(
            diagnostics,
            instruction,
            "vec.reduce mask must match the vector lane count",
        );
    }
    let operation = instruction
        .tokens
        .get(opcode_end(instruction))
        .and_then(atom);
    let float = matches!(element, Type::Float(_));
    let valid = match operation {
        Some("fadd" | "fmul" | "fmin" | "fmax" | "fminnum" | "fmaxnum") => float,
        Some("add" | "mul" | "and" | "or" | "xor" | "smin" | "smax" | "umin" | "umax") => {
            matches!(element, Type::Integer(_))
        }
        _ => false,
    };
    if !valid {
        push(
            diagnostics,
            instruction,
            "vec.reduce operation is incompatible with its element type",
        );
    }
}

pub(super) fn verify_load(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(vector) = operand_type_argument(source, instruction) else {
        return;
    };
    if vector_element(&vector).is_none() {
        push(diagnostics, instruction, "vec.load type must be a vector");
    }
    if operands
        .first()
        .is_some_and(|pointer| !matches!(pointer, Type::Pointer(_)))
    {
        push(
            diagnostics,
            instruction,
            "vec.load address must be a pointer",
        );
    }
    verify_optional_mask_and_passthru(instruction, &vector, operands, 0, diagnostics);
}

pub(super) fn verify_store(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(vector) = operands.first() else {
        return;
    };
    if vector_element(vector).is_none() {
        push(diagnostics, instruction, "vec.store value must be a vector");
    }
    if operands
        .get(1)
        .is_some_and(|pointer| !matches!(pointer, Type::Pointer(_)))
    {
        push(
            diagnostics,
            instruction,
            "vec.store address must be a pointer",
        );
    }
    if let Some(mask) = operands.get(2)
        && !mask_matches(mask, vector)
    {
        push(
            diagnostics,
            instruction,
            "vec.store mask must match the value's lane count",
        );
    }
}

pub(super) fn verify_gather(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(vector) = operand_type_argument(source, instruction) else {
        return;
    };
    if vector_element(&vector).is_none() {
        push(diagnostics, instruction, "vec.gather type must be a vector");
    }
    let Some(address) = operands.first() else {
        return;
    };
    if is_pointer_vector(address) {
        if !same_lanes(address, &vector) {
            push(
                diagnostics,
                instruction,
                "vec.gather pointer vector must match the result lane count",
            );
        }
        if !matches!(operands.len(), 1 | 3) {
            push(
                diagnostics,
                instruction,
                "vec.gather pointer-vector form requires one or three value operands",
            );
        }
    } else if !matches!(address, Type::Pointer(_)) {
        push(
            diagnostics,
            instruction,
            "vec.gather address must be a pointer or pointer vector",
        );
    } else {
        if !matches!(operands.len(), 2 | 4) {
            push(
                diagnostics,
                instruction,
                "vec.gather base-offset form requires two or four value operands",
            );
        }
        if let Some(offsets) = operands.get(1) {
            if !is_integer_vector(offsets) {
                push(
                    diagnostics,
                    instruction,
                    "vec.gather offsets must be an integer vector",
                );
            } else if !same_lanes(offsets, &vector) {
                push(
                    diagnostics,
                    instruction,
                    "vec.gather offsets must match the result lane count",
                );
            }
        }
    }
    let base = if is_pointer_vector(operands.first().unwrap_or(&Type::Unit)) {
        0
    } else {
        1
    };
    verify_optional_mask_and_passthru(instruction, &vector, operands, base, diagnostics);
}

pub(super) fn verify_scatter(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let (Some(vector), Some(address)) = (operands.first(), operands.get(1)) else {
        return;
    };
    if vector_element(vector).is_none() {
        push(
            diagnostics,
            instruction,
            "vec.scatter value must be a vector",
        );
    }
    let mask_index = if is_pointer_vector(address) {
        if !matches!(operands.len(), 2 | 3) {
            push(
                diagnostics,
                instruction,
                "vec.scatter pointer-vector form takes an optional mask",
            );
        }
        if !same_lanes(vector, address) {
            push(
                diagnostics,
                instruction,
                "vec.scatter pointer vector must match the value lane count",
            );
        }
        2
    } else if matches!(address, Type::Pointer(_)) {
        if !matches!(operands.len(), 3 | 4) {
            push(
                diagnostics,
                instruction,
                "vec.scatter base-offset form requires offsets and an optional mask",
            );
        }
        if !operands.get(2).is_some_and(is_integer_vector)
            || !operands
                .get(2)
                .is_some_and(|offsets| same_lanes(vector, offsets))
        {
            push(
                diagnostics,
                instruction,
                "vec.scatter base form requires matching integer offset lanes",
            );
        }
        3
    } else {
        push(
            diagnostics,
            instruction,
            "vec.scatter address must be a pointer or vector of pointers",
        );
        return;
    };
    if let Some(mask) = operands.get(mask_index)
        && !mask_matches(mask, vector)
    {
        push(
            diagnostics,
            instruction,
            "vec.scatter mask must match the value's lane count",
        );
    }
}

pub(super) fn verify_vector_and_mask(
    instruction: &TextInstruction,
    operands: &[Type],
    expected: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.len() != expected {
        push(
            diagnostics,
            instruction,
            "vector mask operation has the wrong number of operands",
        );
        return;
    }
    let vector = &operands[0];
    if vector_element(vector).is_none() || operands[2] != *vector {
        push(
            diagnostics,
            instruction,
            "vector operation requires matching vector values",
        );
    }
    if !mask_matches(&operands[1], vector) {
        push(
            diagnostics,
            instruction,
            "vector operation mask must match the value lane count",
        );
    }
}

pub(super) fn verify_dot(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.len() < 3 {
        return;
    }
    let kind = instruction
        .tokens
        .get(opcode_end(instruction))
        .and_then(atom);
    let (input_element, accumulator_element, factor) = match kind {
        Some("sdot" | "udot" | "usdot") => (Type::Integer(8), Type::Integer(32), 4),
        Some("bfdot") => (
            Type::Float(tir_types::FloatType::Bf16),
            Type::Float(tir_types::FloatType::F32),
            2,
        ),
        _ => {
            push(
                diagnostics,
                instruction,
                "vec.dot requires a supported dot-product kind",
            );
            return;
        }
    };
    let (Some(accumulator_lanes), Some(left_lanes), Some(right_lanes)) = (
        fixed_lanes(&operands[0]),
        fixed_lanes(&operands[1]),
        fixed_lanes(&operands[2]),
    ) else {
        push(
            diagnostics,
            instruction,
            "vec.dot operands must be fixed vectors",
        );
        return;
    };
    if vector_element(&operands[0]) != Some(accumulator_element)
        || vector_element(&operands[1]) != Some(input_element.clone())
        || vector_element(&operands[2]) != Some(input_element)
        || left_lanes != accumulator_lanes.saturating_mul(factor)
        || right_lanes != left_lanes
    {
        push(
            diagnostics,
            instruction,
            "vec.dot operands have incompatible element types or lane counts",
        );
    }
}

pub(super) fn verify_two_vectors(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.first().and_then(vector_element).is_none() || operands.get(1) != operands.first() {
        push(
            diagnostics,
            instruction,
            "vector operands must have the same vector type",
        );
    }
}

pub(super) fn verify_optional_choice(
    instruction: &TextInstruction,
    choices: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let start = opcode_end(instruction);
    let Some(last_value) = instruction.tokens[start..]
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| matches!(pair[0].kind, tir_format::text::TokenKind::Punctuation('%')))
        .map(|(index, _)| start + index + 1)
        .last()
    else {
        return;
    };
    let tail_start = last_value + 1;
    match instruction.tokens.get(tail_start..) {
        Some([]) => {}
        Some([token]) if atom(token).is_some_and(|choice| choices.contains(&choice)) => {}
        _ => push(
            diagnostics,
            instruction,
            format!(
                "{} accepts an optional selector from {}",
                instruction.opcode,
                choices.join(" or ")
            ),
        ),
    }
}

pub(super) fn verify_table(
    instruction: &TextInstruction,
    operands: &[Type],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.len() < 2 {
        return;
    }
    if vector_element(&operands[0]) != Some(Type::Integer(8)) || !is_integer_vector(&operands[1]) {
        push(
            diagnostics,
            instruction,
            "vec.table requires an i8 table and integer indices",
        );
    }
}

fn verify_optional_mask_and_passthru(
    instruction: &TextInstruction,
    vector: &Type,
    operands: &[Type],
    base: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(mask) = operands.get(base + 1)
        && !mask_matches(mask, vector)
    {
        push(
            diagnostics,
            instruction,
            "vector mask must match the result lane count",
        );
    }
    if let Some(passthru) = operands.get(base + 2)
        && passthru != vector
    {
        push(
            diagnostics,
            instruction,
            "vector passthrough must match the result type",
        );
    }
}
