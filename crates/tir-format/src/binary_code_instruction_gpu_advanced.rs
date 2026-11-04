use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::helpers::{malformed, read_index, read_integer, read_type, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;

pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u64, bool)>, FormatError> {
    match opcode {
        0x560..=0x56b => decode_cooperative(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            types,
            operands,
        ),
        0x5a0 => {
            require_no_modifiers(modifiers, base + start)?;
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok(Some((1, false)))
        }
        _ => Ok(None),
    }
}

fn decode_cooperative(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u64, bool)>, FormatError> {
    validate_modifiers(opcode, modifiers, base + start)?;
    let result_count = if matches!(opcode, 0x562 | 0x56a) {
        0
    } else {
        1
    };
    match opcode {
        0x560 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
        }
        0x561 => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_values(bytes, cursor, base, current_value, 2, operands)?;
        }
        0x562 | 0x563 | 0x567 | 0x56a => {
            read_values(bytes, cursor, base, current_value, 3, operands)?
        }
        0x564 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
        }
        0x565 => operands.push(BinaryOperand::Type(
            read_type(bytes, cursor, base, types)? as u64
        )),
        0x566 | 0x569 => read_values(bytes, cursor, base, current_value, 2, operands)?,
        0x568 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let operation_offset = *cursor;
            let operation = read_index(bytes, cursor, base, "cooperative reduction operator")?;
            if operation > 14 {
                return Err(malformed(
                    base + operation_offset,
                    "invalid cooperative reduction operator",
                ));
            }
            operands.push(BinaryOperand::Index(operation));
        }
        0x56b => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let dimension_offset = *cursor;
            let dimension = read_integer(bytes, cursor, base, "tensor query dimension")?;
            if dimension.negative || dimension.magnitude > 7 {
                return Err(malformed(
                    base + dimension_offset,
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
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    let selector_valid = match opcode {
        0x561 | 0x562 => selector <= 1,
        0x568 => selector <= 2,
        0x56b => selector <= 1,
        _ => selector == 0,
    };
    let allowed = if matches!(opcode, 0x561 | 0x562 | 0x568 | 0x56b) {
        SELECTOR_MASK
    } else if opcode == 0x563 {
        (1 << 2) | (1 << 46)
    } else {
        0
    };
    if !selector_valid || modifiers & !allowed != 0 {
        return Err(malformed(
            offset,
            "invalid cooperative or tensor instruction modifiers",
        ));
    }
    Ok(())
}

fn require_no_modifiers(modifiers: u64, offset: usize) -> Result<(), FormatError> {
    if modifiers == 0 {
        Ok(())
    } else {
        Err(malformed(
            offset,
            "GPU tile instruction does not accept modifiers",
        ))
    }
}
