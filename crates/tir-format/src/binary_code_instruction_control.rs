use crate::FormatError;
use crate::types::BinaryOperand;

use super::{malformed, read_index, read_values};

const MAX_LIST_COUNT: u64 = 1 << 32;

pub(super) fn decode_indirect_or_enum_branch(
    opcode: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    match opcode {
        0x1e3 => decode_switch_enum(bytes, cursor, base, current_value, block_count, operands),
        0x1e4 => decode_br_indirect(bytes, cursor, base, current_value, block_count, operands),
        _ => Err(malformed(base + *cursor, "unsupported control-flow opcode")),
    }
}

fn decode_br_indirect(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    read_values(bytes, cursor, base, current_value, 1, operands)?;
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "indirect branch target count")?;
    if count == 0 || count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            count_offset,
            "indirect branch target count is invalid",
        ));
    }
    let mut targets = Vec::new();
    targets
        .try_reserve_exact(usize::try_from(count).map_err(|_| {
            malformed(
                count_offset,
                "indirect branch target count overflows memory",
            )
        })?)
        .map_err(|_| malformed(base + *cursor, "indirect branch target allocation failed"))?;
    for _ in 0..count {
        let offset = *cursor;
        let target = read_index(bytes, cursor, base, "indirect branch target")?;
        if target >= block_count {
            return Err(malformed(
                base + offset,
                "indirect branch target is out of range",
            ));
        }
        targets.push(target);
    }
    operands.push(BinaryOperand::Indices(targets));
    Ok(())
}

fn decode_switch_enum(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    read_values(bytes, cursor, base, current_value, 1, operands)?;
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "enum switch case count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 / 2 {
        return Err(malformed(
            count_offset,
            "enum switch case count exceeds payload",
        ));
    }
    let mut cases = Vec::new();
    cases
        .try_reserve_exact(
            usize::try_from(count)
                .map_err(|_| malformed(count_offset, "enum switch case count overflows memory"))?,
        )
        .map_err(|_| malformed(base + *cursor, "enum switch case allocation failed"))?;
    for _ in 0..count {
        let variant_offset = *cursor;
        let variant = read_index(bytes, cursor, base, "enum switch variant")?;
        let target_offset = *cursor;
        let target = read_index(bytes, cursor, base, "enum switch target")?;
        if target >= block_count {
            return Err(malformed(
                base + target_offset,
                "enum switch target is out of range",
            ));
        }
        if cases
            .last()
            .is_some_and(|(previous, _)| *previous >= variant)
        {
            return Err(malformed(
                base + variant_offset,
                "enum switch variants are not strictly increasing",
            ));
        }
        cases.push((variant, target));
    }
    operands.push(BinaryOperand::SwitchEnumCases(cases));
    let default_offset = *cursor;
    let default = read_index(bytes, cursor, base, "enum switch default target")?;
    let target = if default == 0 {
        None
    } else {
        let target = default - 1;
        if target >= block_count {
            return Err(malformed(
                base + default_offset,
                "enum switch default is out of range",
            ));
        }
        Some(target)
    };
    operands.push(BinaryOperand::OptionalIndex(target));
    Ok(())
}
