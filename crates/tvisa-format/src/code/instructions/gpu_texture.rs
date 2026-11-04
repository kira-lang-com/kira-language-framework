use crate::binary::FormatError;

use super::helpers::{malformed, read_integer, read_values};
use super::{BinaryOperand, read_index};

const SELECTOR_MASK: u64 = 0xf << 48;
const SPARSE: u64 = 1 << 35;
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
    if !(0x480..=0x48d).contains(&opcode) {
        return Ok(None);
    }
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    if (opcode == 0x48b && selector > 6) || (opcode == 0x486 && selector > 3) {
        return Err(malformed(
            base + start,
            "invalid texture instruction selector",
        ));
    }
    if !matches!(opcode, 0x486 | 0x48b) && selector != 0 {
        return Err(malformed(
            base + start,
            "texture instruction has an unsupported selector",
        ));
    }

    let sparse = modifiers & SPARSE != 0;
    let unchecked_uniformity = modifiers & UNCHECKED_UNIFORMITY != 0;
    if modifiers & !(SELECTOR_MASK | SPARSE | UNCHECKED_UNIFORMITY) != 0
        || unchecked_uniformity && !requires_quad_uniformity(opcode)
    {
        return Err(malformed(
            base + start,
            "texture instruction has unsupported modifiers",
        ));
    }
    if sparse && !supports_sparse(opcode) {
        return Err(malformed(
            base + start,
            "sparse modifier is not supported by this texture instruction",
        ));
    }
    let value_count = read_index(bytes, cursor, base, "texture value count")?;
    if value_count > 16 {
        return Err(malformed(base + start, "texture value count exceeds 16"));
    }
    read_values(bytes, cursor, base, current_value, value_count, operands)?;

    let offset_count = read_index(bytes, cursor, base, "texture offset component count")?;
    if offset_count > 3 {
        return Err(malformed(
            base + start,
            "texture offset has more than three components",
        ));
    }
    let mut offsets = Vec::with_capacity(offset_count as usize);
    for _ in 0..offset_count {
        let offset = read_integer(bytes, cursor, base, "texture offset")?;
        if (offset.negative && offset.magnitude > 8) || (!offset.negative && offset.magnitude > 7) {
            return Err(malformed(
                base + start,
                "texture offset component is outside −8…7",
            ));
        }
        offsets.push(offset);
    }
    operands.push(BinaryOperand::Integers(offsets));

    if opcode == 0x48a {
        let operation = read_index(bytes, cursor, base, "texture atomic operation")?;
        if operation > 14 {
            return Err(malformed(base + start, "invalid texture atomic operation"));
        }
        operands.push(BinaryOperand::Index(operation));
    }
    let results = if opcode == 0x489 {
        0
    } else {
        1 + u8::from(sparse)
    };
    Ok(Some((results, false)))
}

fn supports_sparse(opcode: u64) -> bool {
    matches!(opcode, 0x480..=0x488 | 0x48d)
}

fn requires_quad_uniformity(opcode: u64) -> bool {
    matches!(
        opcode,
        0x480 | 0x483 | 0x484 | 0x486 | 0x487 | 0x48c | 0x48d
    )
}
