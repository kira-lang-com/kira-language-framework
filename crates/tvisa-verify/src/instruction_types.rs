use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{TextInstruction, parse_tvisa_type};
use tir_types::Type;

use crate::calls::opcode_end;
use crate::instruction_arity::{operand_arity, operand_count, result_arity};
use crate::type_classes::{
    integer_element_width, is_float, is_integer, is_memory_type, is_pointer,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_instruction(
    source: &str,
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
    gpu_function: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let opcode = instruction.opcode.as_str();
    if let Some(expected) = result_arity(instruction)
        && instruction.results.len() != expected
    {
        push_error(
            &mut diagnostics,
            instruction,
            format!(
                "{} defines {} results; expected {expected}",
                opcode,
                instruction.results.len()
            ),
        );
    }
    if let Some(expected) = operand_arity(opcode)
        && operand_count(instruction) != expected
    {
        push_error(
            &mut diagnostics,
            instruction,
            format!(
                "{} has {} value operands; expected {expected}",
                opcode,
                operand_count(instruction)
            ),
        );
    }
    if opcode == "intrinsic" {
        diagnostics.extend(crate::intrinsic_types::verify_arity(instruction));
    }
    diagnostics.extend(crate::constant_types::verify(source, instruction));
    let Some(operands) = operand_types(instruction, value_types) else {
        return diagnostics;
    };
    if opcode == "intrinsic" {
        diagnostics.extend(crate::intrinsic_types::verify_types(instruction, &operands));
    }
    diagnostics.extend(crate::gpu_types::verify(source, instruction, &operands));

    if matches!(opcode, "clmul" | "clmulh") {
        require_same_numeric(instruction, &operands, 2, true, &mut diagnostics);
        if !matches!(operands.first(), Some(Type::Integer(32 | 64))) {
            push_error(
                &mut diagnostics,
                instruction,
                "clmul and clmulh require scalar i32 or i64 operands",
            );
        }
    } else if is_integer_binary(opcode) {
        require_same_numeric(instruction, &operands, 2, true, &mut diagnostics);
    } else if is_float_binary(opcode) {
        let count = if matches!(opcode, "fma" | "fmuladd") {
            3
        } else {
            2
        };
        require_same_numeric(instruction, &operands, count, false, &mut diagnostics);
    } else if is_float_unary(opcode) {
        require_numeric(instruction, &operands, 0, false, &mut diagnostics);
    } else if opcode == "icmp" {
        if let (Some(first), Some(second)) = (operands.first(), operands.get(1)) {
            if first != second {
                push_error(
                    &mut diagnostics,
                    instruction,
                    format!("icmp operands have different types {first} and {second}"),
                );
            }
            if !is_integer(first) && !matches!(first, Type::Pointer(_) | Type::Code) {
                push_error(
                    &mut diagnostics,
                    instruction,
                    format!("icmp operand has incompatible type {first}"),
                );
            }
            let valid_predicate = match first {
                Type::Code => is_comparison_predicate(instruction, &["eq", "ne"]),
                Type::Pointer(_) => {
                    is_comparison_predicate(instruction, &["eq", "ne", "ult", "ule", "ugt", "uge"])
                }
                _ => is_comparison_predicate(
                    instruction,
                    &[
                        "eq", "ne", "slt", "sle", "sgt", "sge", "ult", "ule", "ugt", "uge",
                    ],
                ),
            };
            if !valid_predicate {
                push_error(
                    &mut diagnostics,
                    instruction,
                    format!("icmp predicate is invalid for {first} operands"),
                );
            }
        }
    } else if opcode == "fcmp" {
        require_same_numeric(instruction, &operands, 2, false, &mut diagnostics);
        if !is_comparison_predicate(
            instruction,
            &[
                "oeq", "one", "olt", "ole", "ogt", "oge", "ord", "uno", "ueq", "une", "ult", "ule",
                "ugt", "uge", "true", "false",
            ],
        ) {
            push_error(
                &mut diagnostics,
                instruction,
                "fcmp has an invalid predicate",
            );
        }
    } else if opcode == "select" {
        if let Some(condition) = operands.first()
            && !matches!(condition, Type::Integer(1))
            && !mask_matches(condition, operands.get(1))
        {
            push_error(
                &mut diagnostics,
                instruction,
                format!("select condition has type {condition}; expected i1"),
            );
        }
        require_same_pair(instruction, &operands, 1, 2, &mut diagnostics);
    } else if matches!(opcode, "shl" | "lshr" | "ashr" | "rotl" | "rotr") {
        require_numeric(instruction, &operands, 0, true, &mut diagnostics);
        require_integer_operand(instruction, &operands, 1, &mut diagnostics);
    } else if matches!(opcode, "fshl" | "fshr") {
        require_same_numeric(instruction, &operands, 2, true, &mut diagnostics);
        require_integer_operand(instruction, &operands, 2, &mut diagnostics);
    } else if opcode == "bswap" {
        require_numeric(instruction, &operands, 0, true, &mut diagnostics);
        if let Some(width) = operands.first().and_then(integer_element_width)
            && width % 16 != 0
        {
            push_error(
                &mut diagnostics,
                instruction,
                "bswap requires integer elements with widths divisible by 16",
            );
        }
    } else if matches!(opcode, "ctlz" | "cttz" | "ctpop" | "bitreverse") {
        require_numeric(instruction, &operands, 0, true, &mut diagnostics);
    } else if matches!(opcode, "not" | "neg" | "abs") {
        require_numeric(instruction, &operands, 0, true, &mut diagnostics);
    } else if opcode == "ldexp" {
        require_numeric(instruction, &operands, 0, false, &mut diagnostics);
        if operands.get(1) != Some(&Type::Integer(32)) {
            push_error(
                &mut diagnostics,
                instruction,
                "ldexp exponent must have type i32",
            );
        }
    } else if opcode.starts_with("math.") {
        diagnostics.extend(crate::math_types::verify(instruction, &operands));
    } else if opcode.starts_with("vec.") {
        diagnostics.extend(crate::vector_types::verify(source, instruction, &operands));
    } else if opcode == "ptr.offset" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        require_integer_operand(instruction, &operands, 1, &mut diagnostics);
    } else if opcode == "ptr.diff" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        require_pointer(instruction, &operands, 1, &mut diagnostics);
    } else if opcode == "raw.load" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        verify_explicit_type(source, instruction, &mut diagnostics);
    } else if opcode == "raw.store" {
        require_pointer(instruction, &operands, 1, &mut diagnostics);
        if operands.first().is_some_and(|ty| !is_memory_type(ty)) {
            push_error(
                &mut diagnostics,
                instruction,
                "raw.store value is not a TVISA memory type",
            );
        }
    } else if opcode == "atomic.load" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        verify_explicit_type(source, instruction, &mut diagnostics);
        diagnostics.extend(crate::atomic_types::verify(
            instruction,
            &operands,
            explicit_type(source, instruction),
            gpu_function,
        ));
    } else if opcode == "atomic.store" {
        require_pointer(instruction, &operands, 1, &mut diagnostics);
        if operands.first().is_some_and(|ty| !is_memory_type(ty)) {
            push_error(
                &mut diagnostics,
                instruction,
                "atomic.store value is not a TVISA memory type",
            );
        }
        diagnostics.extend(crate::atomic_types::verify(
            instruction,
            &operands,
            None,
            gpu_function,
        ));
    } else if opcode == "atomic.rmw" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        if operands.get(1).is_some_and(|ty| !is_memory_type(ty)) {
            push_error(
                &mut diagnostics,
                instruction,
                "atomic.rmw value is not a TVISA memory type",
            );
        }
        diagnostics.extend(crate::atomic_types::verify(
            instruction,
            &operands,
            None,
            gpu_function,
        ));
    } else if opcode == "atomic.cmpxchg" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        require_same_pair(instruction, &operands, 1, 2, &mut diagnostics);
        if operands.get(1).is_some_and(|ty| !is_memory_type(ty)) {
            push_error(
                &mut diagnostics,
                instruction,
                "atomic.cmpxchg values are not a TVISA memory type",
            );
        }
        diagnostics.extend(crate::atomic_types::verify(
            instruction,
            &operands,
            None,
            gpu_function,
        ));
    } else if opcode == "fence" {
        diagnostics.extend(crate::atomic_types::verify(
            instruction,
            &operands,
            None,
            gpu_function,
        ));
    } else if opcode == "mem.copy" || opcode == "mem.move" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        require_pointer(instruction, &operands, 1, &mut diagnostics);
        require_integer_operand(instruction, &operands, 2, &mut diagnostics);
    } else if opcode == "mem.set" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        require_integer_operand(instruction, &operands, 1, &mut diagnostics);
        require_integer_operand(instruction, &operands, 2, &mut diagnostics);
    } else if opcode == "prefetch" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
    } else if opcode == "va.start" || opcode == "va.end" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
    } else if opcode == "va.copy" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        require_pointer(instruction, &operands, 1, &mut diagnostics);
    } else if opcode == "va.arg" {
        require_pointer(instruction, &operands, 1, &mut diagnostics);
    } else if opcode == "mmio.load" {
        require_pointer(instruction, &operands, 0, &mut diagnostics);
        if let Some(ty) = explicit_type(source, instruction) {
            verify_mmio_type(&ty, instruction, &mut diagnostics);
        }
    } else if opcode == "mmio.store" {
        require_pointer(instruction, &operands, 1, &mut diagnostics);
        if let Some(ty) = operands.first() {
            verify_mmio_type(ty, instruction, &mut diagnostics);
        }
    } else if opcode == "code.load" || opcode == "code.fromaddr" || opcode == "code.addr" {
        let valid = operands.first().is_some_and(|actual| {
            if opcode == "code.addr" {
                actual != &Type::Code
            } else {
                !matches!(actual, Type::Pointer(_))
            }
        });
        if valid {
            push_error(
                &mut diagnostics,
                instruction,
                format!("{opcode} operand has an incompatible type"),
            );
        }
    } else if opcode == "code.store" {
        if operands.first().is_some_and(|ty| ty != &Type::Code) {
            push_error(
                &mut diagnostics,
                instruction,
                "code.store requires a code value",
            );
        }
        require_pointer(instruction, &operands, 1, &mut diagnostics);
    } else if opcode == "trap.if" {
        if operands.first().is_some_and(|ty| ty != &Type::Integer(1)) {
            push_error(
                &mut diagnostics,
                instruction,
                "trap.if condition must have type i1",
            );
        }
    } else if opcode == "stack.alloc.dyn" {
        require_integer_operand(instruction, &operands, 0, &mut diagnostics);
    } else if matches!(
        opcode,
        "trunc"
            | "zext"
            | "sext"
            | "fptrunc"
            | "fpext"
            | "sitofp"
            | "uitofp"
            | "fptosi"
            | "fptoui"
            | "ptrtoint"
            | "inttoptr"
            | "bitcast"
            | "fcvt"
    ) {
        diagnostics.extend(crate::conversion_types::verify(
            source,
            instruction,
            &operands,
        ));
    }

    diagnostics
}

