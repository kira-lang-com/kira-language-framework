use crate::binary::FormatError;

use super::helpers::{malformed, read_values};
use super::{BinaryOperand, read_index};

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
    if !(0x500..=0x511).contains(&opcode) {
        return Ok(None);
    }

    let (value_count, terminator) = match opcode {
        0x500 => (8, false),
        0x501 | 0x502 | 0x504 => (1, false),
        0x503 => (2, false),
        0x505 => {
            validate_query_selection(modifiers, base + start)?;
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let attribute = read_attribute(bytes, cursor, base, start, operands, 12, "ray-query")?;
            return Ok(Some((if attribute >= 10 { 12 } else { 1 }, false)));
        }
        0x506 => (11, false),
        0x507 => (3, false),
        0x508 | 0x509 => (0, true),
        0x50a => (2, false),
        0x50b => {
            let code = read_attribute(bytes, cursor, base, start, operands, 17, "ray")?;
            let results = match code {
                14 | 15 => 12,
                16 => 9,
                _ => 1,
            };
            return Ok(Some((results, false)));
        }
        0x50c => (10, false),
        0x50d => (1, false),
        0x50e => (0, false),
        0x50f => (2, false),
        0x510 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let code = read_attribute(bytes, cursor, base, start, operands, 12, "hit-object")?;
            return Ok(Some((if code >= 10 { 12 } else { 1 }, false)));
        }
        0x511 => {
            let count = read_index(bytes, cursor, base, "ray reorder value count")?;
            if !matches!(count, 1 | 2) {
                return Err(malformed(
                    base + start,
                    "ray reorder must carry a hitobject and optional key",
                ));
            }
            read_values(bytes, cursor, base, current_value, count, operands)?;
            let width = read_index(bytes, cursor, base, "ray reorder key width")?;
            if (count == 1 && width != 0) || (count == 2 && !(1..=64).contains(&width)) {
                return Err(malformed(
                    base + start,
                    "ray reorder key width must be omitted or between 1 and 64",
                ));
            }
            operands.push(BinaryOperand::Index(width));
            return Ok(Some((0, false)));
        }
        _ => unreachable!(),
    };

    if value_count != 0 {
        read_values(bytes, cursor, base, current_value, value_count, operands)?;
    }
    let result_count = match opcode {
        0x501 | 0x507 => 1,
        0x50c | 0x50d | 0x50e => 1,
        _ => 0,
    };
    Ok(Some((result_count, terminator)))
}

fn validate_query_selection(modifiers: u64, offset: usize) -> Result<(), FormatError> {
    if (modifiers >> 48) & 0xf > 1 {
        return Err(malformed(
            offset,
            "invalid ray-query intersection selection",
        ));
    }
    Ok(())
}

fn read_attribute(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    operands: &mut Vec<BinaryOperand>,
    count: u64,
    description: &str,
) -> Result<u64, FormatError> {
    let attribute = read_index(bytes, cursor, base, "ray attribute code")?;
    if attribute >= count {
        return Err(malformed(
            base + start,
            format!("invalid {description} attribute code"),
        ));
    }
    operands.push(BinaryOperand::Index(attribute));
    Ok(attribute)
}
