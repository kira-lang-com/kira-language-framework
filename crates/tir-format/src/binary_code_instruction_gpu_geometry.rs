use crate::FormatError;
use crate::types::BinaryOperand;

use super::helpers::{malformed, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u64, bool)>, FormatError> {
    match opcode {
        0x4c0..=0x4c4 => {
            let allowed = if matches!(opcode, 0x4c0 | 0x4c1) {
                UNCHECKED_UNIFORMITY
            } else {
                0
            };
            if modifiers & !allowed != 0 {
                return Err(malformed(
                    base + start,
                    "mesh instruction has unsupported modifiers",
                ));
            }
            let value_count = if opcode == 0x4c0 { 3 } else { 2 };
            read_values(bytes, cursor, base, current_value, value_count, operands)?;
            Ok(Some((0, false)))
        }
        0x5c0..=0x5c5 => decode_node(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            operands,
        ),
        _ => Ok(None),
    }
}

fn decode_node(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u64, bool)>, FormatError> {
    let (count, results) = match opcode {
        0x5c0 => (1, 1),
        0x5c1 => (0, 1),
        0x5c2 => {
            if modifiers & !SELECTOR_MASK != 0 || ((modifiers & SELECTOR_MASK) >> 48) > 1 {
                return Err(malformed(
                    base + start,
                    "invalid node output allocation modifiers",
                ));
            }
            (2, 1)
        }
        0x5c3 => (2, 1),
        0x5c4 => (1, 0),
        0x5c5 => (0, 1),
        _ => return Ok(None),
    };
    if opcode != 0x5c2 && modifiers != 0 {
        return Err(malformed(
            base + start,
            "node instruction has unsupported modifiers",
        ));
    }
    read_values(bytes, cursor, base, current_value, count, operands)?;
    Ok(Some((results, false)))
}
