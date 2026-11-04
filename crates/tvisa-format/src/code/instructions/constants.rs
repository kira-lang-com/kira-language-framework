use tir_format::leb::read_integer_bits;

use crate::binary::FormatError;
use crate::{TvisaValueTypeRecord, TypeSection};

use super::helpers::{malformed, read_integer, read_type, scalar_width, vector_storage_size};
use super::{BinaryOperand, read_byte, read_index};

pub(super) fn decode(
    opcode: u64,
    bytes: &[u8],
    cursor: &mut usize,
    base: usize,
    start: usize,
    types: &TypeSection,
    operands: &mut Vec<BinaryOperand>,
) -> Result<Option<(u8, bool)>, FormatError> {
    let decoded = match opcode {
        0x000 => {
            let type_index = read_type(bytes, cursor, base, types)?;
            let Some(TvisaValueTypeRecord::Integer { bits }) =
                types.value_types.get(type_index as usize)
            else {
                return Err(malformed(base + start, "const.int type must be an integer"));
            };
            let bit_width = u16::try_from(*bits)
                .map_err(|_| malformed(base + start, "const.int width is invalid"))?;
            let value = read_integer_bits(bytes, cursor, bit_width).map_err(|error| {
                malformed(
                    base + *cursor,
                    format!("invalid const.int payload: {error:?}"),
                )
            })?;
            operands.push(BinaryOperand::Type(type_index));
            operands.push(BinaryOperand::IntegerBits(value));
            (1, false)
        }
        0x001 => {
            let type_index = read_type(bytes, cursor, base, types)?;
            if !matches!(
                types.value_types.get(type_index as usize),
                Some(TvisaValueTypeRecord::Float { .. })
            ) {
                return Err(malformed(
                    base + start,
                    "const.float type must be floating-point",
                ));
            }
            operands.push(BinaryOperand::Type(type_index));
            operands.push(BinaryOperand::Index(read_index(
                bytes,
                cursor,
                base,
                "floating-point constant index",
            )?));
            (1, false)
        }
        0x002 => {
            let value = read_integer(bytes, cursor, base, "boolean constant")?;
            if value.negative || value.magnitude > 1 {
                return Err(malformed(
                    base + start,
                    "const.bool payload must be zero or one",
                ));
            }
            operands.push(BinaryOperand::Integer(value));
            (1, false)
        }
        0x003 => {
            let space = read_byte(bytes, cursor, base, "null pointer address space")?;
            if space > 9 {
                return Err(malformed(
                    base + *cursor - 1,
                    "invalid null pointer address space",
                ));
            }
            operands.push(BinaryOperand::Index(u64::from(space)));
            (1, false)
        }
        0x004 | 0x005 => {
            operands.push(BinaryOperand::Type(read_type(bytes, cursor, base, types)?));
            (1, false)
        }
        0x007 => {
            let type_index = read_type(bytes, cursor, base, types)?;
            let length = read_index(bytes, cursor, base, "const.bytes length")?;
            let length = usize::try_from(length).map_err(|_| {
                malformed(base + *cursor, "const.bytes length does not fit in memory")
            })?;
            let end = cursor
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| malformed(base + *cursor, "truncated const.bytes payload"))?;
            operands.push(BinaryOperand::Type(type_index));
            operands.push(BinaryOperand::Bytes(bytes[*cursor..end].to_vec()));
            *cursor = end;
            (1, false)
        }
        0x008 => {
            let type_index = read_type(bytes, cursor, base, types)?;
            let Some(TvisaValueTypeRecord::FixedVector {
                lanes,
                element_type,
            }) = types.value_types.get(type_index as usize)
            else {
                return Err(malformed(
                    base + start,
                    "const.vec type must be a fixed vector",
                ));
            };
            operands.push(BinaryOperand::Type(type_index));
            if vector_storage_size(types, *lanes, *element_type).is_some_and(|size| size > 16) {
                operands.push(BinaryOperand::Index(read_index(
                    bytes,
                    cursor,
                    base,
                    "large vector constant index",
                )?));
            } else {
                let payload_start = *cursor;
                for _ in 0..*lanes {
                    let width = scalar_width(types, *element_type).ok_or_else(|| {
                        malformed(base + start, "const.vec element width is invalid")
                    })?;
                    read_integer_bits(bytes, cursor, width).map_err(|error| {
                        malformed(
                            base + *cursor,
                            format!("invalid const.vec lane payload: {error:?}"),
                        )
                    })?;
                }
                operands.push(BinaryOperand::Bytes(bytes[payload_start..*cursor].to_vec()));
            }
            (1, false)
        }
        0x00a | 0x00b | 0x00c | 0x010 => {
            operands.push(BinaryOperand::Symbol(read_index(
                bytes,
                cursor,
                base,
                "symbol reference",
            )?));
            if opcode == 0x00b {
                operands.push(BinaryOperand::Integer(read_integer(
                    bytes,
                    cursor,
                    base,
                    "global address addend",
                )?));
            }
            (1, false)
        }
        0x00f => {
            operands.push(BinaryOperand::Block(read_index(
                bytes,
                cursor,
                base,
                "block address",
            )?));
            (1, false)
        }
        0x011 => {
            let end = cursor
                .checked_add(23)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| malformed(base + *cursor, "truncated const.sampler payload"))?;
            let payload = &bytes[*cursor..end];
            validate_sampler_payload(payload, base + start)?;
            operands.push(BinaryOperand::Bytes(payload.to_vec()));
            *cursor = end;
            (1, false)
        }
        _ => return Ok(None),
    };
    Ok(Some(decoded))
}

fn validate_sampler_payload(payload: &[u8], offset: usize) -> Result<(), FormatError> {
    if payload.len() != 23
        || payload[..3].iter().any(|value| *value > 1)
        || payload[3..6].iter().any(|value| *value > 4)
        || payload[6] > 2
        || !(1..=16).contains(&payload[7])
        || !(payload[8] <= 7 || payload[8] == u8::MAX)
        || payload[9] > 2
        || payload[10] > 1
    {
        return Err(malformed(offset, "invalid const.sampler descriptor field"));
    }
    Ok(())
}
