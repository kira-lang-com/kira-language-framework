use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryLinkage, BinaryOperand, BinarySignatureTable, BinaryTarget,
    BinaryType, BinaryTypeTable,
};

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

pub(super) fn verify(
    bodies: &[Option<&BinaryFunctionBody>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (function_index, signature) in signatures.records.iter().enumerate() {
        if !matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both) {
            continue;
        }
        let inlineable =
            signature.linkage == BinaryLinkage::Module && signature.entry_kind.is_none();
        let Some(BinaryType::Function(function_type)) = usize::try_from(signature.function_type)
            .ok()
            .and_then(|index| types.entries.get(index))
        else {
            continue;
        };
        if !inlineable {
            for parameter in &function_type.parameters {
                if contains_register_only(types, parameter.ty) {
                    diagnostics.push(Diagnostic::error(
                        V15,
                        format!(
                            "GPU function @{} exposes a register-only parameter across a function boundary",
                            signature.name
                        ),
                    ));
                }
            }
            for type_index in std::iter::once(function_type.result).chain(function_type.error) {
                if contains_register_only(types, type_index) {
                    diagnostics.push(Diagnostic::error(
                        V15,
                        format!(
                            "GPU function @{} exposes a register-only result across a function boundary",
                            signature.name
                        ),
                    ));
                }
            }
        }
        let Some(body) = bodies.get(function_index).copied().flatten() else {
            continue;
        };
        verify_body(body, signature, signatures, types, diagnostics);
    }
}

fn verify_body(
    body: &BinaryFunctionBody,
    signature: &tir_format::BinarySignature,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let value_types = super::super::infer_value_types(body, signatures, types);
    let mut value_cursor = 0usize;
    for block in &body.blocks {
        value_cursor = value_cursor.saturating_add(block.parameters.len());
        for instruction in block.instructions.iter().flatten() {
            let values = instruction_values(instruction);
            if instruction.opcode == 0x120
                && instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::Type(index) => Some(*index),
                        _ => None,
                    })
                    .is_some_and(|type_index| contains_register_only(types, type_index))
                && !value_type_is_thread_address(
                    &value_types,
                    u64::try_from(value_cursor).unwrap_or(u64::MAX),
                    types,
                )
            {
                report(
                    diagnostics,
                    signature,
                    instruction.offset,
                    "register-only stack slots must use thread address space",
                );
            }
            if matches!(instruction.opcode, 0x142 | 0x147)
                && values
                    .get(1)
                    .and_then(|value| value_type(&value_types, *value))
                    .is_some_and(|type_index| contains_register_only(types, type_index))
                && values.first().is_none_or(|pointer| {
                    !value_type_is_thread_address(&value_types, *pointer, types)
                })
            {
                report(
                    diagnostics,
                    signature,
                    instruction.offset,
                    "register-only GPU values may only be stored in thread-address-space slots",
                );
            }
            if is_call(instruction.opcode)
                && values.iter().any(|value| {
                    value_type(&value_types, *value)
                        .is_some_and(|type_index| contains_register_only(types, type_index))
                })
                && !module_gpu_callee(instruction, signatures)
            {
                report(
                    diagnostics,
                    signature,
                    instruction.offset,
                    "register-only GPU values may only cross calls to module-linkage GPU functions",
                );
            }
            if instruction.opcode == 0x180
                && values
                    .first()
                    .and_then(|value| value_type(&value_types, *value))
                    .is_some_and(|type_index| contains_register_only(types, type_index))
                && (signature.linkage != BinaryLinkage::Module || signature.entry_kind.is_some())
            {
                report(
                    diagnostics,
                    signature,
                    instruction.offset,
                    "register-only GPU values cannot be returned across a function boundary",
                );
            }
            value_cursor = value_cursor
                .saturating_add(usize::try_from(instruction.result_count).unwrap_or(usize::MAX));
        }
    }
}

fn module_gpu_callee(
    instruction: &tir_format::BinaryInstruction,
    signatures: &BinarySignatureTable,
) -> bool {
    if !matches!(instruction.opcode, 0x1a0 | 0x1a7 | 0x1aa | 0x1e7) {
        return false;
    }
    let Some(target) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => usize::try_from(*index).ok(),
            _ => None,
        })
    else {
        return false;
    };
    signatures.records.get(target).is_some_and(|callee| {
        callee.linkage == BinaryLinkage::Module
            && matches!(callee.target, BinaryTarget::Gpu | BinaryTarget::Both)
            && callee.entry_kind.is_none()
    })
}

