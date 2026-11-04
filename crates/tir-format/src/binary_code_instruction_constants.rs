use crate::FormatError;
use crate::strings::StringTable;
use crate::types::{
    BinaryConstantTable, BinaryConstantValue, BinaryOperand, BinaryType, BinaryTypeTable,
};
use tir_types::FloatType;

use super::{malformed, read_byte, read_bytes, read_index, read_integer, read_type};

pub(super) fn decode_const_int(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let type_offset = *cursor;
    let ty = read_type(bytes, cursor, base, types)?;
    let Some(BinaryType::Integer(width)) = types.entries.get(ty) else {
        return Err(malformed(
            base + type_offset,
            "const.int type must be an integer",
        ));
    };
    if *width == 0 {
        return Err(malformed(
            base + type_offset,
            "const.int width must be nonzero",
        ));
    }
    let byte_count = width
        .checked_add(7)
        .and_then(|bits| usize::try_from(bits / 8).ok())
        .ok_or_else(|| malformed(base + type_offset, "const.int width overflows memory"))?;
    let payload_offset = *cursor;
    let payload = read_bytes(bytes, cursor, base, byte_count, "integer constant")?;
    let unused = (*width % 8) as u8;
    if unused != 0 && payload.last().is_some_and(|last| *last >> unused != 0) {
        return Err(malformed(
            base + payload_offset,
            "const.int has nonzero unused high bits",
        ));
    }
    operands.push(BinaryOperand::Type(ty as u64));
    operands.push(BinaryOperand::Bytes(payload));
    Ok(())
}

pub(super) fn decode_const_float(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let type_offset = *cursor;
    let ty = read_type(bytes, cursor, base, types)?;
    if !matches!(types.entries.get(ty), Some(BinaryType::Float(_))) {
        return Err(malformed(
            base + type_offset,
            "const.float type must be floating-point",
        ));
    }
    let constant_offset = *cursor;
    let constant = read_index(bytes, cursor, base, "floating-point constant index")?;
    if constants
        .records
        .get(usize::try_from(constant).map_err(|_| {
            malformed(
                base + constant_offset,
                "floating-point constant index overflows memory",
            )
        })?)
        .is_none_or(|record| record.ty != ty as u64)
    {
        return Err(malformed(
            base + constant_offset,
            "const.float constant has the wrong type",
        ));
    }
    operands.push(BinaryOperand::Type(ty as u64));
    operands.push(BinaryOperand::Index(constant));
    Ok(())
}

pub(super) fn decode_const_aggregate(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let type_offset = *cursor;
    let ty = read_type(bytes, cursor, base, types)?;
    let constant_offset = *cursor;
    let index = read_index(bytes, cursor, base, "aggregate constant index")?;
    let constant = constants
        .records
        .get(usize::try_from(index).map_err(|_| {
            malformed(
                base + constant_offset,
                "aggregate constant index overflows memory",
            )
        })?)
        .ok_or_else(|| {
            malformed(
                base + constant_offset,
                "aggregate constant index is out of range",
            )
        })?;
    let form_matches = matches!(
        (types.entries.get(ty), &constant.value),
        (Some(BinaryType::Tuple(_)), BinaryConstantValue::Tuple(_))
            | (
                Some(BinaryType::Array { .. }),
                BinaryConstantValue::Array(_)
            )
            | (
                Some(BinaryType::FixedVector { .. }),
                BinaryConstantValue::Vector(_)
            )
            | (
                Some(BinaryType::Nominal { .. }),
                BinaryConstantValue::Struct(_)
            )
            | (
                Some(BinaryType::Nominal { .. }),
                BinaryConstantValue::Union { .. }
            )
    );
    if constant.ty != ty as u64 || !form_matches {
        return Err(malformed(
            if constant.ty != ty as u64 {
                base + constant_offset
            } else {
                base + type_offset
            },
            "const.aggregate constant has the wrong type or value form",
        ));
    }
    operands.push(BinaryOperand::Type(ty as u64));
    operands.push(BinaryOperand::Index(index));
    Ok(())
}

pub(super) fn decode_const_bool(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let value = read_integer(bytes, cursor, base, "boolean constant")?;
    if value.negative || value.magnitude > 1 {
        return Err(malformed(base + *cursor, "const.bool must be zero or one"));
    }
    operands.push(BinaryOperand::Integer(value));
    Ok(())
}

pub(super) fn decode_const_null(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let space = read_byte(bytes, cursor, base, "null address space")?;
    if space > 9 {
        return Err(malformed(base + offset, "invalid const.null address space"));
    }
    operands.push(BinaryOperand::Index(u64::from(space)));
    Ok(())
}

pub(super) fn decode_const_string(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    strings: &StringTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor, base, "string index")?;
    if usize::try_from(index).map_or(true, |index| index >= strings.len()) {
        return Err(malformed(
            base + offset,
            "const.string string index is out of range",
        ));
    }
    operands.push(BinaryOperand::Index(index));
    Ok(())
}

