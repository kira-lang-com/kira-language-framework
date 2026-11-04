use crate::leb::read_uleb;
use crate::types::{
    BinaryAnnotation, BinaryCopyPolicy, BinaryDestroyPolicy, BinaryField, BinaryLayout,
    BinaryPolicy, BinaryRelocatePolicy, BinaryVariant,
};
use crate::{FormatError, StringTable};

pub(super) fn read_layout(bytes: &[u8], cursor: &mut usize) -> Result<BinaryLayout, FormatError> {
    match read_byte(bytes, cursor)? {
        0 => Ok(BinaryLayout::Ridl),
        1 => Ok(BinaryLayout::C),
        2 => {
            let alignment = read_index(bytes, cursor)?;
            if !matches!(alignment, 1 | 2 | 4 | 8) {
                return Err(malformed(*cursor, "invalid packed layout alignment"));
            }
            Ok(BinaryLayout::Packed(alignment))
        }
        3 => {
            let size = read_index(bytes, cursor)?;
            let align = read_index(bytes, cursor)?;
            if align == 0 {
                return Err(malformed(
                    *cursor,
                    "explicit layout alignment must be nonzero",
                ));
            }
            Ok(BinaryLayout::Explicit { size, align })
        }
        _ => Err(malformed(*cursor - 1, "invalid nominal layout code")),
    }
}

pub(super) fn read_fields(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    type_count: usize,
    layout: BinaryLayout,
) -> Result<Vec<BinaryField>, FormatError> {
    let count = read_count(bytes, cursor, 4, "field")?;
    let mut fields = reserve(count, bytes, *cursor)?;
    let mut names = std::collections::HashSet::new();
    for _ in 0..count {
        let name = read_nonempty_string(bytes, cursor, strings, "field")?;
        if !names.insert(name.clone()) {
            return Err(malformed(*cursor, "duplicate field name"));
        }
        let ty = read_type_reference(bytes, cursor, type_count)?;
        let flags = read_byte(bytes, cursor)?;
        if flags & !0x03 != 0 || (flags & 2 != 0 && flags & 1 == 0) {
            return Err(malformed(*cursor - 1, "invalid field offset flags"));
        }
        let offset = if flags & 1 != 0 {
            Some(read_index(bytes, cursor)?)
        } else {
            None
        };
        let (bit_offset, bit_width) = if flags & 2 != 0 {
            let bit_offset = read_index(bytes, cursor)?;
            let bit_width = read_index(bytes, cursor)?;
            if bit_offset > 7 {
                return Err(malformed(*cursor, "bit offset must be within its byte"));
            }
            (Some(bit_offset), Some(bit_width))
        } else {
            (None, None)
        };
        if matches!(layout, BinaryLayout::Explicit { .. }) && offset.is_none() {
            return Err(malformed(
                *cursor,
                "explicit layout field is missing its byte offset",
            ));
        }
        let annotation_count = read_count(bytes, cursor, 2, "field annotation")?;
        let mut annotations = reserve(annotation_count, bytes, *cursor)?;
        for _ in 0..annotation_count {
            let name = read_nonempty_string(bytes, cursor, strings, "field annotation")?;
            let argument_count = read_count(bytes, cursor, 1, "field annotation argument")?;
            let mut arguments = reserve(argument_count, bytes, *cursor)?;
            for _ in 0..argument_count {
                arguments.push(read_nonempty_string(
                    bytes,
                    cursor,
                    strings,
                    "field annotation literal",
                )?);
            }
            annotations.push(BinaryAnnotation { name, arguments });
        }
        fields.push(BinaryField {
            name,
            ty,
            offset,
            bit_offset,
            bit_width,
            annotations,
        });
    }
    Ok(fields)
}