fn is_call(opcode: u64) -> bool {
    matches!(opcode, 0x1a0..=0x1a9 | 0x1aa | 0x1e7 | 0x404)
}

fn instruction_values(instruction: &tir_format::BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => vec![*value],
            BinaryOperand::Values(values) => values.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn value_type(value_types: &[Option<u64>], value: u64) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn value_type_is_thread_address(
    value_types: &[Option<u64>],
    value: u64,
    types: &BinaryTypeTable,
) -> bool {
    value_type(value_types, value)
        .and_then(|index| {
            usize::try_from(index)
                .ok()
                .and_then(|index| types.entries.get(index))
        })
        .is_some_and(|ty| {
            matches!(
                ty,
                BinaryType::Pointer(4) | BinaryType::Address { space: 4, .. }
            )
        })
}

fn contains_register_only(types: &BinaryTypeTable, type_index: u64) -> bool {
    contains_register_only_inner(types, type_index, &mut HashSet::new())
}

fn contains_register_only_inner(
    types: &BinaryTypeTable,
    type_index: u64,
    seen: &mut HashSet<u64>,
) -> bool {
    if !seen.insert(type_index) {
        return false;
    }
    let Some(ty) = usize::try_from(type_index)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return false;
    };
    if matches!(
        ty,
        BinaryType::RayQuery | BinaryType::HitObject | BinaryType::CooperativeMatrix { .. }
    ) {
        return true;
    }
    match ty {
        BinaryType::Address { pointee, .. }
        | BinaryType::FixedVector {
            element: pointee, ..
        }
        | BinaryType::ScalableVector { element: pointee }
        | BinaryType::Array {
            element: pointee, ..
        }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee }
        | BinaryType::Texture {
            component: pointee, ..
        }
        | BinaryType::Tensor {
            element: pointee, ..
        } => contains_register_only_inner(types, *pointee, seen),
        BinaryType::Tuple(elements) => elements
            .iter()
            .any(|element| contains_register_only_inner(types, *element, seen)),
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            arguments
                .iter()
                .any(|argument| contains_register_only_inner(types, *argument, seen))
                || types
                    .definitions
                    .iter()
                    .find(|definition| definition.declaration == *declaration)
                    .is_some_and(|definition| {
                        definition
                            .fields
                            .iter()
                            .map(|field| field.ty)
                            .chain(
                                definition
                                    .variants
                                    .iter()
                                    .flat_map(|variant| variant.payload.iter().copied()),
                            )
                            .filter_map(|field| {
                                super::super::types::specialize_nominal_type(
                                    types, field, arguments,
                                )
                            })
                            .any(|field| contains_register_only_inner(types, field, seen))
                    })
        }
        BinaryType::Function(function) => {
            function
                .parameters
                .iter()
                .any(|parameter| contains_register_only_inner(types, parameter.ty, seen))
                || contains_register_only_inner(types, function.result, seen)
                || function
                    .error
                    .is_some_and(|error| contains_register_only_inner(types, error, seen))
        }
        BinaryType::Closure { function_type, .. }
        | BinaryType::VisibleFunctionTable { function_type } => {
            contains_register_only_inner(types, *function_type, seen)
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            contains_register_only_inner(types, *yield_type, seen)
                || contains_register_only_inner(types, *resume_type, seen)
                || contains_register_only_inner(types, *result_type, seen)
        }
        BinaryType::Entry {
            root, immediate, ..
        } => {
            contains_register_only_inner(types, *root, seen)
                || contains_register_only_inner(types, *immediate, seen)
        }
        _ => false,
    }
}

fn report(
    diagnostics: &mut Vec<Diagnostic>,
    signature: &tir_format::BinarySignature,
    offset: u64,
    message: &str,
) {
    diagnostics.push(Diagnostic::error(
        V15,
        format!(
            "function @{} CODE offset {offset}: {message}",
            signature.name
        ),
    ));
}
