use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_type, read_values};

const UNINIT: u64 = 1 << 30;

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
    let allowed = if opcode == 0x164 { UNINIT } else { 0 };
    if modifiers & !allowed != 0 {
        return Err(malformed(
            base + *cursor,
            "invalid heap instruction modifiers",
        ));
    }
    match opcode {
        0x160 | 0x161 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            Ok(1)
        }
        0x162 | 0x163 | 0x165 | 0x167 | 0x168 | 0x169 | 0x16a => {
            let count = if opcode == 0x165 { 3 } else { 1 };
            read_values(bytes, cursor, base, current_value, count, operands)?;
            Ok(1)
        }
        0x164 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            Ok(1)
        }
        0x166 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok(0)
        }
        _ => Err(malformed(base + *cursor, "unsupported heap opcode")),
    }
}
