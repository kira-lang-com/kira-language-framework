use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_type, read_values, require_no_modifiers};

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
) -> Result<(u64, bool), FormatError> {
    require_no_modifiers(modifiers, base + *cursor)?;
    let (values, result_count, terminator) = match opcode {
        0x210 | 0x213 | 0x214 => (1, u64::from(opcode == 0x214), false),
        0x211 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            (1, 1, false)
        }
        0x212 | 0x215 => (2, 0, opcode == 0x215),
        _ => return Err(malformed(base + *cursor, "unsupported variadic opcode")),
    };
    read_values(bytes, cursor, base, current_value, values, operands)?;
    Ok((result_count, terminator))
}
