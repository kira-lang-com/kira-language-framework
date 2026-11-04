use crate::TypeSection;
use crate::binary::FormatError;

use super::helpers::{
    malformed, read_call_arguments, read_signature, read_type, read_values, signature_results,
};
use super::{BinaryOperand, read_byte, read_index};

const SCOPE_MASK: u64 = 7 << 26;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;

pub(super) fn decode(
    opcode: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    current_value: u64,
    modifiers: u64,
    types: &TypeSection,
    string_count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u8, bool)>, FormatError> {
    let decoded = match opcode {
        0x400 => {
            require_no_modifiers(modifiers, base + start)?;
            let name = read_index(bytes, cursor, base, "GPU builtin name")?;
            if name >= string_count {
                return Err(malformed(
                    base + start,
                    "GPU builtin name is not in STRINGS",
                ));
            }
            operands.push(BinaryOperand::Index(name));
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            (1, false)
        }
        0x401 => {
            require_no_modifiers(modifiers, base + start)?;
            let id = read_index(bytes, cursor, base, "GPU specialization constant ID")?;
            if id > u64::from(u32::MAX) {
                return Err(malformed(
                    base + start,
                    "GPU specialization constant ID exceeds 32 bits",
                ));
            }
            operands.push(BinaryOperand::Index(id));
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "GPU specialization default constant",
            )?));
            (1, false)
        }
        0x402 | 0x403 => {
            let scope = (modifiers >> 26) & 7;
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
                return Err(malformed(base + start, "invalid GPU synchronization scope"));
            }
            let memory = read_byte(bytes, cursor, base, "GPU memory-domain mask")?;
            if memory & !0x1f != 0 {
                return Err(malformed(
                    base + *cursor - 1,
                    "reserved GPU memory-domain bits are set",
                ));
            }
            operands.push(BinaryOperand::Index(u64::from(memory)));
            (0, false)
        }
        0x404 => {
            require_no_modifiers(modifiers, base + start)?;
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            let signature = read_signature(bytes, cursor, base, types)?;
            operands.push(BinaryOperand::Index(signature));
            let result_count = signature_results(types, signature, base + start)?;
            read_call_arguments(bytes, cursor, base, current_value, operands)?;
            (result_count, false)
        }
        0x405 | 0x407..=0x409 => {
            require_no_modifiers(modifiers, base + start)?;
            let results = u8::from(opcode == 0x409);
            (results, false)
        }
        0x406 => {
            require_no_modifiers(modifiers, base + start)?;
            let attachment =
                super::helpers::read_integer(bytes, cursor, base, "fragment attachment index")?;
            if attachment.negative || attachment.magnitude > u128::from(u32::MAX) {
                return Err(malformed(
                    base + start,
                    "fragment attachment index must be an unsigned 32-bit value",
                ));
            }
            operands.push(BinaryOperand::Integer(attachment));
            (1, false)
        }
        0x40a => {
            require_no_modifiers(modifiers, base + start)?;
            let format = read_index(bytes, cursor, base, "GPU printf format string")?;
            if format >= string_count {
                return Err(malformed(
                    base + start,
                    "GPU printf string is not in STRINGS",
                ));
            }
            operands.push(BinaryOperand::Index(format));
            read_call_arguments(bytes, cursor, base, current_value, operands)?;
            (0, false)
        }
        _ => return Ok(None),
    };
    Ok(Some(decoded))
}

fn require_no_modifiers(modifiers: u64, offset: usize) -> Result<(), FormatError> {
    if modifiers == 0 {
        Ok(())
    } else {
        Err(malformed(
            offset,
            "GPU instruction has unsupported modifiers",
        ))
    }
}
