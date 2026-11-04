use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_types::Type;

use crate::vector_inference::{explicit_type, type_after_atom};

#[path = "vector_types/operations.rs"]
mod operations;

use operations::*;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<Diagnostic> {
    let opcode = instruction.opcode.as_str();
    let mut diagnostics = Vec::new();
    if !valid_operand_count(opcode, operands.len()) {
        push(
            &mut diagnostics,
            instruction,
            "vector instruction has the wrong number of operands",
        );
    }
    match opcode {
        "vec.splat" => {
            let target = type_after_atom(source, instruction, "to");
            if let Some(target) = target {
                if let Some(element) = vector_element(&target) {
                    if operands.last().is_some_and(|value| value != &element) {
                        push(
                            &mut diagnostics,
                            instruction,
                            "vec.splat value must match the vector element type",
                        );
                    }
                } else {
                    push(
                        &mut diagnostics,
                        instruction,
                        "vec.splat target must be a vector type",
                    );
                }
            }
        }
        "vec.extract" => {
            if operands.first().and_then(vector_element).is_none() {
                push(
                    &mut diagnostics,
                    instruction,
                    "vec.extract requires a vector operand",
                );
            }
            if operands
                .get(1)
                .is_some_and(|index| !is_integer_scalar(index))
            {
                push(
                    &mut diagnostics,
                    instruction,
                    "vec.extract index must be an integer",
                );
            }
        }
        "vec.insert" => {
            if let Some(vector) = operands.first() {
                if let Some(element) = vector_element(vector) {
                    if operands.last().is_some_and(|value| value != &element) {
                        push(
                            &mut diagnostics,
                            instruction,
                            "vec.insert value must match the vector element type",
                        );
                    }
                } else {
                    push(
                        &mut diagnostics,
                        instruction,
                        "vec.insert requires a vector operand",
                    );
                }
            }
            if operands.len() >= 3 && !is_integer_scalar(&operands[1]) {
                push(
                    &mut diagnostics,
                    instruction,
                    "vec.insert index must be an integer",
                );
            }
        }
        "vec.shuffle" => verify_shuffle(instruction, operands, &mut diagnostics),
        "vec.step" => {
            if explicit_type(source, instruction)
                .as_ref()
                .and_then(vector_element)
                .is_none_or(|element| !matches!(element, Type::Integer(_)))
            {
                push(
                    &mut diagnostics,
                    instruction,
                    "vec.step requires a vector of integers",
                );
            }
        }
        "vec.reduce" => verify_reduce(instruction, operands, &mut diagnostics),
        "vec.load" => verify_load(source, instruction, operands, &mut diagnostics),
        "vec.store" => verify_store(instruction, operands, &mut diagnostics),
        "vec.gather" => verify_gather(source, instruction, operands, &mut diagnostics),
        "vec.scatter" => verify_scatter(instruction, operands, &mut diagnostics),
        "vec.compress" | "vec.expand" => {
            verify_vector_and_mask(instruction, operands, 3, &mut diagnostics)
        }
        "vec.dot" => verify_dot(instruction, operands, &mut diagnostics),
        "vec.zip" | "vec.unzip" => {
            verify_two_vectors(instruction, operands, &mut diagnostics);
            let choices = if opcode == "vec.zip" {
                &["lo", "hi"][..]
            } else {
                &["even", "odd"][..]
            };
            verify_optional_choice(instruction, choices, &mut diagnostics);
        }
        "vec.reverse" => require_vector(instruction, operands.first(), &mut diagnostics),
        "vec.slide" => {
            verify_two_vectors(instruction, operands, &mut diagnostics);
            verify_optional_choice(instruction, &["up", "down"], &mut diagnostics);
            if operands
                .get(2)
                .is_some_and(|index| !is_integer_scalar(index))
            {
                push(
                    &mut diagnostics,
                    instruction,
                    "vec.slide index must be an integer",
                );
            }
        }
        "vec.concat" => {
            if let (Some(left), Some(right)) = (operands.first(), operands.get(1)) {
                if concatenate_type(left, right).is_none() {
                    push(
                        &mut diagnostics,
                        instruction,
                        "vec.concat requires compatible fixed vectors with a valid lane count",
                    );
                }
            }
        }
        "vec.split" => {
            if operands.first().and_then(split_type).is_none() {
                push(
                    &mut diagnostics,
                    instruction,
                    "vec.split requires a fixed vector with an even lane count",
                );
            }
            verify_optional_choice(instruction, &["lo", "hi"], &mut diagnostics);
        }
        "vec.mask.active" => {
            if operands.iter().any(|operand| !is_integer_scalar(operand)) {
                push(
                    &mut diagnostics,
                    instruction,
                    "vec.mask.active requires integer operands",
                );
            }
        }
        "vec.mask.count" | "vec.mask.first" => {
            if operands.first().is_none_or(|mask| !is_mask(mask)) {
                push(
                    &mut diagnostics,
                    instruction,
                    "vector mask query requires a vector of i1",
                );
            }
        }
        "vec.table" => verify_table(instruction, operands, &mut diagnostics),
        _ => {}
    }
    diagnostics
}

