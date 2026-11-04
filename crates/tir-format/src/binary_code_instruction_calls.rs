use crate::FormatError;
use crate::strings::StringTable;
use crate::types::{BinaryOperand, BinaryType, BinaryTypeTable};

use super::{MAX_LIST_COUNT, malformed, read_index, read_symbol, read_type, read_values};

const CALL_ATTRIBUTES: u64 = 0x1f << 41;
const FOREIGN_UNWIND: u64 = 1 << 46;

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    block_count: u64,
    result_count: u64,
    types: &BinaryTypeTable,
    strings: &StringTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(u64, bool), FormatError> {
    let allowed = CALL_ATTRIBUTES | if opcode == 0x1aa { FOREIGN_UNWIND } else { 0 };
    if modifiers & !allowed != 0 || modifiers & (1 << 44) != 0 && modifiers & (1 << 45) != 0 {
        return Err(malformed(
            base + start,
            "invalid call instruction modifiers",
        ));
    }
    let (results, terminator) = match opcode {
        0x1a0 => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            read_type_arguments(bytes, cursor, base, types, operands)?;
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            (ordinary_results(result_count, base + start)?, false)
        }
        0x1a1 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let type_index = read_type(bytes, cursor, base, types)?;
            operands.push(BinaryOperand::Type(type_index as u64));
            let argument_count = read_argument_list(bytes, cursor, base, current_value, operands)?;
            if usize::try_from(argument_count).ok()
                != Some(function_parameter_count(
                    type_index as u64,
                    types,
                    base + start,
                )?)
            {
                return Err(malformed(
                    base + start,
                    "indirect-call argument count disagrees with its function type",
                ));
            }
            let expected = function_results(type_index as u64, types, base + start)?;
            if result_count != expected {
                return Err(malformed(
                    base + start,
                    "indirect-call result count disagrees with its function type",
                ));
            }
            (expected, false)
        }
        0x1a2 => {
            if modifiers != 0 {
                return Err(malformed(
                    base + start,
                    "closure.new does not accept call attributes",
                ));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            (1, false)
        }
        0x1a3 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            (ordinary_results(result_count, base + start)?, false)
        }
        0x1a4 => {
            if modifiers != 0 {
                return Err(malformed(
                    base + start,
                    "closure.callback does not accept call attributes",
                ));
            }
            let index = read_index(bytes, cursor, base, "closure callback parameter")?;
            operands.push(BinaryOperand::Index(index));
            let type_index = read_type(bytes, cursor, base, types)?;
            function_parameter_count(type_index as u64, types, base + start)?;
            operands.push(BinaryOperand::Type(type_index as u64));
            (1, false)
        }
        0x1a5 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            let name_offset = *cursor;
            let name = read_index(bytes, cursor, base, "witness method name")?;
            if usize::try_from(name)
                .ok()
                .and_then(|index| strings.get(index))
                .is_none()
            {
                return Err(malformed(
                    base + name_offset,
                    "witness method name is out of range",
                ));
            }
            operands.push(BinaryOperand::Index(name));
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            (ordinary_results(result_count, base + start)?, false)
        }
        0x1a6 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(read_symbol(bytes, cursor, base, Some(1))?);
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "vtable slot",
            )?));
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            (ordinary_results(result_count, base + start)?, false)
        }
        0x1a7 => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "try.call normal target",
                operands,
            )?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "try.call error target",
                operands,
            )?;
            (0, true)
        }
        0x1a8 => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            (ordinary_results(result_count, base + start)?, false)
        }
        0x1a9 => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            let function_type = read_type(bytes, cursor, base, types)? as u64;
            let expected = function_results(function_type, types, base + start)?;
            let expected_fixed = function_parameter_count(function_type, types, base + start)?;
            operands.push(BinaryOperand::Type(function_type));
            let fixed_count_offset = *cursor;
            let fixed_count = read_index(bytes, cursor, base, "fixed variadic argument count")?;
            let variadic_count = read_index(bytes, cursor, base, "variadic argument count")?;
            if fixed_count >= MAX_LIST_COUNT
                || variadic_count >= MAX_LIST_COUNT
                || fixed_count.checked_add(variadic_count).is_none()
                || usize::try_from(fixed_count).ok() != Some(expected_fixed)
            {
                return Err(malformed(
                    base + fixed_count_offset,
                    "variadic argument count is invalid",
                ));
            }
            if result_count != expected {
                return Err(malformed(
                    base + start,
                    "variadic-call result count disagrees with its function type",
                ));
            }
            read_values(bytes, cursor, base, current_value, fixed_count, operands)?;
            read_values(bytes, cursor, base, current_value, variadic_count, operands)?;
            (ordinary_results(result_count, base + start)?, false)
        }
        0x1aa => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "invoke normal target",
                operands,
            )?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "invoke unwind target",
                operands,
            )?;
            (0, true)
        }
        0x1e7 => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            read_type_arguments(bytes, cursor, base, types, operands)?;
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            (0, true)
        }
        _ => return Err(malformed(base + start, "unsupported call opcode")),
    };
    Ok((results, terminator))
}

fn ordinary_results(count: u64, offset: usize) -> Result<u64, FormatError> {
    if count <= 1 {
        Ok(count)
    } else {
        Err(malformed(offset, "call result count must be zero or one"))
    }
}

fn function_results(
    type_index: u64,
    types: &BinaryTypeTable,
    offset: usize,
) -> Result<u64, FormatError> {
    let Some(BinaryType::Function(function)) = usize::try_from(type_index)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return Err(malformed(offset, "call type index is not a function type"));
    };
    if function.asynchronous {
        return Ok(1);
    }
    let Some(result) = usize::try_from(function.result)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return Err(malformed(offset, "call result type is out of range"));
    };
    Ok(u64::from(!matches!(
        result,
        BinaryType::Unit | BinaryType::Never
    )))
}

fn function_parameter_count(
    type_index: u64,
    types: &BinaryTypeTable,
    offset: usize,
) -> Result<usize, FormatError> {
    match usize::try_from(type_index)
        .ok()
        .and_then(|index| types.entries.get(index))
    {
        Some(BinaryType::Function(function)) => Ok(function.parameters.len()),
        _ => Err(malformed(offset, "call type index is not a function type")),
    }
}

fn read_type_arguments(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "call type-argument count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + count_offset,
            "call type-argument count exceeds payload",
        ));
    }
    let capacity = usize::try_from(count).map_err(|_| {
        malformed(
            base + count_offset,
            "call type-argument count overflows memory",
        )
    })?;
    let mut arguments = Vec::new();
    arguments
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + *cursor, "call type-argument allocation failed"))?;
    for _ in 0..count {
        arguments.push(read_type(bytes, cursor, base, types)? as u64);
    }
    operands.push(BinaryOperand::Types(arguments));
    Ok(())
}

fn read_argument_list(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    let count = read_index(bytes, cursor, base, "call argument count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + offset,
            "call argument count exceeds payload",
        ));
    }
    read_values(bytes, cursor, base, current_value, count, operands)?;
    Ok(count)
}

fn read_block_index(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    block_count: u64,
    field: &str,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor, base, field)?;
    if index >= block_count {
        return Err(malformed(base + offset, format!("{field} is out of range")));
    }
    operands.push(BinaryOperand::Index(index));
    Ok(())
}
