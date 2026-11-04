use super::*;

pub(super) fn verify_load(
    source: &str,
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(vector) = explicit_type(source, instruction) else {
        report(
            diagnostics,
            instruction,
            "vec.load requires an explicit vector type",
        );
        return false;
    };
    let Some(element) = vector_element(&vector) else {
        report(diagnostics, instruction, "vec.load type must be a vector");
        return false;
    };
    if operands.is_empty() || !is_pointer(operands[0]) {
        report(
            diagnostics,
            instruction,
            "vec.load address must be a pointer",
        );
        return false;
    }
    if let Type::Address { pointee, .. } = operands[0]
        && pointee.as_ref() != element
    {
        report(
            diagnostics,
            instruction,
            "vec.load address element type must match the vector",
        );
    }
    verify_optional_alignment(instruction, diagnostics);
    verify_mask_clause(instruction, operands.len() == 3, true, diagnostics);
    if operands.len() == 3 && (!mask_matches(operands[1], &vector) || operands[2] != &vector) {
        report(
            diagnostics,
            instruction,
            "vec.load mask and passthru must match the result vector",
        );
    }
    true
}

pub(super) fn verify_store(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let (Some(vector), Some(pointer)) = (operands.first().copied(), operands.get(1).copied())
    else {
        report(
            diagnostics,
            instruction,
            "vec.store requires a vector and pointer",
        );
        return false;
    };
    let Some(element) = vector_element(vector) else {
        report(diagnostics, instruction, "vec.store value must be a vector");
        return false;
    };
    if !is_pointer(pointer) {
        report(
            diagnostics,
            instruction,
            "vec.store address must be a pointer",
        );
        return false;
    }
    if let Type::Address { pointee, .. } = pointer
        && pointee.as_ref() != element
    {
        report(
            diagnostics,
            instruction,
            "vec.store address element type must match the value",
        );
    }
    verify_optional_alignment(instruction, diagnostics);
    verify_mask_clause(instruction, operands.len() == 3, false, diagnostics);
    if operands.len() == 3 && !mask_matches(operands[2], vector) {
        report(
            diagnostics,
            instruction,
            "vec.store mask must match the value lane count",
        );
    }
    true
}

pub(super) fn verify_gather(
    source: &str,
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(vector) = explicit_type(source, instruction) else {
        report(
            diagnostics,
            instruction,
            "vec.gather requires an explicit vector type",
        );
        return false;
    };
    if vector_element(&vector).is_none() {
        report(diagnostics, instruction, "vec.gather type must be a vector");
        return false;
    }
    let Some(address) = operands.first().copied() else {
        report(
            diagnostics,
            instruction,
            "vec.gather requires an address operand",
        );
        return false;
    };
    let mask = has_mask_clause(instruction);
    let passthru = has_passthru_clause(instruction);
    verify_mask_clause(instruction, mask, true, diagnostics);
    if is_pointer_vector(address) {
        if !same_lanes(address, &vector) || operands.len() != 1 + usize::from(mask) * 2 {
            report(
                diagnostics,
                instruction,
                "vec.gather pointer vector must match the result lanes and optional mask operands",
            );
            return false;
        }
        verify_scale_clause(instruction, false, diagnostics);
        if mask && (!mask_matches(operands[1], &vector) || operands[2] != &vector) {
            report(
                diagnostics,
                instruction,
                "vec.gather mask and passthru must match the result vector",
            );
        }
        if passthru != mask {
            report(
                diagnostics,
                instruction,
                "masked vec.gather requires both mask and passthru",
            );
        }
        return true;
    }
    if !is_pointer(address) || operands.len() != 2 + usize::from(mask) * 2 {
        report(
            diagnostics,
            instruction,
            "vec.gather base form requires a pointer, offsets vector and optional mask operands",
        );
        return false;
    }
    if !is_integer_vector(operands[1]) || !same_lanes(operands[1], &vector) {
        report(
            diagnostics,
            instruction,
            "vec.gather offsets must be integer lanes matching the result",
        );
    }
    verify_scale_clause(instruction, true, diagnostics);
    if mask && (!mask_matches(operands[2], &vector) || operands[3] != &vector) {
        report(
            diagnostics,
            instruction,
            "vec.gather mask and passthru must match the result vector",
        );
    }
    if passthru != mask {
        report(
            diagnostics,
            instruction,
            "masked vec.gather requires both mask and passthru",
        );
    }
    true
}

