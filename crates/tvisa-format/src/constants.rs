use tir_format::leb::read_uleb;

use crate::binary::FormatError;
use crate::types::{TvisaValueTypeRecord, TypeSection};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstantRecord {
    pub type_index: u64,
    /// Scalar payload bytes, or fixed-vector lane payloads in lane order.
    pub value_bytes: Vec<u8>,
}

pub(crate) fn decode(
    bytes: &[u8],
    types: &TypeSection,
) -> Result<Vec<ConstantRecord>, FormatError> {
    let mut cursor = 0;
    let mut constants = Vec::new();
    while cursor < bytes.len() {
        let start = cursor;
        let type_index = read_index(bytes, &mut cursor)?;
        let Some(value_type) = usize::try_from(type_index)
            .ok()
            .and_then(|index| types.value_types.get(index))
        else {
            return Err(malformed(start, "constant type index is out of range"));
        };
        let value_bytes = read_value(bytes, &mut cursor, value_type, types, start)?;
        constants
            .try_reserve(1)
            .map_err(|_| malformed(start, "constant table allocation failed"))?;
        constants.push(ConstantRecord {
            type_index,
            value_bytes,
        });
    }
    Ok(constants)
}

fn read_value(
    bytes: &[u8],
    cursor: &mut usize,
    value_type: &TvisaValueTypeRecord,
    types: &TypeSection,
    start: usize,
) -> Result<Vec<u8>, FormatError> {
    match value_type {
        TvisaValueTypeRecord::Integer { bits } => {
            read_scalar(bytes, cursor, *bits, start, "integer")
        }
        TvisaValueTypeRecord::Float { tag } if *tag < 0x07 => {
            read_scalar(bytes, cursor, float_width(*tag), start, "floating-point")
        }
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } => {
            let element = usize::try_from(*element_type)
                .ok()
                .and_then(|index| types.value_types.get(index))
                .ok_or_else(|| malformed(start, "constant vector element type is out of range"))?;
            let width = match element {
                TvisaValueTypeRecord::Integer { bits } => *bits,
                TvisaValueTypeRecord::Float { tag } => float_width(*tag),
                _ => {
                    return Err(malformed(
                        start,
                        "constant vector lanes must be integers or floating-point values",
                    ));
                }
            };
            let lane_count = usize::try_from(*lanes)
                .map_err(|_| malformed(start, "constant vector lane count does not fit"))?;
            let lane_bytes = usize::try_from(width.div_ceil(8))
                .map_err(|_| malformed(start, "constant vector lane size does not fit"))?;
            let capacity = lane_count
                .checked_mul(lane_bytes)
                .ok_or_else(|| malformed(start, "constant vector payload size overflows"))?;
            let mut value = Vec::new();
            value
                .try_reserve_exact(capacity)
                .map_err(|_| malformed(start, "constant vector allocation failed"))?;
            for _ in 0..lane_count {
                let lane = read_scalar(bytes, cursor, width, start, "vector lane")?;
                value.extend_from_slice(&lane);
            }
            Ok(value)
        }
        _ => Err(malformed(
            start,
            "constant type must be an integer, floating-point scalar, or fixed vector",
        )),
    }
}

fn read_scalar(
    bytes: &[u8],
    cursor: &mut usize,
    bits: u64,
    start: usize,
    name: &str,
) -> Result<Vec<u8>, FormatError> {
    let byte_count = usize::try_from(bits.div_ceil(8))
        .map_err(|_| malformed(start, format!("{name} constant size does not fit")))?;
    let end = cursor
        .checked_add(byte_count)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(*cursor, format!("truncated {name} constant")))?;
    let value = bytes[*cursor..end].to_vec();
    *cursor = end;
    let used_bits = bits % 8;
    if used_bits != 0 {
        let last = value.last().copied().unwrap_or_default();
        let mask = (1u8 << used_bits) - 1;
        if last & !mask != 0 {
            return Err(malformed(
                start,
                format!("{name} constant has nonzero padding bits"),
            ));
        }
    }
    Ok(value)
}

fn float_width(tag: u8) -> u64 {
    match tag {
        0x02 | 0x03 => 16,
        0x04 => 32,
        0x05 => 64,
        0x06 => 128,
        0x07 | 0x08 | 0x0c => 8,
        0x09 | 0x0a => 6,
        0x0b => 4,
        _ => 0,
    }
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let start = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(start, format!("invalid constant ULEB128: {error:?}")))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
