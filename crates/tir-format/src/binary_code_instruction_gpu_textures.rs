use crate::FormatError;
use crate::leb::IntegerImmediate;
use crate::types::BinaryOperand;

use super::helpers::{MAX_LIST_COUNT, malformed, read_index, read_integer, read_value_list};

const SELECTOR_MASK: u64 = 0xf << 48;
const SPARSE: u64 = 1 << 35;
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
    if !(0x480..=0x48d).contains(&opcode) {
        return Ok(None);
    }
    let selector = (modifiers & SELECTOR_MASK) >> 48;
    let selector_valid = match opcode {
        0x486 => selector <= 3,
        0x48b => selector <= 6,
        _ => selector == 0,
    };
    let sparse = modifiers & SPARSE != 0;
    let unchecked_uniformity = modifiers & UNCHECKED_UNIFORMITY != 0;
    if !selector_valid
        || (modifiers & !(SELECTOR_MASK | SPARSE | UNCHECKED_UNIFORMITY) != 0)
        || sparse && !supports_sparse(opcode)
        || unchecked_uniformity && !requires_quad_uniformity(opcode)
    {
        return Err(malformed(
            base + start,
            "invalid texture instruction modifiers",
        ));
    }
    let value_count_offset = *cursor;
    let value_count = read_index(bytes, cursor, base, "texture value count")?;
    if value_count == 0
        || value_count >= MAX_LIST_COUNT
        || value_count > bytes.len().saturating_sub(*cursor) as u64
    {
        return Err(malformed(
            base + value_count_offset,
            "texture value count is invalid",
        ));
    }
    operands.push(BinaryOperand::Values(read_value_list(
        bytes,
        cursor,
        base,
        current_value,
        value_count,
    )?));
    let offset_count_offset = *cursor;
    let offset_count = read_index(bytes, cursor, base, "texture offset component count")?;
    if offset_count > 3 || offset_count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + offset_count_offset,
            "texture offset component count is invalid",
        ));
    }
    let capacity = usize::try_from(offset_count).map_err(|_| {
        malformed(
            base + offset_count_offset,
            "texture offset count overflows memory",
        )
    })?;
    let mut offsets: Vec<IntegerImmediate> = Vec::new();
    offsets.try_reserve_exact(capacity).map_err(|_| {
        malformed(
            base + offset_count_offset,
            "texture offset allocation failed",
        )
    })?;
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
    Ok(Some((
        if opcode == 0x489 {
            0
        } else {
            1 + u64::from(sparse)
        },
        false,
    )))
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
