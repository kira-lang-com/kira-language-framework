use std::fmt::Write as _;

use crate::BinaryAddressSpace;

pub(crate) fn linkage_name(linkage: crate::types::BinaryLinkage) -> &'static str {
    match linkage {
        crate::types::BinaryLinkage::Private => "private",
        crate::types::BinaryLinkage::Module => "module",
        crate::types::BinaryLinkage::Package => "package",
        crate::types::BinaryLinkage::Exported => "exported",
    }
}

pub(crate) fn address_space_name(space: BinaryAddressSpace) -> &'static str {
    match space {
        BinaryAddressSpace::Generic => "generic",
        BinaryAddressSpace::Device => "device",
        BinaryAddressSpace::Constant => "constant",
        BinaryAddressSpace::Threadgroup => "threadgroup",
        BinaryAddressSpace::Thread => "thread",
        BinaryAddressSpace::ObjectPayload => "object_payload",
        BinaryAddressSpace::RayPayload => "ray_payload",
        BinaryAddressSpace::HitAttribute => "hit_attribute",
        BinaryAddressSpace::Tile => "tile",
        BinaryAddressSpace::NodeRecord => "node_record",
    }
}

pub(crate) fn format_global_initializer(
    constant: &crate::types::BinaryConstant,
    types: &crate::types::BinaryTypeTable,
) -> Result<String, crate::FormatError> {
    use crate::types::{BinaryConstantValue, BinaryType};

    let ty = types
        .entries
        .get(usize::try_from(constant.ty).map_err(|_| {
            crate::FormatError::malformed(None, "constant type index overflows memory")
        })?)
        .ok_or_else(|| {
            crate::FormatError::malformed(None, "constant type index is out of range")
        })?;
    match (&constant.value, ty) {
        (BinaryConstantValue::Integer(bytes), BinaryType::Integer(_)) if bytes.len() <= 16 => {
            let mut bits = [0_u8; 16];
            bits[..bytes.len()].copy_from_slice(bytes);
            Ok(u128::from_le_bytes(bits).to_string())
        }
        (BinaryConstantValue::Float(bytes), BinaryType::Float(float)) => {
            Ok(format_ieee_float_literal(bytes, *float)
                .unwrap_or_else(|| format_byte_literal(bytes)))
        }
        (BinaryConstantValue::Zero, _) => Ok("0".to_owned()),
        (BinaryConstantValue::Null, BinaryType::Pointer(_)) => Ok("null".to_owned()),
        (BinaryConstantValue::Bytes(bytes), _) => Ok(format_byte_literal(bytes)),
        (BinaryConstantValue::String(value), BinaryType::Tuple(elements)) => {
            let string_type = if elements.len() == 2 {
                let pointer = usize::try_from(elements[0])
                    .ok()
                    .and_then(|index| types.entries.get(index));
                let length = usize::try_from(elements[1])
                    .ok()
                    .and_then(|index| types.entries.get(index));
                matches!(pointer, Some(BinaryType::Pointer(0)))
                    && matches!(length, Some(BinaryType::Integer(64)))
            } else {
                false
            };
            if string_type {
                quote(value)
            } else {
                Err(crate::FormatError::malformed(
                    None,
                    "string constant does not have type `(ptr, i64)`",
                ))
            }
        }
        _ => Err(crate::FormatError::malformed(
            None,
            "textual global disassembly does not support this constant form",
        )),
    }
}

pub(crate) fn format_ieee_float_literal(
    bytes: &[u8],
    float: tir_types::FloatType,
) -> Option<String> {
    let (precision, min_normal, _, bias, exponent_bits) =
        crate::numeric_literals::ieee_float_format(float)?;
    let byte_count = usize::from(float.bits().div_ceil(8));
    if bytes.len() != byte_count || byte_count > 16 {
        return None;
    }
    let mut raw = [0_u8; 16];
    raw[..byte_count].copy_from_slice(bytes);
    let bits = u128::from_le_bytes(raw);
    let fraction_bits = precision.checked_sub(1)?;
    let exponent_mask = (1_u128 << exponent_bits) - 1;
    let exponent_field = (bits >> fraction_bits) & exponent_mask;
    let fraction_mask = (1_u128 << fraction_bits) - 1;
    let fraction = bits & fraction_mask;
    let sign = if bits >> (fraction_bits + exponent_bits) != 0 {
        "-"
    } else {
        ""
    };
    if exponent_field == exponent_mask {
        return Some(if fraction == 0 {
            format!("{sign}inf")
        } else {
            format!("{sign}nan(0x{fraction:x})")
        });
    }
    let fraction_bits_i32 = i32::try_from(fraction_bits).ok()?;
    let (significand, exponent) = if exponent_field == 0 {
        (fraction, min_normal.checked_sub(fraction_bits_i32)?)
    } else {
        (
            fraction | (1_u128 << fraction_bits),
            i32::try_from(exponent_field).ok()? - bias - fraction_bits_i32,
        )
    };
    Some(format!("{sign}0x{significand:x}p{exponent}"))
}

fn format_byte_literal(bytes: &[u8]) -> String {
    let mut literal = String::from("b\"");
    for byte in bytes {
        match *byte {
            b'\n' => literal.push_str("\\n"),
            b'\r' => literal.push_str("\\r"),
            b'\t' => literal.push_str("\\t"),
            0 => literal.push_str("\\0"),
            b'\\' => literal.push_str("\\\\"),
            b'"' => literal.push_str("\\\""),
            byte if byte.is_ascii_graphic() || byte == b' ' => literal.push(char::from(byte)),
            byte => {
                let _ = write!(literal, "\\x{byte:02x}");
            }
        }
    }
    literal.push('"');
    literal
}

pub(crate) fn quote(value: &str) -> Result<String, crate::FormatError> {
    let mut quoted = String::with_capacity(value.len().saturating_add(2));
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            '\u{1b}' => quoted.push_str("\\e"),
            '\0' => quoted.push_str("\\0"),
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            character if character.is_control() => {
                return Err(crate::FormatError::malformed(
                    None,
                    "TIR text cannot represent this control character",
                ));
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    Ok(quoted)
}
