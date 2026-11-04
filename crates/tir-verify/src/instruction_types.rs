use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_types::Type;

use crate::Instruction;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify_instruction(instruction: &Instruction) -> Vec<Diagnostic> {
    if let Some(diagnostics) = crate::conversion_types::verify_instruction(instruction) {
        return diagnostics;
    }
    let mut diagnostics = Vec::new();
    let operands = instruction
        .operands
        .iter()
        .map(|operand| (&operand.ty, operand.span))
        .collect::<Vec<_>>();
    let results = instruction
        .results
        .iter()
        .map(|result| (&result.ty, result.span))
        .collect::<Vec<_>>();

    match instruction.opcode.as_str() {
        "const.int" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_integer_scalar,
            &mut diagnostics,
        ),
        "const.float" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_float_scalar,
            &mut diagnostics,
        ),
        "const.bool" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_i1,
            &mut diagnostics,
        ),
        "const.null" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            is_pointer,
            &mut diagnostics,
        ),
        "const.zero" | "const.bytes" | "const.aggregate" => {
            verify_constant(
                &instruction.opcode,
                &operands,
                &results,
                |_| true,
                &mut diagnostics,
            );
        }
        "const.poison" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |_| true,
            &mut diagnostics,
        ),
        "const.string" => {
            if require_counts(
                &instruction.opcode,
                &operands,
                &results,
                0,
                1,
                &mut diagnostics,
            ) {
                let expected = Type::Tuple(vec![
                    Type::Pointer(tir_types::AddressSpace::Generic),
                    Type::Integer(64),
                ]);
                if results[0].0 != &expected {
                    report(
                        &mut diagnostics,
                        results[0].1,
                        format!(
                            "const.string result has type {}; expected {expected}",
                            results[0].0
                        ),
                    );
                }
            }
        }
        "global.addr" | "tls.addr" | "descriptor.ref" | "witness.ref" | "block.addr" => {
            verify_constant(
                &instruction.opcode,
                &operands,
                &results,
                is_pointer,
                &mut diagnostics,
            );
        }
        "const.sampler" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Sampler),
            &mut diagnostics,
        ),
        "const.vec" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::FixedVector { .. }),
            &mut diagnostics,
        ),
        "add" | "sub" | "mul" | "sdiv" | "udiv" | "srem" | "urem" | "and" | "or" | "xor"
        | "shl" | "lshr" | "ashr" | "rotl" | "rotr" | "smin" | "smax" | "umin" | "umax"
        | "mulh" | "clmul" | "clmulh" => {
            verify_binary(
                instruction,
                &operands,
                &results,
                is_integer,
                &mut diagnostics,
            );
            if matches!(instruction.opcode.as_str(), "clmul" | "clmulh")
                && operands.first().is_some_and(|(ty, _)| {
                    integer_element_width(ty).is_some_and(|width| !matches!(width, 32 | 64))
                })
            {
                report(
                    &mut diagnostics,
                    instruction.span,
                    "clmul operands must be i32 or i64",
                );
            }
        }
        "add.ovf" | "sub.ovf" | "mul.ovf" => {
            verify_overflow_binary(instruction, &operands, &results, &mut diagnostics);
        }
        "fshl" | "fshr" => verify_binary_ternary(
            instruction,
            &operands,
            &results,
            is_integer,
            &mut diagnostics,
        ),
        "icmp" => verify_compare(
            instruction,
            &operands,
            &results,
            is_integer_or_pointer,
            &mut diagnostics,
        ),
        "fcmp" => verify_compare(instruction, &operands, &results, is_float, &mut diagnostics),
        "select" => verify_select(instruction, &operands, &results, &mut diagnostics),
        "not" | "neg" | "abs" | "ctlz" | "cttz" | "ctpop" | "bswap" | "bitreverse" => {
            verify_unary(
                instruction,
                &operands,
                &results,
                is_integer,
                &mut diagnostics,
            );
            if instruction.opcode == "bswap"
                && operands.first().is_some_and(|(ty, _)| {
                    integer_element_width(ty).is_some_and(|width| width % 16 != 0)
                })
            {
                report(
                    &mut diagnostics,
                    instruction.span,
                    "bswap requires an integer width divisible by 16",
                );
            }
        }
        "freeze" => verify_unary(instruction, &operands, &results, |_| true, &mut diagnostics),
        "fneg" | "fabs" | "fsqrt" | "ffloor" | "fceil" | "ftrunc" | "fround" | "froundeven" => {
            verify_unary(instruction, &operands, &results, is_float, &mut diagnostics)
        }
        "fadd" | "fsub" | "fmul" | "fdiv" | "frem" | "fmin" | "fmax" | "fminnum" | "fmaxnum"
        | "fcopysign" => {
            verify_binary(instruction, &operands, &results, is_float, &mut diagnostics);
        }
        "fma" | "fmuladd" => {
            verify_ternary(instruction, &operands, &results, is_float, &mut diagnostics)
        }
        "fclass" => verify_fclass(instruction, &operands, &results, &mut diagnostics),
        "frexp" => verify_frexp(instruction, &operands, &results, &mut diagnostics),
        "ldexp" => verify_ldexp(instruction, &operands, &results, &mut diagnostics),
        "fp.flags.read" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Integer(32)),
            &mut diagnostics,
        ),
        "fp.flags.clear" => verify_constant(
            &instruction.opcode,
            &operands,
            &results,
            |ty| matches!(ty, Type::Unit),
            &mut diagnostics,
        ),
        "math.atan2" | "math.pow" | "math.hypot" => {
            verify_binary(instruction, &operands, &results, is_float, &mut diagnostics);
        }
        "math.sin" | "math.cos" | "math.tan" | "math.asin" | "math.acos" | "math.atan"
        | "math.sinh" | "math.cosh" | "math.tanh" | "math.asinh" | "math.acosh" | "math.atanh"
        | "math.exp" | "math.exp2" | "math.exp10" | "math.expm1" | "math.log" | "math.log2"
        | "math.log10" | "math.log1p" | "math.cbrt" | "math.rsqrt" => {
            verify_unary(instruction, &operands, &results, is_float, &mut diagnostics)
        }
        _ => {}
    }
    diagnostics
}

