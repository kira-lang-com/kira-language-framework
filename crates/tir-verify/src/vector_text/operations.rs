use super::*;

pub(super) fn verify_splat(
    source: &str,
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(vector) = type_after_keyword(source, instruction, "to") else {
        report(
            diagnostics,
            instruction,
            "vec.splat requires a target vector type after `to`",
        );
        return false;
    };
    let Some(element) = vector_element(&vector) else {
        report(
            diagnostics,
            instruction,
            "vec.splat target must be a vector type",
        );
        return false;
    };
    if operands.len() != 1 || operands.first().is_some_and(|value| *value != element) {
        report(
            diagnostics,
            instruction,
            "vec.splat value must match the target vector element type",
        );
        return false;
    }
    true
}

pub(super) fn verify_extract(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if operands.len() != 2 || operands.first().and_then(|ty| vector_element(ty)).is_none() {
        report(
            diagnostics,
            instruction,
            "vec.extract requires a vector and integer index",
        );
        return false;
    }
    if !is_integer_scalar(operands[1]) {
        report(
            diagnostics,
            instruction,
            "vec.extract index must be an integer",
        );
        return false;
    }
    true
}

pub(super) fn verify_insert(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if operands.len() != 3 {
        report(
            diagnostics,
            instruction,
            "vec.insert requires three operands",
        );
        return false;
    }
    let Some(element) = vector_element(operands[0]) else {
        report(
            diagnostics,
            instruction,
            "vec.insert requires a vector operand",
        );
        return false;
    };
    if !is_integer_scalar(operands[1]) || operands[2] != element {
        report(
            diagnostics,
            instruction,
            "vec.insert requires an integer index and matching element value",
        );
        return false;
    }
    true
}

pub(super) fn verify_shuffle(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let (
        Some(Type::FixedVector {
            lanes: left_lanes,
            element,
        }),
        Some(Type::FixedVector {
            lanes: right_lanes,
            element: right_element,
        }),
    ) = (operands.first().copied(), operands.get(1).copied())
    else {
        report(
            diagnostics,
            instruction,
            "vec.shuffle requires two fixed-vector operands",
        );
        return false;
    };
    if element != right_element {
        report(
            diagnostics,
            instruction,
            "vec.shuffle operands must have matching element types",
        );
        return false;
    }
    let Some(indices) = shuffle_indices(instruction) else {
        report(
            diagnostics,
            instruction,
            "vec.shuffle requires a constant unsigned index list",
        );
        return false;
    };
    let limit = left_lanes.checked_add(*right_lanes);
    if limit.is_none_or(|limit| indices.iter().any(|index| *index >= limit)) {
        report(
            diagnostics,
            instruction,
            "vec.shuffle index exceeds the concatenated inputs",
        );
        return false;
    }
    if !valid_lanes(u32::try_from(indices.len()).unwrap_or_default()) {
        report(
            diagnostics,
            instruction,
            "vec.shuffle result has an invalid lane count",
        );
        return false;
    }
    true
}

pub(super) fn verify_step(
    source: &str,
    instruction: &TextInstruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let target = explicit_type(source, instruction);
    if target
        .as_ref()
        .and_then(vector_element)
        .is_none_or(|element| !matches!(element, Type::Integer(_)))
    {
        report(
            diagnostics,
            instruction,
            "vec.step requires a vector of integers",
        );
        return false;
    }
    true
}

pub(super) fn verify_reduce(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(vector) = operands.first().copied() else {
        report(
            diagnostics,
            instruction,
            "vec.reduce requires a vector operand",
        );
        return false;
    };
    let Some(element) = vector_element(vector) else {
        report(
            diagnostics,
            instruction,
            "vec.reduce requires a vector operand",
        );
        return false;
    };
    if operands.len() == 2 && !mask_matches(operands[1], vector) {
        report(
            diagnostics,
            instruction,
            "vec.reduce mask must match the vector lane count",
        );
    }
    if clause_count(instruction, "mask") != usize::from(operands.len() == 2) {
        report(
            diagnostics,
            instruction,
            "vec.reduce mask clause does not match its operands",
        );
    }
    let operation = operation_atom(instruction);
    let integer = matches!(element, Type::Integer(_));
    let float = matches!(element, Type::Float(_));
    let valid = match operation {
        Some("fadd" | "fmul" | "fmin" | "fmax" | "fminnum" | "fmaxnum") => float,
        Some("add" | "mul" | "and" | "or" | "xor" | "smin" | "smax" | "umin" | "umax") => integer,
        _ => false,
    };
    let reassoc = leading_modifier_atoms(instruction).contains(&"reassoc");
    if !valid || reassoc && !matches!(operation, Some("fadd" | "fmul")) {
        report(
            diagnostics,
            instruction,
            "vec.reduce operation or reassoc modifier is incompatible with its element type",
        );
        return false;
    }
    true
}

