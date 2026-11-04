use crate::FormatError;
use crate::leb::{IntegerImmediate, read_integer_immediate, read_uleb};
use crate::types::{BinaryOperand, BinaryType, BinaryTypeTable};

pub(super) const MAX_LIST_COUNT: u64 = 1 << 32;

pub(super) fn read_block_target(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
) -> Result<BinaryOperand, FormatError> {
    let block_offset = *cursor;
    let block = read_index(bytes, cursor, base, "block target")?;
    if block >= block_count {
        return Err(malformed(
            base + block_offset,
            "block target is out of range",
        ));
    }
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "block argument count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + count_offset,
            "block argument count exceeds payload",
        ));
    }
    let arguments = read_value_list(bytes, cursor, base, current_value, count)?;
    Ok(BinaryOperand::BlockTarget { block, arguments })
}

pub(super) fn read_merge_target(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    block_count: u64,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    let value = read_index(bytes, cursor, base, "selection merge block")?;
    if value != 0 && value - 1 >= block_count {
        return Err(malformed(
            base + offset,
            "selection merge block is out of range",
        ));
    }
    Ok(value)
}

pub(super) fn read_values(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    if count == 0 {
        return Ok(());
    }
    let values = read_value_list(bytes, cursor, base, current_value, count)?;
    if count == 1 {
        operands.push(BinaryOperand::Value(values[0]));
    } else {
        operands.push(BinaryOperand::Values(values));
    }
    Ok(())
}

pub(super) fn read_value_list(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    count: u64,
) -> Result<Vec<u64>, FormatError> {
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + *cursor,
            "CODE value count exceeds block payload",
        ));
    }
    let capacity = usize::try_from(count)
        .map_err(|_| malformed(base + *cursor, "CODE value count overflows memory"))?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + *cursor, "CODE value allocation failed"))?;
    for _ in 0..count {
        let offset = *cursor;
        let delta = read_index(bytes, cursor, base, "value delta")?;
        if delta == 0 || delta > current_value {
            return Err(malformed(
                base + offset,
                "value delta does not name an earlier SSA value",
            ));
        }
        values.push(current_value - delta);
    }
    Ok(values)
}

pub(super) fn read_integer(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<IntegerImmediate, FormatError> {
    let offset = *cursor;
    read_integer_immediate(bytes, cursor)
        .map_err(|error| malformed(base + offset, format!("invalid {field}: {error:?}")))
}

pub(super) fn read_type(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor, base, "type index")?;
    let index = usize::try_from(index)
        .map_err(|_| malformed(base + offset, "type index overflows memory"))?;
    if index >= types.entries.len() {
        return Err(malformed(base + offset, "type index is out of range"));
    }
    Ok(index)
}

pub(super) fn read_symbol(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    expected_kind: Option<u8>,
) -> Result<BinaryOperand, FormatError> {
    let offset = *cursor;
    let encoded = read_index(bytes, cursor, base, "symbol index")?;
    let kind = (encoded & 0x03) as u8;
    let index = encoded >> 2;
    if kind == 3 || expected_kind.is_some_and(|expected| expected != kind) {
        return Err(malformed(
            base + offset,
            "symbol kind is invalid for this instruction",
        ));
    }
    Ok(BinaryOperand::Symbol { kind, index })
}

pub(super) fn read_index(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(base + offset, format!("invalid {field} ULEB128: {error:?}")))
}

pub(super) fn read_byte(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(base + offset, format!("truncated {field}")))?;
    *cursor += 1;
    Ok(value)
}

pub(super) fn read_bytes(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    length: usize,
    field: &str,
) -> Result<Vec<u8>, FormatError> {
    let offset = *cursor;
    let end = (*cursor)
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(base + offset, format!("truncated {field}")))?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| malformed(base + offset, format!("{field} allocation failed")))?;
    output.extend_from_slice(&bytes[offset..end]);
    *cursor = end;
    Ok(output)
}

pub(super) fn require_no_modifiers(modifiers: u64, offset: usize) -> Result<(), FormatError> {
    if modifiers == 0 {
        Ok(())
    } else {
        Err(malformed(offset, "instruction does not accept modifiers"))
    }
}

pub(super) fn is_unit(ty: u64, types: &BinaryTypeTable) -> bool {
    usize::try_from(ty)
        .ok()
        .and_then(|index| types.entries.get(index))
        .is_some_and(|ty| matches!(ty, BinaryType::Unit))
}

pub(super) fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
