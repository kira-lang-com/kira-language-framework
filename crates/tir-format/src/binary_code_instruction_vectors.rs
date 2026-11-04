use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_index, read_integer, read_type, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const MASK_PRESENT: u64 = 1 << 32;

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<u64, FormatError> {
    let relative = opcode - 0x0c0;
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    match relative {
        6 if selector > 14 => {
            return Err(malformed(base + *cursor, "invalid vec.reduce selector"));
        }
        13 if selector > 3 => {
            return Err(malformed(base + *cursor, "invalid vec.dot selector"));
        }
        9 | 10 | 14 | 15 | 17 | 19 if selector > 1 => {
            return Err(malformed(base + *cursor, "invalid vector selector"));
        }
        6 | 9 | 10 | 13 | 14 | 15 | 17 | 19 => {}
        _ if selector != 0 => {
            return Err(malformed(base + *cursor, "unexpected vector selector"));
        }
        _ => {}
    }

    match relative {
        0 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
        }
        1 => read_values(bytes, cursor, base, current_value, 2, operands)?,
        2 | 11..=13 | 17 => read_values(bytes, cursor, base, current_value, 3, operands)?,
        3 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            let count_offset = *cursor;
            let count = read_index(bytes, cursor, base, "vector shuffle index count")?;
            if count > bytes.len().saturating_sub(*cursor) as u64 / 2 {
                return Err(malformed(
                    count_offset,
                    "vector shuffle index count exceeds payload",
                ));
            }
            let capacity = usize::try_from(count).map_err(|_| {
                malformed(count_offset, "vector shuffle index count overflows memory")
            })?;
            let mut indices = Vec::new();
            indices
                .try_reserve_exact(capacity)
                .map_err(|_| malformed(base + *cursor, "vector shuffle index allocation failed"))?;
            for _ in 0..count {
                let offset = *cursor;
                let index = read_integer(bytes, cursor, base, "vector shuffle index")?;
                if index.negative {
                    return Err(malformed(base + offset, "vector shuffle index is negative"));
                }
                indices.push(index);
            }
            operands.push(BinaryOperand::Integers(indices));
        }
        4 => operands.push(BinaryOperand::Type(
            read_type(bytes, cursor, base, types)? as u64
        )),
        5 => {}
        6 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            if modifiers & MASK_PRESENT != 0 {
                read_values(bytes, cursor, base, current_value, 1, operands)?;
            }
        }
        7 | 8 => {
            if relative == 7 {
                operands.push(BinaryOperand::Type(
                    read_type(bytes, cursor, base, types)? as u64
                ));
                read_values(bytes, cursor, base, current_value, 1, operands)?;
            } else {
                read_values(bytes, cursor, base, current_value, 2, operands)?;
            }
            let alignment_offset = *cursor;
            let alignment = read_index(bytes, cursor, base, "vector memory alignment")?;
            if alignment != 0 && !alignment.is_power_of_two() {
                return Err(malformed(
                    alignment_offset,
                    "vector alignment must be zero or a power of two",
                ));
            }
            operands.push(BinaryOperand::Index(alignment));
            if modifiers & MASK_PRESENT != 0 {
                read_values(
                    bytes,
                    cursor,
                    base,
                    current_value,
                    if relative == 7 { 2 } else { 1 },
                    operands,
                )?;
            }
        }
        9 | 10 => {
            if relative == 9 {
                operands.push(BinaryOperand::Type(
                    read_type(bytes, cursor, base, types)? as u64
                ));
            } else {
                read_values(bytes, cursor, base, current_value, 1, operands)?;
            }
            if selector == 0 {
                read_values(bytes, cursor, base, current_value, 1, operands)?;
            } else {
                read_values(bytes, cursor, base, current_value, 2, operands)?;
                operands.push(BinaryOperand::Index(read_index(
                    bytes,
                    cursor,
                    base,
                    "vector address scale",
                )?));
            }
            if modifiers & MASK_PRESENT != 0 {
                read_values(
                    bytes,
                    cursor,
                    base,
                    current_value,
                    if relative == 9 { 2 } else { 1 },
                    operands,
                )?;
            }
        }
        14 | 15 | 18 => read_values(bytes, cursor, base, current_value, 2, operands)?,
        16 | 19 | 21 | 22 => read_values(bytes, cursor, base, current_value, 1, operands)?,
        20 | 23 => read_values(bytes, cursor, base, current_value, 2, operands)?,
        _ => return Err(malformed(base + *cursor, "unsupported vector opcode")),
    }

    Ok(if matches!(relative, 8 | 10) { 0 } else { 1 })
}
