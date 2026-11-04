use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_index, read_type, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const UNCHECKED: u64 = 1 << 33;

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
    let relative = opcode - 0x120;
    let allowed = if relative == 3 { UNCHECKED } else { 0 };
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    if (relative == 0 && selector > 10)
        || (relative != 0 && selector != 0)
        || modifiers & !(allowed | SELECTOR_MASK) != 0
    {
        return Err(malformed(
            base + *cursor,
            "invalid address instruction modifiers",
        ));
    }
    match relative {
        0 => {
            operands.push(BinaryOperand::Index(selector));
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            let present_offset = *cursor;
            let present = read_index(bytes, cursor, base, "stack allocation count flag")?;
            match present {
                0 => operands.push(BinaryOperand::OptionalValue(None)),
                1 => {
                    let delta_offset = *cursor;
                    let delta = read_index(bytes, cursor, base, "stack allocation count delta")?;
                    if delta == 0 || delta > current_value {
                        return Err(malformed(
                            base + delta_offset,
                            "stack allocation count is not an earlier SSA value",
                        ));
                    }
                    operands.push(BinaryOperand::OptionalValue(Some(current_value - delta)));
                }
                _ => {
                    return Err(malformed(
                        base + present_offset,
                        "invalid stack allocation count flag",
                    ));
                }
            }
            return Ok(1);
        }
        1 | 7 => read_values(bytes, cursor, base, current_value, 1, operands)?,
        2 | 4 | 5 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "aggregate field or variant index",
            )?));
            if relative == 5 {
                return Ok(0);
            }
        }
        3 => read_values(bytes, cursor, base, current_value, 2, operands)?,
        6 | 8 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
        }
        _ => return Err(malformed(base + *cursor, "unsupported address opcode")),
    }
    Ok(u64::from(relative != 1))
}
