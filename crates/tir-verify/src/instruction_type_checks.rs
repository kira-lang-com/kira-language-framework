use super::*;

pub(super) fn verify_closure_new(
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts("closure.new", operands, results, 2, 1, diagnostics) {
        return;
    }
    let matches = matches!(
        (operands[0].0, results[0].0),
        (Type::Function(code), Type::Closure(closure))
            if code == &closure.signature && !closure.borrowed
    );
    if !matches {
        report(
            diagnostics,
            operands[0].1,
            "closure.new code and result must have the same function signature and an owned closure type",
        );
    }
}

pub(super) fn verify_constant(
    opcode: &str,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    accepts: fn(&Type) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if require_counts(opcode, operands, results, 0, 1, diagnostics) {
        if !accepts(results[0].0) {
            report(
                diagnostics,
                results[0].1,
                format!("{opcode} result has invalid type {}", results[0].0),
            );
        }
    }
}

pub(super) fn verify_unary(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    accepts: fn(&Type) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 1, 1, diagnostics) {
        return;
    }
    let (operand_type, operand_span) = operands[0];
    if !accepts(operand_type) {
        report(
            diagnostics,
            operand_span,
            format!("{} requires a compatible operand type", instruction.opcode),
        );
    }
    require_equal(
        diagnostics,
        instruction.span,
        &instruction.opcode,
        &[operand_type, results[0].0],
    );
}

pub(super) fn verify_binary(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    accepts: fn(&Type) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 2, 1, diagnostics) {
        return;
    }
    let (left, left_span) = operands[0];
    let (right, right_span) = operands[1];
    if !accepts(left) {
        report(
            diagnostics,
            left_span,
            format!("{} requires a compatible operand type", instruction.opcode),
        );
    }
    require_equal(
        diagnostics,
        right_span,
        &instruction.opcode,
        &[left, right, results[0].0],
    );
}

pub(super) fn verify_binary_ternary(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    accepts: fn(&Type) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 3, 1, diagnostics) {
        return;
    }
    let (left, left_span) = operands[0];
    if !accepts(left) {
        report(
            diagnostics,
            left_span,
            format!("{} requires integer operands", instruction.opcode),
        );
    }
    require_equal(
        diagnostics,
        instruction.span,
        &instruction.opcode,
        &[left, operands[1].0, operands[2].0, results[0].0],
    );
}

pub(super) fn verify_ternary(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    accepts: fn(&Type) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 3, 1, diagnostics) {
        return;
    }
    let (first, first_span) = operands[0];
    if !accepts(first) {
        report(
            diagnostics,
            first_span,
            format!("{} requires floating-point operands", instruction.opcode),
        );
    }
    require_equal(
        diagnostics,
        instruction.span,
        &instruction.opcode,
        &[first, operands[1].0, operands[2].0, results[0].0],
    );
}

pub(super) fn verify_overflow_binary(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 2, 1, diagnostics) {
        return;
    }
    let (input_type, input_span) = operands[0];
    if !is_integer(input_type) {
        report(
            diagnostics,
            input_span,
            "overflow arithmetic requires integer operands",
        );
    }
    require_equal(
        diagnostics,
        instruction.span,
        &instruction.opcode,
        &[input_type, operands[1].0, results[0].0],
    );
    let expected_result = Type::Tuple(vec![input_type.clone(), mask_type(input_type)]);
    if results[0].0 != &expected_result {
        report(
            diagnostics,
            results[0].1,
            format!(
                "{} result has type {}, expected {}",
                instruction.opcode, results[0].0, expected_result
            ),
        );
    }
}

pub(super) fn verify_compare(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    accepts: fn(&Type) -> bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 2, 1, diagnostics) {
        return;
    }
    let (left, left_span) = operands[0];
    if !accepts(left) {
        report(
            diagnostics,
            left_span,
            format!(
                "{} requires compatible comparison operands",
                instruction.opcode
            ),
        );
    }
    require_equal(
        diagnostics,
        operands[1].1,
        &instruction.opcode,
        &[left, operands[1].0],
    );
    let expected_result = mask_type(left);
    if results[0].0 != &expected_result {
        report(
            diagnostics,
            results[0].1,
            format!(
                "{} result has type {}, expected {}",
                instruction.opcode, results[0].0, expected_result
            ),
        );
    }
}

pub(super) fn verify_select(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 3, 1, diagnostics) {
        return;
    }
    require_equal(
        diagnostics,
        instruction.span,
        &instruction.opcode,
        &[operands[1].0, operands[2].0, results[0].0],
    );
    let expected_condition = mask_type(operands[1].0);
    if operands[0].0 != &expected_condition {
        report(
            diagnostics,
            operands[0].1,
            format!(
                "select condition has type {}, expected {}",
                operands[0].0, expected_condition
            ),
        );
    }
}