pub(super) fn decode_const_bytes(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let ty = read_type(bytes, cursor, base, types)?;
    let length = read_index(bytes, cursor, base, "const.bytes length")?;
    let length = usize::try_from(length)
        .map_err(|_| malformed(base + *cursor, "const.bytes length overflows memory"))?;
    let payload = read_bytes(bytes, cursor, base, length, "const.bytes payload")?;
    operands.push(BinaryOperand::Type(ty as u64));
    operands.push(BinaryOperand::Bytes(payload));
    Ok(())
}

pub(super) fn decode_const_vec(
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    operands: &mut Vec<BinaryOperand>,
) -> Result<(), FormatError> {
    let type_offset = *cursor;
    let ty = read_type(bytes, cursor, base, types)?;
    let Some(BinaryType::FixedVector { lanes, element }) = types.entries.get(ty) else {
        return Err(malformed(
            base + type_offset,
            "const.vec type must be a fixed vector",
        ));
    };
    let element_type = match types.entries.get(usize::try_from(*element).map_err(|_| {
        malformed(
            base + type_offset,
            "const.vec element type index overflows memory",
        )
    })?) {
        Some(ty @ (BinaryType::Integer(_) | BinaryType::Float(_))) => ty,
        _ => {
            return Err(malformed(
                base + type_offset,
                "const.vec element type must be scalar numeric",
            ));
        }
    };
    let element_width = match element_type {
        BinaryType::Integer(width) => *width,
        BinaryType::Float(kind) => u64::from(kind.bits()),
        _ => unreachable!(),
    };
    let total_bits = lanes
        .checked_mul(element_width)
        .ok_or_else(|| malformed(base + type_offset, "const.vec storage size overflows"))?;
    let byte_count = usize::try_from(total_bits.div_ceil(8))
        .map_err(|_| malformed(base + type_offset, "const.vec payload overflows memory"))?;
    let storage_size = vector_storage_size(*lanes, element_type, base + type_offset)?;
    let payload = if storage_size <= 16 {
        let payload_offset = *cursor;
        let payload = read_bytes(bytes, cursor, base, byte_count, "const.vec payload")?;
        let used = (total_bits % 8) as u8;
        if used != 0 && payload.last().is_some_and(|last| *last >> used != 0) {
            return Err(malformed(
                base + payload_offset,
                "const.vec has nonzero padding bits",
            ));
        }
        BinaryOperand::Bytes(payload)
    } else {
        let index_offset = *cursor;
        let index = read_index(bytes, cursor, base, "const.vec constant index")?;
        let constant = constants
            .records
            .get(usize::try_from(index).map_err(|_| {
                malformed(
                    base + index_offset,
                    "const.vec constant index overflows memory",
                )
            })?)
            .ok_or_else(|| {
                malformed(
                    base + index_offset,
                    "const.vec constant index is out of range",
                )
            })?;
        if constant.ty != ty as u64 {
            return Err(malformed(
                base + index_offset,
                "const.vec constant has the wrong type",
            ));
        }
        BinaryOperand::Index(index)
    };
    operands.push(BinaryOperand::Type(ty as u64));
    operands.push(payload);
    Ok(())
}

fn vector_storage_size(
    lanes: u64,
    element: &BinaryType,
    offset: usize,
) -> Result<u64, FormatError> {
    let width = match element {
        BinaryType::Integer(width) => *width,
        BinaryType::Float(kind) => u64::from(kind.bits()),
        _ => unreachable!(),
    };
    if matches!(element, BinaryType::Integer(1)) {
        return Ok(lanes);
    }
    if matches!(
        element,
        BinaryType::Float(FloatType::F6E2M3 | FloatType::F6E3M2 | FloatType::F4E2M1)
    ) {
        return Ok(lanes
            .checked_mul(width)
            .ok_or_else(|| malformed(offset, "const.vec storage size overflows"))?
            .div_ceil(8));
    }
    let scalar_size = width
        .div_ceil(8)
        .max(1)
        .checked_next_power_of_two()
        .ok_or_else(|| malformed(offset, "const.vec element size overflows"))?;
    let size = scalar_size
        .checked_mul(lanes)
        .ok_or_else(|| malformed(offset, "const.vec storage size overflows"))?;
    let lane_alignment = lanes
        .checked_next_power_of_two()
        .ok_or_else(|| malformed(offset, "const.vec lane count overflows"))?;
    let alignment = scalar_size
        .checked_mul(lane_alignment)
        .ok_or_else(|| malformed(offset, "const.vec alignment overflows"))?
        .min(16)
        .max(1);
    let remainder = size % alignment;
    if remainder == 0 {
        Ok(size)
    } else {
        size.checked_add(alignment - remainder)
            .ok_or_else(|| malformed(offset, "const.vec storage size overflows"))
    }
}

pub(super) fn validate_sampler_descriptor(bytes: &[u8], offset: usize) -> Result<(), FormatError> {
    if bytes.len() != 23
        || bytes[0] > 1
        || bytes[1] > 1
        || bytes[2] > 1
        || bytes[3..=5].iter().any(|mode| *mode > 4)
        || bytes[6] > 2
        || !(1..=16).contains(&bytes[7])
        || !(bytes[8] <= 7 || bytes[8] == u8::MAX)
        || bytes[9] > 2
        || bytes[10] > 1
    {
        return Err(malformed(offset, "invalid const.sampler descriptor"));
    }
    Ok(())
}