fn verify_explicit_type(
    source: &str,
    instruction: &TextInstruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(ty) = explicit_type(source, instruction)
        && !is_memory_type(&ty)
    {
        push_error(
            diagnostics,
            instruction,
            format!("raw.load type {ty} is not a TVISA memory type"),
        );
    }
}

fn verify_mmio_type(ty: &Type, instruction: &TextInstruction, diagnostics: &mut Vec<Diagnostic>) {
    let valid_size =
        crate::conversion_types::static_size(ty).is_some_and(|size| matches!(size, 1 | 2 | 4 | 8));
    if !is_memory_type(ty) || !valid_size {
        push_error(
            diagnostics,
            instruction,
            format!(
                "{} type {ty} must be a 1, 2, 4 or 8 byte memory type",
                instruction.opcode
            ),
        );
    }
}

pub(super) fn explicit_type(source: &str, instruction: &TextInstruction) -> Option<Type> {
    let tokens = &instruction.tokens;
    let start = if matches!(
        instruction.opcode.as_str(),
        "trunc"
            | "zext"
            | "sext"
            | "fptrunc"
            | "fpext"
            | "sitofp"
            | "uitofp"
            | "fptosi"
            | "fptoui"
            | "ptrtoint"
            | "inttoptr"
            | "bitcast"
            | "fcvt"
    ) {
        tokens.iter().position(|token| atom(token) == Some("to"))? + 1
    } else {
        opcode_end(instruction)
    };
    let first = tokens.get(start)?;
    let mut depth = if matches!(first.kind, TokenKind::Punctuation('[')) {
        1i32
    } else {
        0
    };
    for token in tokens.iter().skip(start + 1) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '<' | '(' => depth += 1,
                '[' if depth == 0 => {
                    return parse_tvisa_type(source.get(first.span.start..token.span.start)?).ok();
                }
                '[' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => {
                    return parse_tvisa_type(source.get(first.span.start..token.span.start)?).ok();
                }
                _ => {}
            }
        }
    }
    parse_tvisa_type(source.get(first.span.start..tokens.last()?.span.end)?).ok()
}

