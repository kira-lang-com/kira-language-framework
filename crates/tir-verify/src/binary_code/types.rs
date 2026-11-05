use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryTarget,
    BinaryType, BinaryTypeTable,
};
use tir_types::FloatType;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

mod address;
mod address_instructions;
mod aggregate;
pub(super) mod aggregate_instructions;
mod coroutines;
mod gpu;
mod heap;
mod inference;
mod memory_instructions;
mod scalar_instructions;
mod vector_instructions;

use aggregate::{infer_aggregate_result, infer_destructure_results, infer_tuple_new};

pub(super) fn infer_value_types(
    body: &BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Vec<Option<u64>> {
    inference::infer_value_types(body, signatures, types)
}

pub(super) fn verify_function_references(
    body: &BinaryFunctionBody,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if instruction.opcode != 0x00a {
            continue;
        }
        let Some((kind, index)) = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Symbol { kind, index } => Some((*kind, *index)),
                _ => None,
            })
        else {
            report_function_reference_error(
                &mut diagnostics,
                function_name,
                instruction,
                "func.ref must name a function".to_owned(),
            );
            continue;
        };
        if kind != 0 {
            report_function_reference_error(
                &mut diagnostics,
                function_name,
                instruction,
                format!("func.ref requires a function symbol, found symbol kind {kind}"),
            );
            continue;
        }
        let Some(signature) = usize::try_from(index)
            .ok()
            .and_then(|index| signatures.records.get(index))
        else {
            report_function_reference_error(
                &mut diagnostics,
                function_name,
                instruction,
                format!("func.ref names unknown function symbol {index}"),
            );
            continue;
        };
        let arguments = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Types(types) => Some(types.as_slice()),
                _ => None,
            })
            .unwrap_or_default();
        if arguments.len() != signature.generic_parameters.len() {
            report_function_reference_error(
                &mut diagnostics,
                function_name,
                instruction,
                format!(
                    "func.ref @{} has {} type arguments; expected {}",
                    signature.name,
                    arguments.len(),
                    signature.generic_parameters.len()
                ),
            );
            continue;
        }
        let specialized = inference::specialize_function_type(types, signature, arguments);
        if !specialized
            .is_some_and(|index| matches!(type_at(types, index), Some(BinaryType::Function(_))))
        {
            report_function_reference_error(
                &mut diagnostics,
                function_name,
                instruction,
                format!(
                    "func.ref @{} does not resolve to a function type in TYPES",
                    signature.name
                ),
            );
        }
    }
    diagnostics
}

fn report_function_reference_error(
    diagnostics: &mut Vec<Diagnostic>,
    function_name: &str,
    instruction: &BinaryInstruction,
    message: String,
) {
    diagnostics.push(Diagnostic::error(
        V2,
        format!(
            "function @{function_name} CODE offset {}: {message}",
            instruction.offset
        ),
    ));
}

pub(super) fn specialize_nominal_type(
    types: &BinaryTypeTable,
    type_index: u64,
    arguments: &[u64],
) -> Option<u64> {
    address::substitute_type(types, type_index, arguments)
}

pub(super) fn verify_scalar_instructions(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    scalar_instructions::verify(body, value_types, types, function_name)
}

pub(super) fn verify_vector_instructions(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    vector_instructions::verify(body, value_types, types, function_name)
}

pub(super) fn verify_aggregate_instructions(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    aggregate_instructions::verify(body, value_types, types, function_name)
}

pub(super) fn verify_address_instructions(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    target: BinaryTarget,
) -> Vec<tir_diag::Diagnostic> {
    address_instructions::verify(body, value_types, types, function_name, target)
}

pub(super) fn verify_memory_instructions(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    memory_instructions::verify(body, value_types, types, function_name)
}

pub(super) fn verify_closure_contexts(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut null_values = std::collections::HashSet::new();
    let mut value_cursor = 0usize;

    for block in &body.blocks {
        value_cursor = value_cursor.saturating_add(block.parameters.len());
        let Some(instructions) = &block.instructions else {
            value_cursor = value_cursor.saturating_add(block.instruction_count as usize);
            continue;
        };
        for instruction in instructions {
            if instruction.opcode == 0x1a2 {
                let context = instruction
                    .operands
                    .iter()
                    .find_map(|operand| match operand {
                        BinaryOperand::Values(values) => values.get(1).copied(),
                        _ => None,
                    });
                let valid_context = context
                    .and_then(|value| {
                        value_types
                            .get(usize::try_from(value).ok()?)
                            .copied()
                            .flatten()
                            .map(|ty| (value, ty))
                    })
                    .is_some_and(|(value, ty)| {
                        match types.entries.get(usize::try_from(ty).unwrap_or(usize::MAX)) {
                            Some(BinaryType::Unique { .. }) => true,
                            Some(BinaryType::Pointer(_)) => null_values.contains(&value),
                            _ => false,
                        }
                    });
                if !valid_context {
                    diagnostics.push(Diagnostic::error(
                        V2,
                        format!(
                            "function @{function_name} CODE offset {}: closure.new context must be an owned unique value or null",
                            instruction.offset
                        ),
                    ));
                }
            }
            if instruction.opcode == 0x003 && instruction.result_count == 1 {
                null_values.insert(value_cursor as u64);
            }
            value_cursor = value_cursor.saturating_add(instruction.result_count as usize);
        }
    }
    diagnostics
}

