use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_type, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;

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
    let relative = opcode - 0x180;
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    if relative == 7 {
        if selector > 5 || modifiers & !SELECTOR_MASK != 0 {
            return Err(malformed(base + *cursor, "invalid access.begin selector"));
        }
    } else if modifiers != 0 {
        return Err(malformed(
            base + *cursor,
            "ownership instruction does not accept modifiers",
        ));
    }

    match relative {
        0..=5 => read_values(bytes, cursor, base, current_value, 1, operands)?,
        6 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
        }
        7 => read_values(bytes, cursor, base, current_value, 1, operands)?,
        8 => read_values(bytes, cursor, base, current_value, 1, operands)?,
        _ => return Err(malformed(base + *cursor, "unsupported ownership opcode")),
    }
    Ok(match relative {
        2 | 4 | 8 => 0,
        _ => 1,
    })
}
