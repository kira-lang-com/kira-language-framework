use crate::FormatError;
use crate::strings::StringTable;
use crate::types::{BinaryOperand, BinaryTypeTable};

use super::{MAX_LIST_COUNT, malformed, read_index, read_symbol, read_type, require_no_modifiers};

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    block_count: u64,
    types: &BinaryTypeTable,
    strings: &StringTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<u64, FormatError> {
    require_no_modifiers(modifiers, base + start)?;
    match opcode {
        0x00a => {
            operands.push(read_symbol(bytes, cursor, base, Some(0))?);
            read_type_arguments(bytes, cursor, base, types, operands)?;
        }
        0x00b | 0x00c => operands.push(read_symbol(bytes, cursor, base, Some(1))?),
        0x00d => operands.push(BinaryOperand::Type(
            read_type(bytes, cursor, base, types)? as u64
        )),
        0x00e => {
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            let offset = *cursor;
            let name = read_index(bytes, cursor, base, "witness trait name")?;
            if usize::try_from(name)
                .ok()
                .and_then(|index| strings.get(index))
                .is_none()
            {
                return Err(malformed(
                    base + offset,
                    "witness trait name is out of range",
                ));
            }
            operands.push(BinaryOperand::Index(name));
        }
        0x00f => {
            let offset = *cursor;
            let block = read_index(bytes, cursor, base, "block address target")?;
            if block >= block_count {
                return Err(malformed(
                    base + offset,
                    "block address target is out of range",
                ));
            }
            operands.push(BinaryOperand::Index(block));
        }
        0x010 => operands.push(read_symbol(bytes, cursor, base, Some(0))?),
        _ => {
            return Err(malformed(
                base + start,
                "unsupported symbol-reference opcode",
            ));
        }
    }
    Ok(1)
}

fn read_type_arguments(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let count = read_index(
        bytes,
        cursor,
        base,
        "function-reference type-argument count",
    )?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + offset,
            "function-reference type arguments exceed payload",
        ));
    }
    let capacity = usize::try_from(count).map_err(|_| {
        malformed(
            base + offset,
            "function-reference type count overflows memory",
        )
    })?;
    let mut arguments = Vec::new();
    arguments
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(base + *cursor, "function-reference type allocation failed"))?;
    for _ in 0..count {
        arguments.push(read_type(bytes, cursor, base, types)? as u64);
    }
    operands.push(BinaryOperand::Types(arguments));
    Ok(())
}
