use crate::TypeSection;
use crate::binary::FormatError;

use super::helpers::{malformed, read_call_arguments, read_type, read_values};
use super::{BinaryOperand, read_index};

pub(super) fn decode(
    opcode: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    types: &TypeSection,
    string_count: u64,
    assembly_count: u64,
    modifiers: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u8, bool)>, FormatError> {
    let decoded = match opcode {
        0x200 => {
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            (1, false)
        }
        0x201 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            (0, false)
        }
        0x202 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            (1, false)
        }
        0x203 => {
            read_values(bytes, cursor, base, current_value, 3, operands)?;
            (2, false)
        }
        0x204 => (0, false),
        0x210 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            (0, false)
        }
        0x211 => {
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            (1, false)
        }
        0x212 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            (0, false)
        }
        0x213 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            (0, false)
        }
        0x220 => {
            let allowed_modifiers = (1 << 37) | (1 << 38) | (1 << 39) | (1 << 40);
            if modifiers & !allowed_modifiers != 0
                || modifiers & (1 << 39) != 0 && modifiers & ((1 << 38) | (1 << 37)) != 0
            {
                return Err(malformed(
                    base + *cursor,
                    "invalid inline assembly modifiers",
                ));
            }
            let record = read_index(bytes, cursor, base, "ASM record index")?;
            if record >= assembly_count {
                return Err(malformed(base + *cursor, "ASM record index is invalid"));
            }
            operands.push(BinaryOperand::Index(record));
            let input_count = read_index(bytes, cursor, base, "ASM input count")?;
            read_values(bytes, cursor, base, current_value, input_count, operands)?;
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            (1, false)
        }
        0x221 => {
            if modifiers != 0 {
                return Err(malformed(
                    base + *cursor,
                    "asm.goto does not accept instruction modifiers",
                ));
            }
            let record = read_index(bytes, cursor, base, "ASM record index")?;
            if record >= assembly_count {
                return Err(malformed(base + *cursor, "ASM record index is invalid"));
            }
            operands.push(BinaryOperand::Index(record));
            let input_count = read_index(bytes, cursor, base, "ASM input count")?;
            read_values(bytes, cursor, base, current_value, input_count, operands)?;
            let normal = read_index(bytes, cursor, base, "asm.goto normal block")?;
            let label_count = read_index(bytes, cursor, base, "asm.goto label count")?;
            let remaining = bytes.len().saturating_sub(*cursor);
            if label_count > remaining as u64 {
                return Err(malformed(
                    base + *cursor,
                    "asm.goto label count exceeds the function body",
                ));
            }
            let capacity = usize::try_from(label_count)
                .ok()
                .and_then(|count| count.checked_add(1))
                .ok_or_else(|| malformed(base + *cursor, "asm.goto label count overflows"))?;
            let mut targets = Vec::new();
            targets
                .try_reserve_exact(capacity)
                .map_err(|_| malformed(base + *cursor, "asm.goto target allocation failed"))?;
            targets.push(normal);
            for _ in 0..label_count {
                targets.push(read_index(bytes, cursor, base, "asm.goto label block")?);
            }
            operands.push(BinaryOperand::Blocks(targets));
            (0, true)
        }
        0x222 => {
            let name = read_index(bytes, cursor, base, "target intrinsic name")?;
            if name >= string_count {
                return Err(malformed(
                    base + *cursor,
                    "target intrinsic name is not in STRINGS",
                ));
            }
            operands.push(BinaryOperand::Index(name));
            read_call_arguments(bytes, cursor, base, current_value, operands)?;
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            (1, false)
        }
        0x223 => {
            let feature = read_index(bytes, cursor, base, "target feature name")?;
            if feature >= string_count {
                return Err(malformed(
                    base + *cursor,
                    "target feature name is not in STRINGS",
                ));
            }
            operands.push(BinaryOperand::Index(feature));
            (1, false)
        }
        0x224 => (1, false),
        0x230 => {
            let id = read_index(bytes, cursor, base, "portable intrinsic ID")?;
            let Some((argument_count, result_count)) = portable_intrinsic_shape(id) else {
                return Err(malformed(
                    base + *cursor,
                    format!("unknown portable intrinsic ID {id}"),
                ));
            };
            operands.push(BinaryOperand::Index(id));
            read_values(bytes, cursor, base, current_value, argument_count, operands)?;
            (result_count, false)
        }
        0x240 | 0x241 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "debug variable record",
            )?));
            (0, false)
        }
        0x242 => {
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "debug label record",
            )?));
            (0, false)
        }
        0x243 => {
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            (0, false)
        }
        0x245 => (0, false),
        0x270 => {
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            (1, false)
        }
        0x271 => {
            read_values(bytes, cursor, base, current_value, 2, operands)?;
            (0, false)
        }
        0x272 => (0, false),
        0x273 | 0x280..=0x282 => {
            operands.push(BinaryOperand::Symbol(read_index(
                bytes,
                cursor,
                base,
                "platform symbol reference",
            )?));
            (1, false)
        }
        _ => return Ok(None),
    };
    Ok(Some(decoded))
}

fn portable_intrinsic_shape(id: u64) -> Option<(u64, u8)> {
    Some(match id {
        0..=5 => (2, 1),
        6 => (1, 1),
        7..=9 => (2, 0),
        10 | 11 | 14 => (0, 0),
        12 | 13 => (0, 1),
        15 | 16 => (3, 1),
        _ => return None,
    })
}
