use crate::binary::FormatError;

use super::helpers::{malformed, read_integer, read_type, read_values};
use super::{BinaryOperand, read_index};

pub(super) fn decode(
    opcode: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    modifiers: u64,
    types: &crate::TypeSection,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u8, bool)>, FormatError> {
    if !(0x560..=0x56b).contains(&opcode) {
        return Ok(None);
    }
    validate_modifiers(opcode, modifiers, base + start)?;
    let result_count = if matches!(opcode, 0x562 | 0x56a) {
        0
    } else {
        1
    };
    match opcode {
        0x560 => {
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
        }
        0x561 => {
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            read_values(bytes, cursor, base, current_value, 2, operands)?;
        }
        0x562 | 0x563 | 0x567 | 0x56a => {
            read_values(bytes, cursor, base, current_value, 3, operands)?;
        }
        0x564 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
        }
        0x565 => operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?)),
        0x566 | 0x569 => read_values(bytes, cursor, base, current_value, 2, operands)?,
        0x568 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let operation = read_index(bytes, cursor, base, "cooperative reduction operator")?;
            if operation > 14 {
                return Err(malformed(
                    base + start,
                    "invalid cooperative reduction operator",
                ));
            }
            operands.push(BinaryOperand::Index(operation));
        }
        0x56b => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let dimension = read_integer(bytes, cursor, base, "tensor query dimension")?;
            if dimension.negative || dimension.magnitude > 7 {
                return Err(malformed(
                    base + start,
                    "tensor query dimension must be in 0…7",
                ));
            }
            operands.push(BinaryOperand::Integer(dimension));
        }
        _ => unreachable!(),
    }
    Ok(Some((result_count, false)))
}

fn validate_modifiers(opcode: u64, modifiers: u64, offset: usize) -> Result<(), FormatError> {
    let selector = (modifiers >> 48) & 0xf;
    let selector_valid = match opcode {
        0x561 | 0x562 => selector <= 1,
        0x568 => selector <= 2,
        0x56b => selector <= 1,
        _ => selector == 0,
    };
    let mut allowed = if matches!(opcode, 0x561 | 0x562 | 0x568 | 0x56b) {
        0xf << 48
    } else {
        0
    };
    if opcode == 0x563 {
        allowed |= (1 << 2) | (1 << 46);
    }
    if !selector_valid || modifiers & !allowed != 0 {
        return Err(malformed(
            offset,
            "invalid cooperative or tensor instruction modifiers",
        ));
    }
    Ok(())
}