pub(super) fn verify_fclass(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 1, 1, diagnostics) {
        return;
    }
    let (input, input_span) = operands[0];
    if !is_float(input) {
        report(
            diagnostics,
            input_span,
            "fclass requires a floating-point operand",
        );
        return;
    }
    let expected = match input {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(32)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(32))),
        _ => Type::Integer(32),
    };
    if results[0].0 != &expected {
        report(
            diagnostics,
            results[0].1,
            format!(
                "fclass result has type {}, expected {expected}",
                results[0].0
            ),
        );
    }
}

pub(super) fn verify_frexp(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 1, 1, diagnostics) {
        return;
    }
    let (input, input_span) = operands[0];
    if !is_float(input) {
        report(
            diagnostics,
            input_span,
            "frexp requires a floating-point operand",
        );
        return;
    }
    let exponent = match input {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(32)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(32))),
        _ => Type::Integer(32),
    };
    let expected = Type::Tuple(vec![input.clone(), exponent]);
    if results[0].0 != &expected {
        report(
            diagnostics,
            results[0].1,
            format!(
                "frexp result has type {}, expected {expected}",
                results[0].0
            ),
        );
    }
}

pub(super) fn verify_ldexp(
    instruction: &Instruction,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require_counts(&instruction.opcode, operands, results, 2, 1, diagnostics) {
        return;
    }
    let (input, input_span) = operands[0];
    if !is_float(input) {
        report(
            diagnostics,
            input_span,
            "ldexp requires a floating-point operand",
        );
        return;
    }
    require_equal(
        diagnostics,
        instruction.span,
        "ldexp",
        &[input, results[0].0],
    );
    let expected_exponent = match input {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(32)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(32))),
        _ => Type::Integer(32),
    };
    if operands[1].0 != &expected_exponent {
        report(
            diagnostics,
            operands[1].1,
            format!(
                "ldexp exponent has type {}, expected {expected_exponent}",
                operands[1].0
            ),
        );
    }
}

pub(super) fn require_counts(
    opcode: &str,
    operands: &[(&Type, Span)],
    results: &[(&Type, Span)],
    expected_operands: usize,
    expected_results: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let mut valid = true;
    if operands.len() != expected_operands {
        report(
            diagnostics,
            operands
                .first()
                .map_or(Span::new(0, 0), |operand| operand.1),
            format!(
                "{opcode} has {} operands; expected {expected_operands}",
                operands.len()
            ),
        );
        valid = false;
    }
    if results.len() != expected_results {
        report(
            diagnostics,
            results.first().map_or(Span::new(0, 0), |result| result.1),
            format!(
                "{opcode} defines {} results; expected {expected_results}",
                results.len()
            ),
        );
        valid = false;
    }
    valid
}

pub(super) fn require_equal(
    diagnostics: &mut Vec<Diagnostic>,
    span: Span,
    opcode: &str,
    types: &[&Type],
) {
    if let Some(expected) = types.first()
        && let Some(actual) = types.iter().copied().find(|actual| *actual != *expected)
    {
        report(
            diagnostics,
            span,
            format!("{opcode} operand/result type {actual} does not match {expected}"),
        );
    }
}

pub(super) fn is_i1(ty: &Type) -> bool {
    matches!(ty, Type::Integer(1))
}

pub(super) fn is_integer_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

pub(super) fn is_float_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Float(_))
}

pub(super) fn is_integer(ty: &Type) -> bool {
    match ty {
        Type::Integer(_) => true,
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            matches!(element.as_ref(), Type::Integer(_))
        }
        _ => false,
    }
}

pub(super) fn is_float(ty: &Type) -> bool {
    match ty {
        Type::Float(_) => true,
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            matches!(element.as_ref(), Type::Float(_))
        }
        _ => false,
    }
}

pub(super) fn is_pointer(ty: &Type) -> bool {
    matches!(ty, Type::Pointer(_))
}

pub(super) fn is_integer_or_pointer(ty: &Type) -> bool {
    is_integer(ty) || is_pointer(ty)
}

pub(super) fn mask_type(ty: &Type) -> Type {
    match ty {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(1)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(1))),
        _ => Type::Integer(1),
    }
}

pub(super) fn integer_element_width(ty: &Type) -> Option<u16> {
    match ty {
        Type::Integer(width) => Some(*width),
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => match element.as_ref()
        {
            Type::Integer(width) => Some(*width),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn report(diagnostics: &mut Vec<Diagnostic>, span: Span, message: impl Into<String>) {
    diagnostics.push(Diagnostic::error(V2, message).with_primary_span(span));
}
