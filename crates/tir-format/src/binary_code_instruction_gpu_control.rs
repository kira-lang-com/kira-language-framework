use crate::FormatError;
use crate::strings::StringTable;
use crate::types::{BinaryConstantTable, BinaryOperand, BinaryType, BinaryTypeTable};

use super::helpers::{
    MAX_LIST_COUNT, is_unit, malformed, read_index, read_integer, read_type, read_value_list,
    read_values,
};

const SCOPE_MASK: u64 = 7 << 26;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

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
    let result = match opcode {
        0x400 => {
            require_modifiers(modifiers, 0, base + start)?;
            let name_offset = *cursor;
            let name = read_index(bytes, cursor, base, "GPU builtin name")?;
            if usize::try_from(name)
                .ok()
                .and_then(|index| strings.get(index))
                .is_none()
            {
                return Err(malformed(
                    base + name_offset,
                    "GPU builtin name is out of range",
                ));
            }
            operands.push(BinaryOperand::Index(name));
            operands.push(BinaryOperand::Type(
                read_type(bytes, cursor, base, types)? as u64
            ));
            (1, false)
        }
        0x401 => {
            require_modifiers(modifiers, 0, base + start)?;
            let id_offset = *cursor;
            let id = read_index(bytes, cursor, base, "GPU specialization constant ID")?;
            if id > u64::from(u32::MAX) {
                return Err(malformed(
                    base + id_offset,
                    "GPU specialization constant ID exceeds 32 bits",
                ));
            }
            let ty = read_type(bytes, cursor, base, types)? as u64;
            let constant_offset = *cursor;
            let constant = read_index(bytes, cursor, base, "GPU specialization default constant")?;
            let Some(record) = usize::try_from(constant)
                .ok()
                .and_then(|index| constants.records.get(index))
            else {
                return Err(malformed(
                    base + constant_offset,
                    "GPU specialization default is out of range",
                ));
            };
            if record.ty != ty {
                return Err(malformed(
                    base + constant_offset,
                    "GPU specialization default has the wrong type",
                ));
            }
            operands.push(BinaryOperand::Index(id));
            operands.push(BinaryOperand::Type(ty));
            operands.push(BinaryOperand::Index(constant));
            (1, false)
        }
        0x402 | 0x403 => {
            let scope = (modifiers & SCOPE_MASK) >> 26;
            let valid_scope = if opcode == 0x402 {
                matches!(scope, 0 | 2 | 3)
            } else {
                scope <= 5
            };
            let allowed = SCOPE_MASK
                | if opcode == 0x402 {
                    UNCHECKED_UNIFORMITY
                } else {
                    0
                };
            if !valid_scope || modifiers & !allowed != 0 {
                return Err(malformed(
                    base + start,
                    "invalid GPU synchronization modifiers",
                ));
            }
            let mask = super::helpers::read_byte(bytes, cursor, base, "GPU memory-domain mask")?;
            if mask & !0x1f != 0 {
                return Err(malformed(
                    base + *cursor - 1,
                    "reserved GPU memory-domain bits are set",
                ));
            }
            operands.push(BinaryOperand::Index(u64::from(mask)));
            (0, false)
        }
        0x404 => {
            require_modifiers(modifiers, 0, base + start)?;
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            let function_type = read_type(bytes, cursor, base, types)? as u64;
            let Some(BinaryType::Function(signature)) = usize::try_from(function_type)
                .ok()
                .and_then(|index| types.entries.get(index))
            else {
                return Err(malformed(
                    base + start,
                    "GPU visible-function call type is not a function",
                ));
            };
            operands.push(BinaryOperand::Type(function_type));
            let count_offset = *cursor;
            let count = read_index(bytes, cursor, base, "GPU visible-function argument count")?;
            if count >= MAX_LIST_COUNT
                || usize::try_from(count).ok() != Some(signature.parameters.len())
            {
                return Err(malformed(
                    base + count_offset,
                    "GPU visible-function argument count disagrees with its type",
                ));
            }
            operands.push(BinaryOperand::Values(read_value_list(
                bytes,
                cursor,
                base,
                current_value,
                count,
            )?));
            let expected = u64::from(
                !is_unit(signature.result, types)
                    && !matches!(
                        types.entries.get(signature.result as usize),
                        Some(BinaryType::Never)
                    ),
            );
            (expected, false)
        }
        0x405 | 0x407 | 0x408 => {
            require_modifiers(modifiers, 0, base + start)?;
            (0, false)
        }
        0x406 => {
            require_modifiers(modifiers, 0, base + start)?;
            let attachment_offset = *cursor;
            let attachment = read_integer(bytes, cursor, base, "fragment attachment index")?;
            if attachment.negative || attachment.magnitude > u128::from(u32::MAX) {
                return Err(malformed(
                    base + attachment_offset,
                    "fragment attachment index must be an unsigned 32-bit value",
                ));
            }
            operands.push(BinaryOperand::Integer(attachment));
            (1, false)
        }
        0x409 => {
            require_modifiers(modifiers, 0, base + start)?;
            (1, false)
        }
        0x40a => {
            require_modifiers(modifiers, 0, base + start)?;
            let format_offset = *cursor;
            let format = read_index(bytes, cursor, base, "GPU printf format string")?;
            if usize::try_from(format)
                .ok()
                .and_then(|index| strings.get(index))
                .is_none()
            {
                return Err(malformed(
                    base + format_offset,
                    "GPU printf string is out of range",
                ));
            }
            operands.push(BinaryOperand::Index(format));
            read_argument_list(bytes, cursor, base, current_value, operands)?;
            (0, false)
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

fn read_argument_list(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let count_offset = *cursor;
    let count = read_index(bytes, cursor, base, "GPU printf argument count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + count_offset,
            "GPU printf argument count exceeds payload",
        ));
    }
    operands.push(BinaryOperand::Values(read_value_list(
        bytes,
        cursor,
        base,
        current_value,
        count,
    )?));
    Ok(())
}

fn require_modifiers(modifiers: u64, allowed: u64, offset: usize) -> Result<(), FormatError> {
    if modifiers & !allowed == 0 {
        Ok(())
    } else {
        Err(malformed(
            offset,
            "GPU instruction has unsupported modifiers",
        ))
    }
}
