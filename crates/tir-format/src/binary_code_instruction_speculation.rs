use crate::FormatError;
use crate::types::BinaryOperand;

use super::{
    MAX_LIST_COUNT, malformed, read_index, read_symbol, read_values, require_no_modifiers,
};

pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(u64, bool), FormatError> {
    require_no_modifiers(modifiers, base + *cursor)?;
    if opcode == 0x260 {
        read_values(bytes, cursor, base, current_value, 1, operands)?;
    } else if opcode != 0x261 {
        return Err(malformed(base + *cursor, "unsupported speculation opcode"));
    }

    operands.push(read_symbol(bytes, cursor, base, Some(0))?);
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "deoptimization state count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + count_offset,
            "deoptimization state count exceeds payload",
        ));
    }
    read_values(bytes, cursor, base, current_value, count, operands)?;
    Ok((0, opcode == 0x261))
}
