use tir_format::leb::{IntegerImmediate, read_integer_immediate};

use crate::binary::FormatError;
use crate::{TvisaValueTypeRecord, TypeSection};

use super::super::{BinaryOperand, CallSignatures, read_index};

pub(super) fn read_type(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &TypeSection,
) -> Result<u64, FormatError> {
    let type_index = read_index(bytes, cursor, base, "type index")?;
    if type_index >= types.value_types.len() as u64 {
        return Err(malformed(
            base + *cursor,
            "instruction type index is invalid",
        ));
    }
    Ok(type_index)
}

pub(super) fn read_signature(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &TypeSection,
) -> Result<u64, FormatError> {
    let signature = read_index(bytes, cursor, base, "signature index")?;
    if signature >= types.signatures.len() as u64 {
        return Err(malformed(
            base + *cursor,
            "instruction signature index is invalid",
        ));
    }
    Ok(signature)
}

pub(super) fn scalar_width(types: &TypeSection, type_index: u64) -> Option<u16> {
    match types.value_types.get(type_index as usize)? {
        TvisaValueTypeRecord::Integer { bits } => u16::try_from(*bits).ok(),
        TvisaValueTypeRecord::Float { tag } => Some(match tag {
            0x02 | 0x03 => 16,
            0x04 => 32,
            0x05 => 64,
            0x06 => 128,
            0x07 | 0x08 | 0x0c => 8,
            0x09 | 0x0a => 6,
            0x0b => 4,
            _ => return None,
        }),
        TvisaValueTypeRecord::Pointer { .. } => Some(64),
        _ => None,
    }
}

pub(super) fn vector_storage_size(
    types: &TypeSection,
    lanes: u64,
    element_type: u64,
) -> Option<u64> {
    let width = u64::from(scalar_width(types, element_type)?);
    if width == 1 {
        return Some(lanes);
    }
    if matches!(width, 4 | 6) {
        return lanes.checked_mul(width).map(|bits| bits.div_ceil(8));
    }
    let scalar_bytes = width.div_ceil(8).checked_next_power_of_two()?.min(16);
    let alignment = scalar_bytes
        .checked_mul(lanes.checked_next_power_of_two()?)?
        .min(16);
    let size = lanes.checked_mul(scalar_bytes)?;
    size.checked_add(alignment - 1)
        .map(|rounded| rounded / alignment * alignment)
}

pub(super) fn signature_results(
    types: &TypeSection,
    signature_index: u64,
    offset: usize,
) -> Result<u8, FormatError> {
    let Some(signature) = types.signatures.get(signature_index as usize) else {
        return Err(malformed(offset, "call references an invalid signature"));
    };
    let mut count = 0usize;
    for result in &signature.results {
        count += match result {
            crate::ResultItemSummary::Leaf { .. } => 1,
            crate::ResultItemSummary::Group(group) => group.members.len(),
        };
    }
    count += usize::from(signature.flags & 1 != 0);
    u8::try_from(count).map_err(|_| malformed(offset, "call result count exceeds 255"))
}

pub(super) fn read_call_arguments(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let count = read_index(bytes, cursor, base, "call argument count")?;
    read_values(bytes, cursor, base, current_value, count, operands)
}

pub(super) fn read_integer_list(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    field: &str,
) -> Result<Vec<IntegerImmediate>, FormatError> {
    let count = read_index(bytes, cursor, base, "integer list count")?;
    if count > bytes.len().saturating_sub(*cursor) as u64 / 2 {
        return Err(malformed(
            base + *cursor,
            "integer list count exceeds the function body",
        ));
    }
    let capacity = usize::try_from(count)
        .map_err(|_| malformed(base + *cursor, "integer list count does not fit in memory"))?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + *cursor, "integer list allocation failed"))?;
    for _ in 0..count {
        values.push(read_integer(bytes, cursor, base, field)?);
    }
    Ok(values)
}

