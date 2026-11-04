use crate::binary::FormatError;

use super::BinaryOperand;
use super::helpers::{malformed, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

pub(super) fn decode(
    opcode: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    modifiers: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u8, bool)>, FormatError> {
    if matches!(opcode, 0x4c0..=0x4c4 | 0x5a0) {
        let allowed = if matches!(opcode, 0x4c0 | 0x4c1) {
            UNCHECKED_UNIFORMITY
        } else {
            0
        };
        if modifiers & !allowed != 0 {
            return Err(malformed(
                base + start,
                "GPU geometry instruction has unsupported modifiers",
            ));
        }
    }
    let (value_count, result_count) = match opcode {
        0x4c0 => (3, 0),
        0x4c1 => (2, 0),
        0x4c2..=0x4c4 => (2, 0),
        0x5a0 => (1, 1),
        0x5c0 => (1, 1),
        0x5c1 => (0, 1),
        0x5c2 => {
            if modifiers & !SELECTOR_MASK != 0 || (modifiers >> 48) & 0xf > 1 {
                return Err(malformed(base + start, "invalid node output launch mode"));
            }
            (2, 1)
        }
        0x5c3 => (2, 1),
        0x5c4 => (1, 0),
        0x5c5 => (0, 1),
        _ => return Ok(None),
    };
    if matches!(opcode, 0x5c0 | 0x5c1 | 0x5c3..=0x5c5) && modifiers != 0 {
        return Err(malformed(
            base + start,
            "GPU node instruction has unsupported modifiers",
        ));
    }
    read_values(bytes, cursor, base, current_value, value_count, operands)?;
    Ok(Some((result_count, false)))
}
