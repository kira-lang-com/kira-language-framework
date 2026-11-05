use tir_format::{
    BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable,
};

pub(super) fn infer_start_result(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let function_index = function_index(instruction)?;
    let signature = signatures
        .records
        .get(usize::try_from(function_index).ok()?)?;
    if !signature.generic_parameters.is_empty() {
        return None;
    }
    let (yield_type, resume_type) = signature.coroutine?;
    let BinaryType::Function(function) = type_at(types, signature.function_type)? else {
        return None;
    };
    coroutine_type(types, yield_type, resume_type, function.result)
}

pub(super) fn infer_await_result(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let future = value_type(instruction, 0, value_types)?;
    let BinaryType::Frame { function_index } = type_at(types, future)? else {
        return None;
    };
    let signature = signatures
        .records
        .get(usize::try_from(*function_index).ok()?)?;
    let BinaryType::Function(function) = type_at(types, signature.function_type)? else {
        return None;
    };
    if !function.asynchronous || !signature.generic_parameters.is_empty() {
        return None;
    }
    value_result(types, function.result)
}

pub(super) fn infer_async_waker(types: &BinaryTypeTable) -> Option<u64> {
    types
        .entries
        .iter()
        .position(|ty| {
            let BinaryType::Nominal {
                declaration,
                arguments,
            } = ty
            else {
                return false;
            };
            arguments.is_empty()
                && usize::try_from(*declaration)
                    .ok()
                    .and_then(|index| types.declarations.get(index))
                    .is_some_and(|declaration| declaration.name == "Async.Waker")
        })
        .map(|index| index as u64)
}

pub(super) fn frame_type(types: &BinaryTypeTable, function_index: u64) -> Option<u64> {
    types
        .entries
        .iter()
        .position(|ty| {
            matches!(
                ty,
                BinaryType::Frame {
                    function_index: actual
                } if *actual == function_index
            )
        })
        .map(|index| index as u64)
}

fn function_index(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })
}

fn value_type(
    instruction: &BinaryInstruction,
    operand_index: usize,
    value_types: &[Option<u64>],
) -> Option<u64> {
    let value = instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => vec![*value],
            BinaryOperand::Values(values) => values.clone(),
            _ => Vec::new(),
        })
        .nth(operand_index)?;
    value_types
        .get(usize::try_from(value).ok()?)?
        .as_ref()
        .copied()
}

fn coroutine_type(
    types: &BinaryTypeTable,
    yield_type: u64,
    resume_type: u64,
    result_type: u64,
) -> Option<u64> {
    types
        .entries
        .iter()
        .position(|ty| {
            matches!(
                ty,
                BinaryType::Coroutine {
                    yield_type: actual_yield,
                    resume_type: actual_resume,
                    result_type: actual_result,
                } if *actual_yield == yield_type
                    && *actual_resume == resume_type
                    && *actual_result == result_type
            )
        })
        .map(|index| index as u64)
}

fn value_result(types: &BinaryTypeTable, result: u64) -> Option<u64> {
    match type_at(types, result)? {
        BinaryType::Unit | BinaryType::Never => None,
        _ => Some(result),
    }
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}
