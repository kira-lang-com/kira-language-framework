use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_index, read_type, read_value_list, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const UNCHECKED: u64 = 1 << 33;
const MAX_LIST_COUNT: u64 = 1 << 32;

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    encoded_result_count: u64,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<u64, FormatError> {
    let relative = opcode - 0x100;
    let allowed_modifiers = if relative == 8 { UNCHECKED } else { 0 };
    if modifiers & SELECTOR_MASK != 0 || modifiers & !(allowed_modifiers | SELECTOR_MASK) != 0 {
        return Err(malformed(
            base + *cursor,
            "invalid aggregate instruction modifiers",
        ));
    }
    match relative {
        0 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_counted_values(bytes, cursor, base, current_value, operands)?;
        }
        1 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "field index",
            )?));
        }
        2 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "field index",
            )?));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
        }
        3 | 6 => read_values(bytes, cursor, base, current_value, 1, operands)?,
        4 => read_counted_values(bytes, cursor, base, current_value, operands)?,
        5 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "tuple element index",
            )?));
        }
        7 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_counted_values(bytes, cursor, base, current_value, operands)?;
        }
        8 => read_values(bytes, cursor, base, current_value, 2, operands)?,
        9 => read_values(bytes, cursor, base, current_value, 3, operands)?,
        10 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "enum variant index",
            )?));
            read_counted_values(bytes, cursor, base, current_value, operands)?;
        }
        11 => read_values(bytes, cursor, base, current_value, 1, operands)?,
        12 | 13 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "enum variant index",
            )?));
        }
        _ => return Err(malformed(base + *cursor, "unsupported aggregate opcode")),
    }
    Ok(if matches!(relative, 3 | 6) {
        encoded_result_count
    } else {
        1
    })
}

fn read_counted_values(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "aggregate operand count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            count_offset,
            "aggregate operand count exceeds payload",
        ));
    }
    operands.push(BinaryOperand::Values(read_value_list(
        bytes,
        cursor,
        base,
        current_value,
        count,
    )?));
    Ok(())
}
