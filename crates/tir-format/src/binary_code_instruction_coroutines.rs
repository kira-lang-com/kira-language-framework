use crate::FormatError;
use crate::types::BinaryOperand;

use super::{
    MAX_LIST_COUNT, malformed, read_index, read_symbol, read_values, require_no_modifiers,
};

const SELECTOR_MASK: u64 = 0xf << 48;

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
    operands: &mut Vec<BinaryOperand>,
) -> Result<(u64, bool), FormatError> {
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    if opcode == 0x1c0 {
        if modifiers & !SELECTOR_MASK != 0 || selector > 1 {
            return Err(malformed(base + start, "invalid coro.start modifier"));
        }
    } else {
        require_no_modifiers(modifiers, base + start)?;
    }

    match opcode {
        0x1c0 => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            if selector == 1 {
                read_values(bytes, cursor, base, current_value, 1, operands)?;
            }
            Ok((1, false))
        }
        0x1c1 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "coroutine resume target",
                operands,
            )?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "coroutine destroy target",
                operands,
            )?;
            Ok((0, true))
        }
        0x1c2 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "coroutine yielded target",
                operands,
            )?;
            read_block_index(
                bytes,
                cursor,
                base,
                block_count,
                "coroutine done target",
                operands,
            )?;
            Ok((0, true))
        }
        0x1c3 => Ok((0, true)),
        0x1c4 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok((0, false))
        }
        0x1c5 | 0x1c8 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            if opcode == 0x1c5 {
                Ok((1, false))
            } else if result_count <= 1 {
                Ok((result_count, false))
            } else {
                Err(malformed(
                    base + start,
                    "await result count must be zero or one",
                ))
            }
        }
        0x1c6 | 0x1c7 => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            Ok((1, false))
        }
        0x1c9 => Ok((1, false)),
        _ => Err(malformed(base + start, "unsupported coroutine opcode")),
    }
}

fn read_argument_list(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "coroutine argument count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + count_offset,
            "coroutine argument count exceeds payload",
        ));
    }
    read_values(bytes, cursor, base, current_value, count, operands)
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