pub(super) fn verify_scatter(
    instruction: &TextInstruction,
    operands: &[&Type],
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let (Some(vector), Some(address)) = (operands.first().copied(), operands.get(1).copied())
    else {
        report(
            diagnostics,
            instruction,
            "vec.scatter requires a vector and address",
        );
        return false;
    };
    if vector_element(vector).is_none() {
        report(
            diagnostics,
            instruction,
            "vec.scatter value must be a vector",
        );
        return false;
    }
    let mask = has_mask_clause(instruction);
    verify_mask_clause(instruction, mask, false, diagnostics);
    let mask_index = if is_pointer_vector(address) {
        if !same_lanes(vector, address) || operands.len() != 2 + usize::from(mask) {
            report(
                diagnostics,
                instruction,
                "vec.scatter pointer vector must match value lanes and optional mask",
            );
            return false;
        }
        verify_scale_clause(instruction, false, diagnostics);
        2
    } else {
        if !is_pointer(address)
            || operands.len() != 3 + usize::from(mask)
            || !is_integer_vector(operands[2])
            || !same_lanes(vector, operands[2])
        {
            report(
                diagnostics,
                instruction,
                "vec.scatter base form requires a pointer and matching integer offsets",
            );
            return false;
        }
        verify_scale_clause(instruction, true, diagnostics);
        3
    };
    if mask && !mask_matches(operands[mask_index], vector) {
        report(
            diagnostics,
            instruction,
            "vec.scatter mask must match value lanes",
        );
    }
    true
}

fn verify_optional_alignment(instruction: &TextInstruction, diagnostics: &mut Vec<Diagnostic>) {
    let mut alignments = Vec::new();
    for (index, token) in instruction.tokens.iter().enumerate() {
        if atom(token) != Some("align")
            || index > 0 && is_punctuation(&instruction.tokens[index - 1], '%')
        {
            continue;
        }
        let Some(value) = instruction
            .tokens
            .get(index + 1)
            .and_then(atom)
            .and_then(tir_format::numeric_literals::parse_unsigned_integer)
            .and_then(|value| u64::try_from(value).ok())
        else {
            report(
                diagnostics,
                instruction,
                "vector alignment must be an unsigned integer",
            );
            continue;
        };
        alignments.push(value);
    }
    if alignments.len() > 1
        || alignments
            .iter()
            .any(|value| *value != 0 && !value.is_power_of_two())
    {
        report(
            diagnostics,
            instruction,
            "vector alignment must be zero or a single power of two",
        );
    }
}

fn verify_mask_clause(
    instruction: &TextInstruction,
    expected: bool,
    passthru: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mask_count = clause_count(instruction, "mask");
    let passthru_count = clause_count(instruction, "passthru");
    if mask_count != usize::from(expected) || passthru_count != usize::from(expected && passthru) {
        report(
            diagnostics,
            instruction,
            "vector mask and passthru clauses do not match operands",
        );
    }
}

fn verify_scale_clause(
    instruction: &TextInstruction,
    expected: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let tokens = &instruction.tokens;
    let start = text_instruction_operands_start(tokens);
    let mask_start = tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| {
            (is_punctuation(token, '[') && tokens.get(index + 1).and_then(atom) == Some("mask"))
                .then_some(index)
        });
    let end = mask_start.unwrap_or_else(|| {
        tokens[start..]
            .iter()
            .position(|token| {
                matches!(
                    token.kind,
                    TokenKind::Punctuation('!') | TokenKind::Newline | TokenKind::End
                )
            })
            .map_or(tokens.len(), |index| start + index)
    });
    let last_value_end = tokens[start..end]
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| is_punctuation(&pair[0], '%') && atom(&pair[1]).is_some())
        .map(|(index, _)| start + index + 2)
        .last()
        .unwrap_or(end);
    let trailing = tokens[last_value_end..end]
        .iter()
        .filter(|token| !is_punctuation(token, ','))
        .collect::<Vec<_>>();
    if expected {
        let valid = trailing.len() == 1
            && atom(trailing[0])
                .and_then(tir_format::numeric_literals::parse_unsigned_integer)
                .and_then(|value| u64::try_from(value).ok())
                .is_some();
        if !valid {
            report(
                diagnostics,
                instruction,
                "gather/scatter base form requires an unsigned scale",
            );
        }
    } else if !trailing.is_empty() {
        report(
            diagnostics,
            instruction,
            "pointer-vector gather/scatter form does not take a scale",
        );
    }
}