pub(super) fn verify_closure_callbacks(
    body: &BinaryFunctionBody,
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter().flatten())
    {
        if instruction.opcode != 0x1a4 {
            continue;
        }
        let parameter_index = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => usize::try_from(*index).ok(),
                _ => None,
            });
        let function_type = instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Type(index) => usize::try_from(*index)
                    .ok()
                    .and_then(|index| types.entries.get(index)),
                _ => None,
            });
        let valid = match (parameter_index, function_type) {
            (Some(index), Some(BinaryType::Function(signature))) if signature.c_abi => {
                signature.parameters.get(index).is_some_and(|parameter| {
                    usize::try_from(parameter.ty)
                        .ok()
                        .and_then(|index| types.entries.get(index))
                        .is_some_and(|ty| matches!(ty, BinaryType::Pointer(0)))
                })
            }
            _ => false,
        };
        if !valid {
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "function @{function_name} CODE offset {}: closure.callback requires an abi(c) function type whose indexed parameter is ptr",
                    instruction.offset
                ),
            ));
        }
    }
    diagnostics
}

fn address_pointee(types: &BinaryTypeTable, address: u64) -> Option<u64> {
    match type_at(types, address)? {
        BinaryType::Address { pointee, .. } => Some(*pointee),
        _ => None,
    }
}

fn address_space(types: &BinaryTypeTable, address: u64) -> Option<u8> {
    match type_at(types, address)? {
        BinaryType::Address { space, .. } => Some(*space),
        _ => None,
    }
}

fn reference_pointee(types: &BinaryTypeTable, reference: u64) -> Option<u64> {
    match type_at(types, reference)? {
        BinaryType::Reference { pointee } | BinaryType::Unique { pointee } => Some(*pointee),
        _ => None,
    }
}

fn weak_type(types: &BinaryTypeTable, pointee: u64) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Weak { pointee: actual } if *actual == pointee),
    )
}

fn atomic_compare_exchange_type(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<u64> {
    let value = value_type_at(instruction, 1, value_types)?;
    let flag = integer_type(types, 1)?;
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Tuple(elements) if elements == &[value, flag]),
    )
}

fn infer_closure_type(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<u64> {
    let function_type = value_type_at(instruction, 0, value_types)?;
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Closure { function_type: actual, borrowed: false } if *actual == function_type),
    )
}

fn lane_type(types: &BinaryTypeTable, source: u64, element: u64) -> Option<u64> {
    match type_at(types, source)? {
        BinaryType::FixedVector { lanes, .. } => vector_type(types, *lanes, element),
        BinaryType::ScalableVector { .. } => type_index(
            types,
            |ty| matches!(ty, BinaryType::ScalableVector { element: actual } if *actual == element),
        ),
        _ => Some(element),
    }
}

fn overflow_result(value: u64, types: &BinaryTypeTable) -> Option<u64> {
    let flag = lane_type(types, value, integer_type(types, 1)?)?;
    tuple_type(types, &[value, flag])
}

fn tuple_type(types: &BinaryTypeTable, elements: &[u64]) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Tuple(actual) if actual == elements),
    )
}

fn string_type(types: &BinaryTypeTable) -> Option<u64> {
    let pointer = pointer_type(types, 0)?;
    let length = integer_type(types, 64)?;
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Tuple(items) if items == &[pointer, length]),
    )
}

fn address_type(types: &BinaryTypeTable, pointee: u64, space: u8) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Address { pointee: actual, space: actual_space } if *actual == pointee && *actual_space == space),
    )
}

fn reference_type(types: &BinaryTypeTable, pointee: u64) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Reference { pointee: actual } if *actual == pointee),
    )
}

fn unique_type(types: &BinaryTypeTable, pointee: u64) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Unique { pointee: actual } if *actual == pointee),
    )
}

fn pointer_type(types: &BinaryTypeTable, space: u8) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Pointer(actual) if *actual == space),
    )
}

fn integer_type(types: &BinaryTypeTable, bits: u64) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Integer(width) if *width == bits),
    )
}

fn float_type(types: &BinaryTypeTable, float: FloatType) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Float(actual) if *actual == float),
    )
}

fn vector_type(types: &BinaryTypeTable, lanes: u64, element: u64) -> Option<u64> {
    type_index(
        types,
        |ty| matches!(ty, BinaryType::FixedVector { lanes: actual_lanes, element: actual_element } if *actual_lanes == lanes && *actual_element == element),
    )
}

fn type_index(types: &BinaryTypeTable, predicate: impl Fn(&BinaryType) -> bool) -> Option<u64> {
    types
        .entries
        .iter()
        .position(predicate)
        .map(|index| index as u64)
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn explicit_type(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

fn index_operand(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
}

fn selector(instruction: &BinaryInstruction) -> u64 {
    (instruction.modifiers >> 48) & 0xf
}

fn value_type_at(
    instruction: &BinaryInstruction,
    operand_index: usize,
    value_types: &[Option<u64>],
) -> Option<u64> {
    let mut values = Vec::new();
    for operand in &instruction.operands {
        match operand {
            BinaryOperand::Value(value) => values.push(*value),
            BinaryOperand::Values(items) => values.extend_from_slice(items),
            _ => {}
        }
    }
    values
        .get(operand_index)
        .and_then(|value| usize::try_from(*value).ok())
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}