pub(super) fn decode_selected_callee(
    selector: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    types: &TypeSection,
    calls: &CallSignatures,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<u64>, FormatError> {
    match selector {
        0 => {
            let symbol = read_index(bytes, cursor, base, "direct call symbol")?;
            let signature = calls.symbol_signature(symbol).ok_or_else(|| {
                malformed(base + start, "direct call symbol has no function signature")
            })?;
            operands.push(BinaryOperand::Symbol(symbol));
            Ok(Some(signature))
        }
        1 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            let signature = read_signature(bytes, cursor, base, types)?;
            operands.push(BinaryOperand::Index(signature));
            operands.push(BinaryOperand::TypeTag(read_u32(bytes, cursor, base)?));
            Ok(Some(signature))
        }
        2 => {
            let id = read_index(bytes, cursor, base, "runtime entry ID")?;
            if id > u64::from(u16::MAX) || runtime_results(id).is_none() {
                return Err(malformed(
                    base + start,
                    "runtime callee has no valid signature",
                ));
            }
            operands.push(BinaryOperand::Index(id));
            Ok(None)
        }
        _ => Err(malformed(base + start, "invalid call callee selector")),
    }
}

pub(super) fn runtime_results(id: u64) -> Option<u8> {
    let count = match id {
        0x0001..=0x0004 | 0x0006 | 0x0008 | 0x000a..=0x000c | 0x0020 | 0x0032 | 0x0040 => 1,
        0x0060..=0x0065 => 2,
        0x0070..=0x0082 | 0x0170..=0x0182 => {
            let base = if id >= 0x0100 { id - 0x0100 } else { id };
            match base {
                0x0076 => 1,
                0x0082 => 3,
                _ => 2,
            }
        }
        0x0088..=0x008b | 0x0090 | 0x0091 | 0x0094 | 0x0096 => 2,
        0x008e | 0x008f => 2,
        0x008c | 0x008d | 0x0092 | 0x0093 | 0x0095 | 0x0097 => 1,
        0x00a0 => 3,
        0x0800..=0x0862 | 0x0880..=0x08e2 => {
            if id & 3 == 2 {
                2
            } else {
                1
            }
        }
        0x0005
        | 0x0007
        | 0x0009
        | 0x000d..=0x0011
        | 0x0021
        | 0x0030..=0x0031
        | 0x0033
        | 0x0041
        | 0x0050..=0x0053
        | 0x00a8
        | 0x00b0 => 0,
        _ => return None,
    };
    Some(count)
}

pub(super) fn read_values(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let capacity = usize::try_from(count)
        .map_err(|_| malformed(base + *cursor, "operand count does not fit in memory"))?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + *cursor, "operand allocation failed"))?;
    for _ in 0..count {
        values.push(read_value(bytes, cursor, base, current_value)?);
    }
    if count == 1 {
        operands.push(BinaryOperand::Value(values[0]));
    } else {
        operands.push(BinaryOperand::Values(values));
    }
    Ok(())
}

pub(super) fn read_value(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
) -> Result<u64, FormatError> {
    let delta = read_index(bytes, cursor, base, "value delta")?;
    if delta == 0 || delta > current_value {
        return Err(malformed(
            base + *cursor,
            "value delta does not name an earlier value",
        ));
    }
    Ok(current_value - delta)
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

pub(super) fn read_block_target(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
) -> Result<BinaryOperand, FormatError> {
    let block = read_index(bytes, cursor, base, "block target")?;
    if block >= block_count {
        return Err(malformed(base + *cursor, "block target index is invalid"));
    }
    let count = read_index(bytes, cursor, base, "block argument count")?;
    let capacity = usize::try_from(count).map_err(|_| {
        malformed(
            base + *cursor,
            "block argument count does not fit in memory",
        )
    })?;
    let mut arguments = Vec::new();
    arguments
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + *cursor, "block argument allocation failed"))?;
    for _ in 0..count {
        arguments.push(read_value(bytes, cursor, base, current_value)?);
    }
    Ok(BinaryOperand::BlockTarget { block, arguments })
}

pub(super) fn into_target(target: BinaryOperand) -> (u64, Vec<u64>) {
    match target {
        BinaryOperand::BlockTarget { block, arguments } => (block, arguments),
        _ => unreachable!(),
    }
}

pub(super) fn read_u32(bytes: &[u8], cursor: &mut usize, base: usize) -> Result<u32, FormatError> {
    let start = *cursor;
    let end = start
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(base + start, "truncated type tag"))?;
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[start..end]);
    *cursor = end;
    Ok(u32::from_le_bytes(value))
}

pub(super) fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