pub(super) fn read_variants(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    type_count: usize,
) -> Result<Vec<BinaryVariant>, FormatError> {
    let count = read_count(bytes, cursor, 2, "enum variant")?;
    let mut variants = reserve(count, bytes, *cursor)?;
    let mut names = std::collections::HashSet::new();
    for _ in 0..count {
        let name = read_nonempty_string(bytes, cursor, strings, "enum variant")?;
        if !names.insert(name.clone()) {
            return Err(malformed(*cursor, "duplicate enum variant name"));
        }
        let payload_count = read_count(bytes, cursor, 1, "enum payload")?;
        let mut payload = reserve(payload_count, bytes, *cursor)?;
        for _ in 0..payload_count {
            payload.push(read_type_reference(bytes, cursor, type_count)?);
        }
        variants.push(BinaryVariant { name, payload });
    }
    Ok(variants)
}

pub(super) fn read_policy(bytes: &[u8], cursor: &mut usize) -> Result<BinaryPolicy, FormatError> {
    let copy = match read_byte(bytes, cursor)? {
        0 => BinaryCopyPolicy::Trivial,
        1 => BinaryCopyPolicy::Implicit(read_index(bytes, cursor)?),
        2 => BinaryCopyPolicy::Explicit(read_index(bytes, cursor)?),
        3 => BinaryCopyPolicy::None,
        _ => return Err(malformed(*cursor - 1, "invalid copy policy code")),
    };
    let destroy = match read_byte(bytes, cursor)? {
        0 => BinaryDestroyPolicy::Trivial,
        1 => BinaryDestroyPolicy::Function(read_index(bytes, cursor)?),
        _ => return Err(malformed(*cursor - 1, "invalid destroy policy code")),
    };
    let relocate = match read_byte(bytes, cursor)? {
        0 => BinaryRelocatePolicy::Bitwise,
        1 => BinaryRelocatePolicy::Function(read_index(bytes, cursor)?),
        _ => return Err(malformed(*cursor - 1, "invalid relocate policy code")),
    };
    let exclusivity = read_enum_byte(bytes, cursor, 2, "exclusivity policy")?;
    let pairing = read_enum_byte(bytes, cursor, 1, "pairing policy")?;
    let lifetime = read_enum_byte(bytes, cursor, 1, "lifetime policy")?;
    Ok(BinaryPolicy {
        copy,
        destroy,
        relocate,
        exclusivity,
        pairing,
        lifetime,
    })
}

fn read_type_reference(
    bytes: &[u8],
    cursor: &mut usize,
    type_count: usize,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor)?;
    if usize::try_from(index).map_or(true, |index| index >= type_count) {
        return Err(malformed(
            offset,
            "nominal definition type index is out of range",
        ));
    }
    Ok(index)
}

pub(super) fn read_nonempty_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    label: &str,
) -> Result<String, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, "TYPES string index overflows memory"))?;
    let value = strings
        .get(index)
        .ok_or_else(|| malformed(offset, "TYPES string index is out of range"))?;
    if value.is_empty() {
        return Err(malformed(offset, format!("{label} name is empty")));
    }
    Ok(value.to_owned())
}

pub(super) fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_record_bytes: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, format!("{label} count overflows memory")))?;
    if count > bytes.len().saturating_sub(*cursor) / minimum_record_bytes {
        return Err(malformed(
            offset,
            format!("{label} count exceeds remaining TYPES data"),
        ));
    }
    Ok(count)
}

pub(super) fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    if count > bytes.len() {
        return Err(malformed(offset, "TYPES record count exceeds section size"));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, "TYPES allocation failed"))?;
    Ok(values)
}

pub(super) fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid TYPES ULEB: {error:?}")))
}

pub(super) fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| malformed(*cursor, "truncated TYPES section"))?;
    *cursor += 1;
    Ok(value)
}

pub(super) fn read_enum_byte(
    bytes: &[u8],
    cursor: &mut usize,
    maximum: u8,
    label: &str,
) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = read_byte(bytes, cursor)?;
    if value > maximum {
        return Err(malformed(offset, format!("invalid {label} code")));
    }
    Ok(value)
}

pub(super) fn read_bool(
    bytes: &[u8],
    cursor: &mut usize,
    label: &str,
) -> Result<bool, FormatError> {
    match read_byte(bytes, cursor)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(malformed(*cursor - 1, format!("invalid {label}"))),
    }
}

pub(super) fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