pub(super) fn concatenate_type(left: &Type, right: &Type) -> Option<Type> {
    let (
        Type::FixedVector {
            lanes: left_lanes,
            element,
        },
        Type::FixedVector {
            lanes: right_lanes,
            element: right_element,
        },
    ) = (left, right)
    else {
        return None;
    };
    if element != right_element {
        return None;
    }
    let lanes = left_lanes.checked_add(*right_lanes)?;
    valid_lanes(lanes).then(|| Type::FixedVector {
        lanes,
        element: element.clone(),
    })
}

pub(super) fn split_type(vector: &Type) -> Option<Type> {
    let Type::FixedVector { lanes, element } = vector else {
        return None;
    };
    if lanes % 2 != 0 {
        return None;
    }
    let lanes = lanes / 2;
    valid_lanes(lanes).then(|| Type::FixedVector {
        lanes,
        element: element.clone(),
    })
}

pub(super) fn table_result_type(indices: &Type) -> Option<Type> {
    match indices {
        Type::FixedVector { lanes, .. } => Some(Type::FixedVector {
            lanes: *lanes,
            element: Box::new(Type::Integer(8)),
        }),
        Type::ScalableVector(_) => Some(Type::ScalableVector(Box::new(Type::Integer(8)))),
        _ => None,
    }
}

pub(super) fn vector_element(vector: &Type) -> Option<Type> {
    match vector {
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            Some((**element).clone())
        }
        _ => None,
    }
}

fn fixed_lanes(vector: &Type) -> Option<u32> {
    match vector {
        Type::FixedVector { lanes, .. } => Some(*lanes),
        _ => None,
    }
}

fn same_lanes(left: &Type, right: &Type) -> bool {
    match (left, right) {
        (Type::FixedVector { lanes: a, .. }, Type::FixedVector { lanes: b, .. }) => a == b,
        (Type::ScalableVector(_), Type::ScalableVector(_)) => true,
        _ => false,
    }
}

fn mask_matches(mask: &Type, vector: &Type) -> bool {
    same_lanes(mask, vector) && vector_element(mask) == Some(Type::Integer(1))
}

fn is_mask(ty: &Type) -> bool {
    vector_element(ty) == Some(Type::Integer(1))
}

fn is_integer_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_))
}

fn is_integer_vector(ty: &Type) -> bool {
    matches!(vector_element(ty), Some(Type::Integer(_)))
}

fn is_pointer_vector(ty: &Type) -> bool {
    matches!(vector_element(ty), Some(Type::Pointer(_)))
}

fn valid_operand_count(opcode: &str, count: usize) -> bool {
    match opcode {
        "vec.step" | "vec.vscale" => count == 0,
        "vec.splat" | "vec.reverse" | "vec.split" | "vec.mask.count" | "vec.mask.first" => {
            count == 1
        }
        "vec.extract" | "vec.shuffle" | "vec.zip" | "vec.unzip" | "vec.concat"
        | "vec.mask.active" | "vec.table" => count == 2,
        "vec.insert" | "vec.compress" | "vec.expand" | "vec.dot" | "vec.slide" => count == 3,
        "vec.reduce" => matches!(count, 1 | 2),
        "vec.load" => matches!(count, 1 | 3),
        "vec.store" => matches!(count, 2 | 3),
        "vec.scatter" => matches!(count, 2..=4),
        "vec.gather" => matches!(count, 1..=4),
        _ => true,
    }
}

fn require_vector(
    instruction: &TextInstruction,
    ty: Option<&Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if ty.and_then(vector_element).is_none() {
        push(
            diagnostics,
            instruction,
            "instruction requires a vector operand",
        );
    }
}

pub(super) fn valid_lanes(lanes: u32) -> bool {
    matches!(lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64)
}

fn push(
    diagnostics: &mut Vec<Diagnostic>,
    instruction: &TextInstruction,
    message: impl Into<String>,
) {
    diagnostics.push(Diagnostic::error(VFY3, message.into()).with_primary_span(instruction.span));
}
