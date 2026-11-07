use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{malformed, read_index, read_symbol, read_type, read_values};

const ORDER_MASK: u64 = 0x7 << 20;
const SELECTOR_MASK: u64 = 0xf << 48;

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
    match opcode {
        0x250..=0x252 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            Ok(1)
        }
        0x270 => {
            if modifiers & !(ORDER_MASK | SELECTOR_MASK) != 0
                || modifiers & SELECTOR_MASK != 0
                || !matches!((modifiers & ORDER_MASK) >> 20, 0..=2)
            {
                return Err(malformed(base + *cursor, "invalid mmio.load modifiers"));
            }
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            read_mmio_alignment(bytes, cursor, base, operands)?;
            Ok(1)
        }
        0x271 => {
            if modifiers & !(ORDER_MASK | SELECTOR_MASK) != 0
                || modifiers & SELECTOR_MASK != 0
                || !matches!((modifiers & ORDER_MASK) >> 20, 0 | 1 | 3)
            {
                return Err(malformed(base + *cursor, "invalid mmio.store modifiers"));
            }
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            read_mmio_alignment(bytes, cursor, base, operands)?;
            Ok(0)
        }
        0x272 => {
            let selector = (modifiers & SELECTOR_MASK) >> 48;
            if modifiers & !SELECTOR_MASK != 0 || selector > 2 {
                return Err(malformed(base + *cursor, "invalid fence.io selector"));
            }
            Ok(0)
        }
        0x273 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            operands.push(read_symbol(bytes, cursor, base, Some(1))?);
            Ok(1)
        }
        0x280..=0x282 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            let kind = if opcode == 0x281 { 1 } else { 0 };
            operands.push(read_symbol(bytes, cursor, base, Some(kind))?);
            Ok(1)
        }
        _ => Err(malformed(
            base + *cursor,
            "unsupported generic or platform opcode",
        )),
    }
}

fn read_mmio_alignment(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let alignment = read_index(bytes, cursor, base, "MMIO alignment")?;
    if alignment != 0 && !alignment.is_power_of_two() {
        return Err(malformed(
            base + offset,
            "MMIO alignment must be zero or a power of two",
        ));
    }
    operands.push(BinaryOperand::Index(alignment));
    Ok(())
}

fn require_no_modifiers(modifiers: u64, offset: usize) -> Result<(), FormatError> {
    if modifiers == 0 {
        Ok(())
    } else {
        Err(malformed(offset, "instruction does not accept modifiers"))
    }
}