fn verify_constant(
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

fn verify_unary(
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

fn verify_binary(
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

fn verify_binary_ternary(
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

fn verify_ternary(
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

fn verify_overflow_binary(
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

fn verify_compare(
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

fn verify_select(
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

fn verify_fclass(
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

fn verify_frexp(
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

fn verify_ldexp(
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

fn require_counts(
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

fn require_equal(diagnostics: &mut Vec<Diagnostic>, span: Span, opcode: &str, types: &[&Type]) {
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

fn is_i1(ty: &Type) -> bool {
    matches!(ty, Type::Integer(1))
}

fn is_integer_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

fn is_float_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Float(_))
}

fn is_integer(ty: &Type) -> bool {
    match ty {
        Type::Integer(_) => true,
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            matches!(element.as_ref(), Type::Integer(_))
        }
        _ => false,
    }
}

fn is_float(ty: &Type) -> bool {
    match ty {
        Type::Float(_) => true,
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            matches!(element.as_ref(), Type::Float(_))
        }
        _ => false,
    }
}

fn is_pointer(ty: &Type) -> bool {
    matches!(ty, Type::Pointer(_))
}

fn is_integer_or_pointer(ty: &Type) -> bool {
    is_integer(ty) || is_pointer(ty)
}

fn mask_type(ty: &Type) -> Type {
    match ty {
        Type::FixedVector { lanes, .. } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(1)),
        },
        Type::ScalableVector(_) => Type::ScalableVector(Box::new(Type::Integer(1))),
        _ => Type::Integer(1),
    }
}

fn integer_element_width(ty: &Type) -> Option<u16> {
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

fn report(diagnostics: &mut Vec<Diagnostic>, span: Span, message: impl Into<String>) {
    diagnostics.push(Diagnostic::error(V2, message).with_primary_span(span));
}
