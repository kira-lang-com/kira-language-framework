use crate::FormatError;
use crate::types::{
    BinaryBlock, BinaryFunctionBody, BinaryFunctionType, BinaryInstruction, BinaryOperand,
    BinaryType, BinaryTypeTable,
};

pub(super) fn value_operands(
    instruction: &BinaryInstruction,
    count: usize,
) -> Result<Vec<u64>, FormatError> {
    match instruction.operands.as_slice() {
        [BinaryOperand::Value(value)] if count == 1 => Ok(vec![*value]),
        [BinaryOperand::Values(values)] if values.len() == count => Ok(values.clone()),
        _ => Err(malformed("unexpected operation operand shape")),
    }
}

pub(super) fn mask_type_index(types: &BinaryTypeTable, source: u64) -> Result<u64, FormatError> {
    let lanes = match get_type(types, source)? {
        BinaryType::FixedVector { lanes, .. } => Some(*lanes),
        _ => None,
    };
    types
        .entries
        .iter()
        .enumerate()
        .find_map(|(index, ty)| {
            let matches_mask = match (lanes, ty) {
                (None, BinaryType::Integer(1)) => true,
                (
                    Some(expected),
                    BinaryType::FixedVector {
                        lanes: actual,
                        element,
                    },
                ) => {
                    expected == *actual
                        && matches!(get_type(types, *element), Ok(BinaryType::Integer(1)))
                }
                _ => false,
            };
            matches_mask.then_some(index as u64)
        })
        .ok_or_else(|| malformed("matching scalar/vector i1 mask type is not interned"))
}

pub(super) fn get_type(types: &BinaryTypeTable, index: u64) -> Result<&BinaryType, FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|i| types.entries.get(i))
        .ok_or_else(|| malformed("type index is out of range"))
}
pub(super) fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("SSA value index is out of range"))
}
pub(super) fn malformed(message: &'static str) -> FormatError {
    FormatError::malformed(None, message)
}

pub(super) fn formatting_error(error: std::fmt::Error) -> FormatError {
    FormatError::malformed(None, error.to_string())
}

pub(super) fn append_operation(
    text: &mut String,
    opcode: &str,
    operands: &[u64],
    result_ty: u64,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    use std::fmt::Write as _;
    let result_name = format!("v{}", values.len());
    write!(text, "    %{result_name} = {opcode}").map_err(formatting_error)?;
    for (index, operand) in operands.iter().enumerate() {
        let (name, _) = get_value(values, *operand)?;
        write!(text, "{}%{name}", if index == 0 { " " } else { ", " }).map_err(formatting_error)?;
    }
    text.push('\n');
    values.push((result_name, result_ty));
    Ok(())
}

pub(super) fn select_operands(
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &[(String, u64)],
) -> Result<(Vec<u64>, u64), FormatError> {
    if instruction.result_count != 1 || instruction.modifiers != 0 {
        return Err(malformed("invalid select result or modifiers"));
    }
    let operands = value_operands(instruction, 3)?;
    let ty = get_value(values, operands[1])?.1;
    if get_value(values, operands[2])?.1 != ty {
        return Err(malformed("select value types must match"));
    }
    if get_value(values, operands[0])?.1 != mask_type_index(types, ty)? {
        return Err(malformed("select condition has wrong i1 mask shape"));
    }
    Ok((operands, ty))
}

pub(super) fn single_block<'a>(
    body: &'a BinaryFunctionBody,
    function: &BinaryFunctionType,
) -> Result<(&'a BinaryBlock, &'a Vec<BinaryInstruction>), FormatError> {
    let [block] = body.blocks.as_slice() else {
        return Err(malformed("straight-line disassembly requires one block"));
    };
    let instructions = block
        .instructions
        .as_ref()
        .ok_or_else(|| malformed("unsupported CODE instruction"))?;
    if body.block_count != 1
        || block.instruction_count != instructions.len() as u64
        || block.parameters.len() != function.parameters.len()
        || block.cold
        || block.loop_merge.is_some()
        || block.loop_continue.is_some()
    {
        return Err(malformed("unsupported straight-line CODE block"));
    }
    Ok((block, instructions))
}