pub(super) fn verify_vector_and_mask(
    instruction: &TextInstruction,
    operands: &[&Type],
    expected: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if operands.len() != expected {
        report(
            diagnostics,
            instruction,
            "vector mask operation has the wrong number of operands",
        );
        return false;
    }
    let vector = operands[0];
    if vector_element(vector).is_none()
        || operands[2] != vector
        || !mask_matches(operands[1], vector)
    {
        report(
            diagnostics,
            instruction,
            "vector operation requires matching vector values and lane mask",
        );
        return false;
    }
    true
}

pub(super) fn verify_dot(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if operands.len() != 3 {
        report(
            diagnostics,
            instruction,
            "vec.dot requires three vector operands",
        );
        return false;
    }
    let kind = operation_atom(instruction);
    let (input, accumulator, factor) = match kind {
        Some("sdot" | "udot" | "usdot") => (Type::Integer(8), Type::Integer(32), 4),
        Some("bfdot") => (Type::Float(FloatType::Bf16), Type::Float(FloatType::F32), 2),
        _ => {
            report(
                diagnostics,
                instruction,
                "vec.dot requires a supported dot-product kind",
            );
            return false;
        }
    };
    let (
        Some((acc_lanes, acc_element)),
        Some((left_lanes, left_element)),
        Some((right_lanes, right_element)),
    ) = (
        fixed_vector(operands[0]),
        fixed_vector(operands[1]),
        fixed_vector(operands[2]),
    )
    else {
        report(
            diagnostics,
            instruction,
            "vec.dot operands must be fixed vectors",
        );
        return false;
    };
    if acc_element != &accumulator
        || left_element != &input
        || right_element != &input
        || left_lanes != acc_lanes.saturating_mul(factor)
        || right_lanes != left_lanes
    {
        report(
            diagnostics,
            instruction,
            "vec.dot operands have incompatible element types or lane counts",
        );
        return false;
    }
    true
}

pub(super) fn verify_two_vectors(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if operands.len() != 2 || vector_element(operands[0]).is_none() || operands[1] != operands[0] {
        report(
            diagnostics,
            instruction,
            "vector operands must have the same vector type",
        );
        return false;
    }
    true
}

pub(super) fn verify_reverse(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if operands.len() != 1 || operands.first().and_then(|ty| vector_element(ty)).is_none() {
        report(
            diagnostics,
            instruction,
            "vec.reverse requires one vector operand",
        );
        return false;
    }
    true
}

pub(super) fn verify_table(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if operands.len() != 2 {
        report(
            diagnostics,
            instruction,
            "vec.table requires a table and index vector",
        );
        return false;
    }
    if vector_element(operands[0]) != Some(&Type::Integer(8)) || !is_integer_vector(operands[1]) {
        report(
            diagnostics,
            instruction,
            "vec.table requires an i8 table and integer indices",
        );
        return false;
    }
    true
}

pub(super) fn verify_optional_choice(
    instruction: &TextInstruction,
    choices: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let start = text_instruction_operands_start(&instruction.tokens);
    let last_value_end = instruction.tokens[start..]
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| is_punctuation(&pair[0], '%') && atom(&pair[1]).is_some())
        .map(|(index, _)| start + index + 2)
        .last();
    let Some(last_value_end) = last_value_end else {
        return true;
    };
    let tail = instruction.tokens[last_value_end..]
        .iter()
        .take_while(|token| {
            !matches!(
                token.kind,
                TokenKind::Punctuation('!') | TokenKind::Newline | TokenKind::End
            )
        })
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Punctuation(',') | TokenKind::Newline | TokenKind::End
            )
        })
        .collect::<Vec<_>>();
    if tail.is_empty()
        || tail.len() == 3
            && is_punctuation(tail[0], '[')
            && atom(tail[1]).is_some_and(|choice| choices.contains(&choice))
            && is_punctuation(tail[2], ']')
    {
        true
    } else {
        report(
            diagnostics,
            instruction,
            format!(
                "{} accepts an optional selector from {}",
                instruction.opcode,
                choices.join(" or ")
            ),
        );
        false
    }
}
