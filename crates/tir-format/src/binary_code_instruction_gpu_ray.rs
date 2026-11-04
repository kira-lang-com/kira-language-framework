use crate::FormatError;
use crate::types::BinaryOperand;

use super::helpers::{malformed, read_index, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;

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
    if !(0x500..=0x511).contains(&opcode) {
        return Ok(None);
    }
    if opcode == 0x505 {
        if modifiers & !SELECTOR_MASK != 0 || ((modifiers & SELECTOR_MASK) >> 48) > 1 {
            return Err(malformed(
                base + start,
                "invalid ray-query intersection selection",
            ));
        }
    } else if modifiers != 0 {
        return Err(malformed(
            base + start,
            "ray instruction has unsupported modifiers",
        ));
    }
    let (count, result_count, terminator) = match opcode {
        0x500 => (8, 0, false),
        0x501 => (1, 1, false),
        0x502 | 0x504 => (1, 0, false),
        0x503 => (2, 0, false),
        0x505 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let attribute = read_attribute(bytes, cursor, base, operands, 12, "ray-query")?;
            return Ok(Some((if attribute >= 10 { 12 } else { 1 }, false)));
        }
        0x506 => (11, 0, false),
        0x507 => (3, 1, false),
        0x508 | 0x509 => (0, 0, true),
        0x50a => (2, 0, false),
        0x50b => {
            let attribute = read_attribute(bytes, cursor, base, operands, 17, "ray")?;
            return Ok(Some((
                match attribute {
                    14 | 15 => 12,
                    16 => 9,
                    _ => 1,
                },
                false,
            )));
        }
        0x50c => (10, 1, false),
        0x50d | 0x50e => (u64::from(opcode == 0x50d), 1, false),
        0x50f => (2, 0, false),
        0x510 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let attribute = read_attribute(bytes, cursor, base, operands, 12, "hit-object")?;
            return Ok(Some((if attribute >= 10 { 12 } else { 1 }, false)));
        }
        0x511 => {
            let count_offset = *cursor;
            let count = read_index(bytes, cursor, base, "ray reorder value count")?;
            if !matches!(count, 1 | 2) {
                return Err(malformed(
                    base + count_offset,
                    "ray reorder must carry a hitobject and optional key",
                ));
            }
            read_values(bytes, cursor, base, current_value, count, operands)?;
            let width_offset = *cursor;
            let width = read_index(bytes, cursor, base, "ray reorder key width")?;
            if (count == 1 && width != 0) || (count == 2 && !(1..=64).contains(&width)) {
                return Err(malformed(
                    base + width_offset,
                    "ray reorder key width must be omitted or between 1 and 64",
                ));
            }
            operands.push(BinaryOperand::Index(width));
            return Ok(Some((0, false)));
        }
        _ => unreachable!(),
    };
    read_values(bytes, cursor, base, current_value, count, operands)?;
    Ok(Some((result_count, terminator)))
}

fn read_attribute(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    operands: &mut Vec<BinaryOperand>,
    count: u64,
    label: &str,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    let attribute = read_index(bytes, cursor, base, "ray attribute code")?;
    if attribute >= count {
        return Err(malformed(
            base + offset,
            format!("invalid {label} attribute code"),
        ));
    }
    operands.push(BinaryOperand::Index(attribute));
    Ok(attribute)
}