fn operand_types(
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Option<Vec<Type>> {
    let mut result = Vec::new();
    for pair in instruction.tokens[opcode_end(instruction)..].windows(2) {
        if !matches!(pair[0].kind, TokenKind::Punctuation('%')) {
            continue;
        }
        let name = atom(&pair[1])?;
        result.push(value_types.get(name)?.clone());
    }
    Some(result)
}

fn require_same_numeric(
    instruction: &TextInstruction,
    operands: &[Type],
    count: usize,
    integer: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(first) = operands.first() {
        let valid = if integer {
            is_integer(first)
        } else {
            is_float(first)
        };
        if !valid {
            push_error(
                diagnostics,
                instruction,
                format!(
                    "{} operand has incompatible type {first}",
                    instruction.opcode
                ),
            );
        }
        for operand in operands.iter().take(count).skip(1) {
            if operand != first {
                push_error(
                    diagnostics,
                    instruction,
                    format!(
                        "{} operands have different types {first} and {operand}",
                        instruction.opcode
                    ),
                );
                break;
            }
        }
    }
}

fn require_same_pair(
    instruction: &TextInstruction,
    operands: &[Type],
    first: usize,
    second: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let (Some(left), Some(right)) = (operands.get(first), operands.get(second))
        && left != right
    {
        push_error(
            diagnostics,
            instruction,
            format!(
                "{} values have different types {left} and {right}",
                instruction.opcode
            ),
        );
    }
}

fn require_numeric(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    integer: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(actual) = operands.get(index) else {
        return;
    };
    let valid = if integer {
        is_integer(actual)
    } else {
        is_float(actual)
    };
    if !valid {
        push_error(
            diagnostics,
            instruction,
            format!(
                "{} operand has incompatible type {actual}",
                instruction.opcode
            ),
        );
    }
}

fn require_integer_operand(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    require_numeric(instruction, operands, index, true, diagnostics);
}

fn require_pointer(
    instruction: &TextInstruction,
    operands: &[Type],
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if operands.get(index).is_some_and(|ty| !is_pointer(ty)) {
        push_error(
            diagnostics,
            instruction,
            format!("{} operand must have pointer type", instruction.opcode),
        );
    }
}

fn push_error(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span));
}

