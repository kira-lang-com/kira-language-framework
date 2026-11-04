use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryConstantTable, BinaryFunctionBody, BinaryGpuEntry, BinaryOperand, BinarySignatureTable,
    BinaryType, BinaryTypeTable,
};
use tir_types::FloatType;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

#[derive(Clone, Copy)]
struct Use {
    ty: u64,
    default: u64,
    function_index: usize,
    offset: u64,
}

pub(super) fn verify_entry(
    entry: &BinaryGpuEntry,
    reachable: &[bool],
    bodies: &[Option<&BinaryFunctionBody>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    constants: Option<&BinaryConstantTable>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let entry_name = usize::try_from(entry.function_index)
        .ok()
        .and_then(|index| signatures.records.get(index))
        .map_or("<unknown>", |signature| signature.name.as_str());
    let mut uses: HashMap<u32, Use> = HashMap::new();

    for (function_index, is_reachable) in reachable.iter().copied().enumerate() {
        if !is_reachable {
            continue;
        }
        let Some(body) = bodies.get(function_index).copied().flatten() else {
            continue;
        };
        for instruction in body
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter().flatten())
            .filter(|instruction| instruction.opcode == 0x401)
        {
            let Some((id, ty, default)) = specialization_use(instruction) else {
                continue;
            };
            if !default_has_type(constants, default, ty) {
                let function_name = signatures
                    .records
                    .get(function_index)
                    .map_or("<unknown>", |signature| signature.name.as_str());
                report(
                    diagnostics,
                    format!(
                        "GPU entry @{entry_name} uses specialization constant {id} with a default whose type does not match its CODE type in @{function_name} at CODE+0x{:x}",
                        instruction.offset
                    ),
                );
            }
            if !is_supported_type(types, ty) {
                report(
                    diagnostics,
                    format!(
                        "GPU entry @{entry_name} uses specialization constant {id} with an unsupported type"
                    ),
                );
            }
            let current = Use {
                ty,
                default,
                function_index,
                offset: instruction.offset,
            };
            if let Some(previous) = uses.get(&id).copied() {
                if !same_type(types, previous.ty, current.ty)
                    || !same_default(constants, previous.default, current.default)
                {
                    let function_name = signatures
                        .records
                        .get(function_index)
                        .map_or("<unknown>", |signature| signature.name.as_str());
                    report(
                        diagnostics,
                        format!(
                            "GPU entry @{entry_name} reaches specialization constant {id} with conflicting type or default in @{function_name} at CODE+0x{:x}",
                            instruction.offset
                        ),
                    );
                }
            } else {
                uses.insert(id, current);
            }
        }
    }

    for declared in &entry.specialization_constants {
        if !is_supported_type(types, declared.ty) {
            report(
                diagnostics,
                format!(
                    "GPU entry @{entry_name} declares specialization constant {} with an unsupported type",
                    declared.id
                ),
            );
        }
        let Some(used) = uses.get(&declared.id) else {
            report(
                diagnostics,
                format!(
                    "GPU entry @{entry_name} declares unused specialization constant {}",
                    declared.id
                ),
            );
            continue;
        };
        if !same_type(types, declared.ty, used.ty)
            || !same_default(constants, declared.default_constant, used.default)
        {
            report(
                diagnostics,
                format!(
                    "GPU entry @{entry_name} specialization constant {} disagrees with its CODE use",
                    declared.id
                ),
            );
        }
    }

    for (id, used) in uses {
        if entry
            .specialization_constants
            .iter()
            .all(|declared| declared.id != id)
        {
            let function_name = signatures
                .records
                .get(used.function_index)
                .map_or("<unknown>", |signature| signature.name.as_str());
            report(
                diagnostics,
                format!(
                    "GPU entry @{entry_name} omits specialization constant {id} used by @{function_name} at CODE+0x{:x}",
                    used.offset
                ),
            );
        }
    }
}

fn specialization_use(instruction: &tir_format::BinaryInstruction) -> Option<(u32, u64, u64)> {
    let mut indexes = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let id = u32::try_from(indexes.next()?).ok()?;
    let ty = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(ty) => Some(*ty),
            _ => None,
        })?;
    Some((id, ty, indexes.next()?))
}

fn is_supported_type(types: &BinaryTypeTable, ty: u64) -> bool {
    let Some(index) = usize::try_from(ty).ok() else {
        return false;
    };
    match types.entries.get(index) {
        Some(BinaryType::Integer(1..=64)) => true,
        Some(BinaryType::Float(
            FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64,
        )) => true,
        Some(BinaryType::FixedVector { lanes, element }) if (2..=4).contains(lanes) => {
            numeric_type(types, *element)
        }
        _ => false,
    }
}

fn numeric_type(types: &BinaryTypeTable, ty: u64) -> bool {
    let Some(index) = usize::try_from(ty).ok() else {
        return false;
    };
    matches!(types.entries.get(index), Some(BinaryType::Integer(1..=64)))
        || matches!(
            types.entries.get(index),
            Some(BinaryType::Float(
                FloatType::F16
                    | FloatType::Bf16
                    | FloatType::F32
                    | FloatType::F64
                    | FloatType::F8E4M3
                    | FloatType::F8E5M2
                    | FloatType::F6E2M3
                    | FloatType::F6E3M2
                    | FloatType::F4E2M1
                    | FloatType::E8M0
            ))
        )
}

fn same_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    let Some(left) = usize::try_from(left)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    let Some(right) = usize::try_from(right)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    match (left, right) {
        (BinaryType::Integer(left), BinaryType::Integer(right)) => left == right,
        (BinaryType::Float(left), BinaryType::Float(right)) => left == right,
        (
            BinaryType::FixedVector {
                lanes: left_lanes,
                element: left_element,
            },
            BinaryType::FixedVector {
                lanes: right_lanes,
                element: right_element,
            },
        ) => left_lanes == right_lanes && same_scalar_type(types, *left_element, *right_element),
        _ => false,
    }
}

fn same_scalar_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    let Some(left) = usize::try_from(left)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    let Some(right) = usize::try_from(right)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    matches!((left, right), (BinaryType::Integer(left), BinaryType::Integer(right)) if left == right)
        || matches!((left, right), (BinaryType::Float(left), BinaryType::Float(right)) if left == right)
}

fn same_default(constants: Option<&BinaryConstantTable>, left: u64, right: u64) -> bool {
    let Some(constants) = constants else {
        return left == right;
    };
    let left = usize::try_from(left)
        .ok()
        .and_then(|index| constants.records.get(index));
    let right = usize::try_from(right)
        .ok()
        .and_then(|index| constants.records.get(index));
    matches!((left, right), (Some(left), Some(right)) if left.value == right.value)
}

fn default_has_type(constants: Option<&BinaryConstantTable>, constant: u64, ty: u64) -> bool {
    constants
        .and_then(|constants| {
            usize::try_from(constant)
                .ok()
                .and_then(|index| constants.records.get(index))
        })
        .is_some_and(|constant| constant.ty == ty)
}

fn report(diagnostics: &mut Vec<Diagnostic>, message: String) {
    diagnostics.push(Diagnostic::error(V2, message));
}
