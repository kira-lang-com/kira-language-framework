use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_index, read_type, read_values};

const SELECTOR_MASK: u64 = 0xf << 48;
const INBOUNDS: u64 = 1 << 5;
const VOLATILE: u64 = 1 << 18;
const NONTEMPORAL: u64 = 1 << 19;
const NOOVERLAP: u64 = 1 << 31;

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
    let relative = opcode - 0x140;
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    let allowed = match relative {
        0 | 2 => SELECTOR_MASK,
        3 | 4 => SELECTOR_MASK,
        6 | 7 => VOLATILE | NONTEMPORAL,
        8 => INBOUNDS,
        10 => VOLATILE | NOOVERLAP,
        11 | 12 => VOLATILE,
        13 => 0x7 | SELECTOR_MASK,
        _ => 0,
    };
    if modifiers & !(allowed | SELECTOR_MASK) != 0 {
        return Err(malformed(
            base + *cursor,
            "invalid memory instruction modifiers",
        ));
    }
    match relative {
        0 => {
            if selector > 2 {
                return Err(malformed(base + *cursor, "invalid load ownership selector"));
            }
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok(1)
        }
        1 | 5 => {
            if selector != 0 {
                return Err(malformed(base + *cursor, "unexpected memory selector"));
            }
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok(u64::from(relative == 1))
        }
        2 => {
            if selector > 2 {
                return Err(malformed(base + *cursor, "invalid store mode selector"));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            Ok(0)
        }
        3 | 4 => {
            if selector > 1 {
                return Err(malformed(
                    base + *cursor,
                    "invalid address copy mode selector",
                ));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            Ok(0)
        }
        6 => {
            if selector != 0 {
                return Err(malformed(base + *cursor, "unexpected raw.load selector"));
            }
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            read_alignment(bytes, cursor, base, operands)?;
            Ok(1)
        }
        7 => {
            if selector != 0 {
                return Err(malformed(base + *cursor, "unexpected raw.store selector"));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            read_alignment(bytes, cursor, base, operands)?;
            Ok(0)
        }
        8 | 9 => {
            if selector != 0 {
                return Err(malformed(
                    base + *cursor,
                    "unexpected pointer arithmetic selector",
                ));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            Ok(1)
        }
        10..=12 => {
            if selector != 0 {
                return Err(malformed(
                    base + *cursor,
                    "unexpected memory range selector",
                ));
            }
            read_values(bytes, cursor, base, current_value, 3, operands)?;
            Ok(0)
        }
        13 => {
            if selector > 1 {
                return Err(malformed(
                    base + *cursor,
                    "invalid prefetch access selector",
                ));
            }
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok(0)
        }
        _ => Err(malformed(base + *cursor, "unsupported memory opcode")),
    }
}

fn read_alignment(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let alignment = read_index(bytes, cursor, base, "memory alignment")?;
    if alignment != 0 && !alignment.is_power_of_two() {
        return Err(malformed(
            base + offset,
            "memory alignment must be zero or a power of two",
        ));
    }
    operands.push(BinaryOperand::Index(alignment));
    Ok(())
}
