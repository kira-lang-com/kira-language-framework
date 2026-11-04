use crate::FormatError;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{
    MAX_LIST_COUNT, malformed, read_index, read_symbol, read_type, read_values,
    require_no_modifiers,
};

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
) -> Result<(u64, bool), FormatError> {
    require_no_modifiers(modifiers, base + *cursor)?;
    match opcode {
        0x1d0 => {
            decode_landingpad(bytes, cursor, base, types, operands)?;
            Ok((1, false))
        }
        0x1d1 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok((0, true))
        }
        0x1d2 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            Ok((0, true))
        }
        0x1d3 | 0x1d4 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok((1, false))
        }
        _ => Err(malformed(base + *cursor, "unsupported unwinding opcode")),
    }
}

fn decode_landingpad(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "landingpad clause count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + count_offset,
            "landingpad clause count exceeds payload",
        ));
    }
    let capacity = usize::try_from(count).map_err(|_| {
        malformed(
            base + count_offset,
            "landingpad clause count overflows memory",
        )
    })?;
    let mut clauses = Vec::new();
    clauses
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + *cursor, "landingpad clause allocation failed"))?;
    for _ in 0..count {
        let kind_offset = *cursor;
        let kind = *bytes
            .get(*cursor)
            .ok_or_else(|| malformed(base + *cursor, "truncated landingpad clause kind"))?;
        *cursor += 1;
        let item = match kind {
            0 => read_type(bytes, cursor, base, types)? as u64,
            1 => match read_symbol(bytes, cursor, base, Some(0))? {
                BinaryOperand::Symbol { index, .. } => index,
                _ => unreachable!(),
            },
            2 | 3 => 0,
            _ => {
                return Err(malformed(
                    base + kind_offset,
                    "invalid landingpad clause kind",
                ));
            }
        };
        clauses.push((kind, item));
    }
    operands.push(BinaryOperand::Clauses(clauses));
    Ok(())
}
