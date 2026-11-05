use tir_format::{
    BinaryFunctionType, BinaryInstruction, BinaryOperand, BinarySignature, BinarySignatureTable,
    BinaryType, BinaryTypeTable,
};

pub(super) fn result_edge_types(types: &BinaryTypeTable, result: u64) -> Vec<u64> {
    match type_at(types, result) {
        Some(BinaryType::Unit | BinaryType::Never) | None => Vec::new(),
        Some(_) => vec![result],
    }
}

pub(super) fn direct_callee<'a>(
    instruction: &BinaryInstruction,
    signatures: &'a BinarySignatureTable,
) -> Option<(u64, &'a BinarySignature)> {
    let index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    let signature = usize::try_from(index)
        .ok()
        .and_then(|index| signatures.records.get(index))?;
    Some((index, signature))
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

pub(super) fn function_type(types: &BinaryTypeTable, index: u64) -> Option<&BinaryFunctionType> {
    match type_at(types, index)? {
        BinaryType::Function(function) => Some(function),
        _ => None,
    }
}

pub(super) fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

pub(super) fn is_generic_call(function: &BinaryFunctionType, types: &BinaryTypeTable) -> bool {
    function
        .parameters
        .iter()
        .any(|parameter| contains_generic(types, parameter.ty, &mut Vec::new()))
        || contains_generic(types, function.result, &mut Vec::new())
        || function
            .error
            .is_some_and(|ty| contains_generic(types, ty, &mut Vec::new()))
}

pub(super) fn contains_generic(types: &BinaryTypeTable, index: u64, seen: &mut Vec<u64>) -> bool {
    if seen.contains(&index) {
        return false;
    }
    seen.push(index);
    let Some(ty) = type_at(types, index) else {
        return false;
    };
    let children = match ty {
        BinaryType::Generic { .. } => return true,
        BinaryType::Address { pointee, .. }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee }
        | BinaryType::Tensor {
            element: pointee, ..
        } => vec![*pointee],
        BinaryType::FixedVector { element, .. }
        | BinaryType::ScalableVector { element }
        | BinaryType::Array { element, .. }
        | BinaryType::CooperativeMatrix { element, .. } => vec![*element],
        BinaryType::Tuple(elements) => elements.clone(),
        BinaryType::Nominal { arguments, .. } => arguments.clone(),
        BinaryType::Function(function) => function
            .parameters
            .iter()
            .map(|parameter| parameter.ty)
            .chain([function.result])
            .chain(function.error)
            .collect(),
        BinaryType::Closure { function_type, .. } => vec![*function_type],
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => vec![*yield_type, *resume_type, *result_type],
        BinaryType::VisibleFunctionTable { function_type } => vec![*function_type],
        BinaryType::Entry {
            root, immediate, ..
        } => vec![*root, *immediate],
        BinaryType::Integer(_)
        | BinaryType::Float(_)
        | BinaryType::Pointer(_)
        | BinaryType::Unit
        | BinaryType::Never
        | BinaryType::Frame { .. }
        | BinaryType::Texture { .. }
        | BinaryType::Sampler
        | BinaryType::AccelerationStructure
        | BinaryType::RayQuery
        | BinaryType::HitObject => Vec::new(),
    };
    children
        .into_iter()
        .any(|child| contains_generic(types, child, seen))
}

pub(super) fn explicit_type(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

pub(super) fn value_ids(instruction: &BinaryInstruction) -> Vec<u64> {
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

pub(super) fn value_type(
    instruction: &BinaryInstruction,
    index: usize,
    value_types: &[Option<u64>],
) -> Option<u64> {
    value_ids(instruction)
        .get(index)
        .and_then(|value| usize::try_from(*value).ok())
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}