pub(super) fn is_integer_binary(opcode: &str) -> bool {
    [
        "add", "sub", "mul", "mulh", "sdiv", "udiv", "srem", "urem", "and", "or", "xor", "smin",
        "smax", "umin", "umax", "clmul", "clmulh",
    ]
    .iter()
    .any(|prefix| opcode == *prefix || opcode.starts_with(&format!("{prefix}.")))
}

pub(super) fn is_float_binary(opcode: &str) -> bool {
    [
        "fadd",
        "fsub",
        "fmul",
        "fdiv",
        "frem",
        "fmin",
        "fmax",
        "fminnum",
        "fmaxnum",
        "fcopysign",
        "fma",
        "fmuladd",
    ]
    .iter()
    .any(|prefix| opcode == *prefix || opcode.starts_with(&format!("{prefix}.")))
}

pub(super) fn is_float_unary(opcode: &str) -> bool {
    [
        "fneg",
        "fabs",
        "fsqrt",
        "ffloor",
        "fceil",
        "ftrunc",
        "fround",
        "froundeven",
        "fclass",
        "frexp",
    ]
    .iter()
    .any(|prefix| opcode == *prefix || opcode.starts_with(&format!("{prefix}.")))
}

fn is_comparison_predicate(instruction: &TextInstruction, expected: &[&str]) -> bool {
    let start = opcode_end(instruction);
    instruction.tokens[start..]
        .iter()
        .filter_map(atom)
        .any(|predicate| expected.contains(&predicate))
}

fn mask_matches(mask: &Type, value: Option<&Type>) -> bool {
    match (mask, value) {
        (
            Type::FixedVector { lanes, element },
            Some(Type::FixedVector {
                lanes: value_lanes, ..
            }),
        ) => lanes == value_lanes && matches!(element.as_ref(), Type::Integer(1)),
        (Type::ScalableVector(element), Some(Type::ScalableVector(_))) => {
            matches!(element.as_ref(), Type::Integer(1))
        }
        _ => false,
    }
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}
