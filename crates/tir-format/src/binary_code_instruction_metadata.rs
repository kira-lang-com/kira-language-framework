use crate::FormatError;
use crate::strings::StringTable;
use crate::types::{BinaryOperand, BinaryType, BinaryTypeTable};

use super::{
    malformed, read_bytes, read_index, read_integer, read_type, read_value_list, read_values,
};

const MAX_LIST_COUNT: u64 = 1 << 32;
const ASM_MODIFIERS: u64 = (1 << 37) | (1 << 38) | (1 << 39) | (1 << 40);

#[allow(clippy::too_many_arguments)]
pub(super) fn decode(
    opcode: u64,
    modifiers: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    block_count: u64,
    types: &BinaryTypeTable,
    strings: &StringTable,
    assembly_count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(u64, bool), FormatError> {
    match opcode {
        0x220 => {
            if modifiers & !ASM_MODIFIERS != 0
                || modifiers & (1 << 39) != 0 && modifiers & ((1 << 37) | (1 << 38)) != 0
            {
                return Err(malformed(base + *cursor, "invalid asm modifiers"));
            }
            read_assembly_index(bytes, cursor, base, assembly_count, operands)?;
            read_counted_values(bytes, cursor, base, current_value, operands)?;
            let ty = read_type(bytes, cursor, base, types)?;
            Ok((if is_unit(ty, types) { 0 } else { 1 }, false))
        }
        0x221 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            read_assembly_index(bytes, cursor, base, assembly_count, operands)?;
            read_counted_values(bytes, cursor, base, current_value, operands)?;
            let normal_offset = *cursor;
            let normal = read_index(bytes, cursor, base, "asm.goto normal block")?;
            if normal >= block_count {
                return Err(malformed(
                    base + normal_offset,
                    "asm.goto normal block is out of range",
                ));
            }
            operands.push(BinaryOperand::Index(normal));
            let label_offset = *cursor;
            let labels = read_index(bytes, cursor, base, "asm.goto label count")?;
            if labels >= MAX_LIST_COUNT || labels > bytes.len().saturating_sub(*cursor) as u64 {
                return Err(malformed(
                    base + label_offset,
                    "asm.goto label count exceeds payload",
                ));
            }
            let mut targets = Vec::new();
            targets
                .try_reserve_exact(usize::try_from(labels).map_err(|_| {
                    malformed(base + label_offset, "asm.goto label count overflows memory")
                })?)
                .map_err(|_| malformed(base + *cursor, "asm.goto target allocation failed"))?;
            for _ in 0..labels {
                let offset = *cursor;
                let target = read_index(bytes, cursor, base, "asm.goto label block")?;
                if target >= block_count {
                    return Err(malformed(
                        base + offset,
                        "asm.goto label block is out of range",
                    ));
                }
                targets.push(target);
            }
            operands.push(BinaryOperand::Indices(targets));
            Ok((0, true))
        }
        0x222 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            read_string_index(
                bytes,
                cursor,
                base,
                strings,
                "target intrinsic name",
                operands,
            )?;
            read_counted_values(bytes, cursor, base, current_value, operands)?;
            let ty = read_type(bytes, cursor, base, types)?;
            Ok((if is_unit(ty, types) { 0 } else { 1 }, false))
        }
        0x223 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            read_string_index(
                bytes,
                cursor,
                base,
                strings,
                "target feature name",
                operands,
            )?;
            Ok((1, false))
        }
        0x224 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            Ok((1, false))
        }
        0x230 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            let id_offset = *cursor;
            let id = read_index(bytes, cursor, base, "portable intrinsic ID")?;
            let (expected_arguments, results) = intrinsic_shape(id)
                .ok_or_else(|| malformed(base + id_offset, "unknown portable intrinsic ID"))?;
            operands.push(BinaryOperand::Index(id));
            let count_offset = *cursor;
            let count = read_index(bytes, cursor, base, "portable intrinsic argument count")?;
            if count != expected_arguments {
                return Err(malformed(
                    count_offset,
                    "portable intrinsic has the wrong argument count",
                ));
            }
            read_values(bytes, cursor, base, current_value, count, operands)?;
            Ok((results, false))
        }
        0x240 | 0x241 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "debug variable index",
            )?));
            Ok((0, false))
        }
        0x242 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "debug label index",
            )?));
            Ok((0, false))
        }
        0x243 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            Ok((0, false))
        }
        0x244 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            read_values(bytes, cursor, base, current_value, 1, operands)?;
            operands.push(BinaryOperand::Integer(read_integer(
                bytes,
                cursor,
                base,
                "expected value",
            )?));
            let present_offset = *cursor;
            match read_index(bytes, cursor, base, "expect probability flag")? {
                0 => operands.push(BinaryOperand::OptionalFloat32(None)),
                1 => {
                    let probability_offset = *cursor;
                    let bits = read_bytes(bytes, cursor, base, 4, "expect probability")?;
                    let bits = u32::from_le_bytes([bits[0], bits[1], bits[2], bits[3]]);
                    let probability = f32::from_bits(bits);
                    if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
                        return Err(malformed(
                            base + probability_offset,
                            "expect probability must be finite and between zero and one",
                        ));
                    }
                    operands.push(BinaryOperand::OptionalFloat32(Some(bits)));
                }
                _ => {
                    return Err(malformed(
                        base + present_offset,
                        "invalid expect probability flag",
                    ));
                }
            }
            Ok((1, false))
        }
        0x245 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            Ok((0, false))
        }
        0x246 => {
            require_no_modifiers(modifiers, base + *cursor)?;
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "profile counter ID",
            )?));
            Ok((0, false))
        }
        _ => Err(malformed(
            base + *cursor,
            "unsupported metadata instruction",
        )),
    }
}

fn read_assembly_index(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    assembly_count: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor, base, "ASM record index")?;
    if index >= assembly_count {
        return Err(malformed(base + offset, "ASM record index is out of range"));
    }
    operands.push(BinaryOperand::Index(index));
    Ok(())
}

fn read_string_index(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    strings: &StringTable,
    label: &str,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor, base, label)?;
    if usize::try_from(index).map_or(true, |index| index >= strings.len()) {
        return Err(malformed(base + offset, format!("{label} is out of range")));
    }
    operands.push(BinaryOperand::Index(index));
    Ok(())
}

fn read_counted_values(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    current_value: u64,
    operands: &mut Vec<BinaryOperand>,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    let count = read_index(bytes, cursor, base, "value count")?;
    if count >= MAX_LIST_COUNT || count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            base + offset,
            "value count exceeds instruction payload",
        ));
    }
    operands.push(BinaryOperand::Values(read_value_list(
        bytes,
        cursor,
        base,
        current_value,
        count,
    )?));
    Ok(count)
}

fn intrinsic_shape(id: u64) -> Option<(u64, u64)> {
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

fn is_unit(ty: usize, types: &BinaryTypeTable) -> bool {
    types
        .entries
        .get(ty)
        .is_some_and(|ty| matches!(ty, BinaryType::Unit))
}

fn require_no_modifiers(modifiers: u64, offset: usize) -> Result<(), FormatError> {
    if modifiers == 0 {
        Ok(())
    } else {
        Err(malformed(offset, "instruction does not accept modifiers"))
    }
}
