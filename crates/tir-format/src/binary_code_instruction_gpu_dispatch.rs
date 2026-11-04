use crate::FormatError;
use crate::strings::StringTable;
use crate::types::{BinaryConstantTable, BinaryOperand, BinaryTypeTable};

use super::{gpu_advanced, gpu_collectives, gpu_control, gpu_geometry, gpu_ray, gpu_textures};

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u64, bool)>, FormatError> {
    match opcode {
        0x400..=0x40a => gpu_control::decode(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            types,
            constants,
            strings,
            operands,
        ),
        0x420..=0x426 | 0x440..=0x452 => gpu_collectives::decode(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            operands,
        ),
        0x480..=0x48d => gpu_textures::decode(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            operands,
        ),
        0x4c0..=0x4c4 | 0x5c0..=0x5c5 => gpu_geometry::decode(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            operands,
        ),
        0x500..=0x511 => gpu_ray::decode(
            opcode,
            modifiers,
            bytes,
            cursor,
            base,
            start,
            current_value,
            operands,
        ),
        0x560..=0x56b | 0x5a0 => gpu_advanced::decode(
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
        _ => Ok(None),
    }
}
