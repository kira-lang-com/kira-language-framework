use crate::FormatError;
use crate::types::BinaryOperand;

use super::helpers::{malformed, read_index, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const FINE: u64 = 1 << 36;
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
        0x420..=0x426 => decode_quad(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            operands,
        ),
        0x440..=0x452 => decode_subgroup(
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

fn decode_quad(
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
        0x420 | 0x421 => {
            if modifiers & !(FINE | UNCHECKED_UNIFORMITY) != 0 {
                return Err(malformed(
                    base + start,
                    "derivative instruction has unsupported modifiers",
                ));
            }
            read_values(bytes, cursor, base, current_value, 1, operands)?;
        }
        0x422 | 0x425 | 0x426 => {
            if modifiers & !UNCHECKED_UNIFORMITY != 0 {
                return Err(malformed(
                    base + start,
                    "quad instruction has unsupported modifiers",
                ));
            }
            read_values(bytes, cursor, base, current_value, 1, operands)?;
        }
        0x423 => {
            if modifiers & !UNCHECKED_UNIFORMITY != 0 {
                return Err(malformed(
                    base + start,
                    "quad.broadcast has unsupported modifiers",
                ));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
        }
        0x424 => {
            if modifiers & !(SELECTOR_MASK | UNCHECKED_UNIFORMITY) != 0
                || ((modifiers & SELECTOR_MASK) >> 48) > 2
            {
                return Err(malformed(
                    base + start,
                    "invalid quad swap selector or modifiers",
                ));
            }
            read_values(bytes, cursor, base, current_value, 1, operands)?;
        }
        _ => return Ok(None),
    }
    Ok(Some((1, false)))
}

fn decode_subgroup(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u64, bool)>, FormatError> {
    if modifiers != 0 {
        return Err(malformed(
            base + start,
            "subgroup instruction has unsupported modifiers",
        ));
    }
    let value_count = match opcode {
        0x440 => 0,
        0x444 | 0x44b..=0x44f => 2,
        _ => 1,
    };
    read_values(bytes, cursor, base, current_value, value_count, operands)?;
    if matches!(opcode, 0x450..=0x452) {
        let operator = read_index(bytes, cursor, base, "subgroup reduction operator")?;
        if operator > 14 {
            return Err(malformed(
                base + start,
                "invalid subgroup reduction operator",
            ));
        }
        operands.push(BinaryOperand::Index(operator));
        let cluster = read_index(bytes, cursor, base, "subgroup cluster size")?;
        if cluster != 0 && (cluster > 128 || !cluster.is_power_of_two()) {
            return Err(malformed(
                base + start,
                "subgroup cluster size must be a power of two no greater than 128",
            ));
        }
        operands.push(BinaryOperand::Index(cluster));
    }
    Ok(Some((1, false)))
}
