use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_types::{AddressSpace, Type};

use crate::calls::opcode_end;
use crate::instruction_arity::operand_count;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_arity(instruction: &TextInstruction) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(id) = intrinsic_id(instruction) else {
        push(
            &mut diagnostics,
            instruction,
            "intrinsic requires a valid ID",
        );
        return diagnostics;
    };
    let Some((expected_operands, expected_results)) = arity(id) else {
        push(
            &mut diagnostics,
            instruction,
            format!("unknown intrinsic ID {id}"),
        );
        return diagnostics;
    };
    let actual_operands = operand_count(instruction);
    if actual_operands != expected_operands {
        push(
            &mut diagnostics,
            instruction,
            format!(
                "intrinsic {id} has {actual_operands} value operands; expected {expected_operands}"
            ),
        );
    }
    if instruction.results.len() != expected_results {
        push(
            &mut diagnostics,
            instruction,
            format!(
                "intrinsic {id} defines {} results; expected {expected_results}",
                instruction.results.len()
            ),
        );
    }
    diagnostics
}

pub(super) fn verify_types(instruction: &TextInstruction, operands: &[Type]) -> Vec<Diagnostic> {
    let Some(id) = intrinsic_id(instruction) else {
        return Vec::new();
    };
    let valid = match id {
        0 | 1 => {
            operands.first() == Some(&Type::Integer(32))
                && matches!(operands.get(1), Some(Type::Integer(8 | 16 | 32 | 64)))
        }
        2..=5 => operands.len() == 2 && is_aes_vector(&operands[0]) && operands[0] == operands[1],
        6 => operands.len() == 1 && operands.first().is_some_and(is_aes_vector),
        7..=9 => {
            operands.len() == 2
                && is_generic_pointer(&operands[0])
                && is_generic_pointer(&operands[1])
        }
        10 => operands.len() == 1 && operands.first().is_some_and(is_generic_pointer),
        11..=14 => operands.is_empty(),
        15 => {
            operands.len() == 3
                && operands.first() == Some(&Type::Integer(1))
                && operands.get(1) == operands.get(2)
        }
        16 => {
            operands.len() == 3
                && operands.first().is_some_and(is_generic_pointer)
                && operands.get(1).is_some_and(is_generic_pointer)
                && operands.get(2) == Some(&Type::Integer(64))
        }
        _ => false,
    };
    if valid {
        Vec::new()
    } else {
        vec![
            Diagnostic::error(
                VFY3,
                format!("intrinsic {id} operands do not match its signature"),
            )
            .with_primary_span(instruction.span),
        ]
    }
}

pub(super) fn infer_result_types(instruction: &TextInstruction, operands: &[Type]) -> Vec<Type> {
    match intrinsic_id(instruction) {
        Some(0 | 1) => vec![Type::Integer(32)],
        Some(2..=6) => operands.first().cloned().into_iter().collect(),
        Some(12 | 13) => vec![Type::Integer(64)],
        Some(15) => operands.get(1).cloned().into_iter().collect(),
        Some(16) => vec![Type::Integer(1)],
        _ => Vec::new(),
    }
}

fn intrinsic_id(instruction: &TextInstruction) -> Option<u16> {
    let start = opcode_end(instruction);
    let tir_format::text::TokenKind::Atom(value) = &instruction.tokens.get(start)?.kind else {
        return None;
    };
    u16::try_from(tir_format::numeric_literals::parse_unsigned_integer(value)?).ok()
}

fn arity(id: u16) -> Option<(usize, usize)> {
    match id {
        0..=5 => Some((2, 1)),
        6 => Some((1, 1)),
        7..=9 => Some((2, 0)),
        10..=11 | 14 => Some((0, 0)),
        12..=13 => Some((0, 1)),
        15..=16 => Some((3, 1)),
        _ => None,
    }
}

fn is_aes_vector(ty: &Type) -> bool {
    matches!(
        ty,
        Type::FixedVector {
            lanes: 16,
            element
        } if element.as_ref() == &Type::Integer(8)
    )
}

fn is_generic_pointer(ty: &Type) -> bool {
    matches!(ty, Type::Pointer(AddressSpace::Generic))
}

fn push(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span));
}
