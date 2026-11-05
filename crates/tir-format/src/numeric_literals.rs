use tir_types::FloatType;

/// Returns whether a signed or unsigned integer spelling fits in an `iN` value.
#[must_use]
pub fn integer_literal_fits(literal: &str, bits: u16) -> bool {
    if bits == 0 || bits > 128 {
        return false;
    }
    let (negative, unsigned) = if let Some(rest) = literal.strip_prefix('-') {
        (true, rest)
    } else {
        (false, literal.strip_prefix('+').unwrap_or(literal))
    };
    let Some(magnitude) = parse_unsigned_integer(unsigned) else {
        return false;
    };
    if negative {
        if bits == 128 {
            magnitude <= (1u128 << 127)
        } else {
            magnitude <= (1u128 << (bits - 1))
        }
    } else if bits == 128 {
        true
    } else {
        magnitude < (1u128 << bits)
    }
}

/// Encodes an in-range integer spelling as the low `bits` bits of its value.
#[must_use]
pub fn integer_literal_bits(literal: &str, bits: u16) -> Option<u128> {
    if !integer_literal_fits(literal, bits) {
        return None;
    }
    let (negative, unsigned) = if let Some(rest) = literal.strip_prefix('-') {
        (true, rest)
    } else {
        (false, literal.strip_prefix('+').unwrap_or(literal))
    };
    let magnitude = parse_unsigned_integer(unsigned)?;
    Some(if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    })
}

/// Parses a nonnegative TIR integer literal without restricting it to a value width.
#[must_use]
pub fn parse_unsigned_integer(literal: &str) -> Option<u128> {
    let (radix, digits) = if let Some(rest) = literal
        .strip_prefix("0x")
        .or_else(|| literal.strip_prefix("0X"))
    {
        (16, rest)
    } else if let Some(rest) = literal
        .strip_prefix("0b")
        .or_else(|| literal.strip_prefix("0B"))
    {
        (2, rest)
    } else if let Some(rest) = literal
        .strip_prefix("0o")
        .or_else(|| literal.strip_prefix("0O"))
    {
        (8, rest)
    } else {
        (10, literal)
    };
    if digits.is_empty() || digits.starts_with('_') {
        return None;
    }
    let normalized: String = digits
        .chars()
        .filter(|character| *character != '_')
        .collect();
    u128::from_str_radix(&normalized, radix).ok()
}

/// Parses a signed TIR integer literal into the range of `i128`.
#[must_use]
pub fn parse_signed_integer(literal: &str) -> Option<i128> {
    let (negative, unsigned) = if let Some(rest) = literal.strip_prefix('-') {
        (true, rest)
    } else {
        (false, literal.strip_prefix('+').unwrap_or(literal))
    };
    let magnitude = parse_unsigned_integer(unsigned)?;
    if negative && magnitude == 1u128 << 127 {
        return Some(i128::MIN);
    }
    let value = i128::try_from(magnitude).ok()?;
    if negative {
        value.checked_neg()
    } else {
        Some(value)
    }
}

/// Returns whether a floating-point spelling is exactly representable by its type.
#[must_use]
pub fn float_literal_is_exactly_representable(literal: &str, float_type: FloatType) -> bool {
    let literal = literal.replace('_', "");
    if matches!(literal.as_str(), "inf" | "-inf") {
        return matches!(
            float_type,
            FloatType::F16
                | FloatType::Bf16
                | FloatType::F32
                | FloatType::F64
                | FloatType::F128
                | FloatType::F8E5M2
        );
    }
    let unsigned = literal
        .strip_prefix('-')
        .or_else(|| literal.strip_prefix('+'))
        .unwrap_or(&literal);
    if unsigned.starts_with("nan(") {
        return nan_payload(unsigned).is_some()
            && matches!(
                float_type,
                FloatType::F16
                    | FloatType::Bf16
                    | FloatType::F32
                    | FloatType::F64
                    | FloatType::F128
                    | FloatType::F8E4M3
                    | FloatType::F8E5M2
                    | FloatType::E8M0
            );
    }
    if float_type == FloatType::E8M0 {
        if literal.starts_with('-') {
            return false;
        }
        let Some((significand, exponent)) = parse_numeric_float(&literal) else {
            return false;
        };
        let Some((significand, exponent)) = normalize(significand, exponent) else {
            return false;
        };
        return significand == 1 && (-127..=127).contains(&exponent);
    }
    if float_type == FloatType::F8E4M3 {
        let Some((significand, exponent)) = parse_numeric_float(&literal) else {
            return false;
        };
        return f8e4m3_contains(significand, exponent);
    }
    let Some((precision, min_normal, max_normal)) = float_format(float_type) else {
        return false;
    };
    let Some((significand, exponent)) = parse_numeric_float(&literal) else {
        return false;
    };
    exactly_representable(significand, exponent, precision, min_normal, max_normal)
}

fn nan_payload(literal: &str) -> Option<&str> {
    let payload = literal.strip_prefix("nan(")?.strip_suffix(')')?;
    let unsigned = payload
        .strip_prefix('-')
        .or_else(|| payload.strip_prefix('+'))
        .unwrap_or(payload);
    let digits = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"));
    if let Some(digits) = digits {
        (!digits.is_empty()
            && digits
                .chars()
                .all(|character| character.is_ascii_hexdigit()))
        .then_some(payload)
    } else {
        (!unsigned.is_empty() && unsigned.chars().all(|character| character.is_ascii_digit()))
            .then_some(payload)
    }
}

fn float_format(float_type: FloatType) -> Option<(u32, i32, i32)> {
    Some(match float_type {
        FloatType::F16 => (11, -14, 15),
        FloatType::Bf16 => (8, -126, 127),
        FloatType::F32 => (24, -126, 127),
        FloatType::F64 => (53, -1022, 1023),
        FloatType::F128 => (113, -16382, 16383),
        FloatType::F8E4M3 => (4, -6, 8),
        FloatType::F8E5M2 => (3, -14, 15),
        FloatType::F6E2M3 => (4, 0, 2),
        FloatType::F6E3M2 => (3, -2, 4),
        FloatType::F4E2M1 => (2, 0, 2),
        FloatType::E8M0 => return None,
    })
}

pub(crate) fn ieee_float_format(float_type: FloatType) -> Option<(u32, i32, i32, i32, u32)> {
    Some(match float_type {
        FloatType::F16 => (11, -14, 15, 15, 5),
        FloatType::Bf16 => (8, -126, 127, 127, 8),
        FloatType::F32 => (24, -126, 127, 127, 8),
        FloatType::F64 => (53, -1022, 1023, 1023, 11),
        FloatType::F128 => (113, -16382, 16383, 16383, 15),
        _ => return None,
    })
}

pub(crate) fn parse_ieee_float_literal_bytes(
    literal: &str,
    float_type: FloatType,
) -> Option<Vec<u8>> {
    let literal = literal.replace('_', "");
    let (precision, min_normal, max_normal, bias, exponent_bits) = ieee_float_format(float_type)?;
    if !float_literal_is_exactly_representable(&literal, float_type) {
        return None;
    }
    let negative = literal.starts_with('-');
    let unsigned = literal
        .strip_prefix('-')
        .or_else(|| literal.strip_prefix('+'))
        .unwrap_or(&literal);
    let fraction_bits = precision.checked_sub(1)?;
    let sign = u128::from(negative) << (fraction_bits + exponent_bits);
    let bits = if unsigned == "inf" {
        sign | (((1_u128 << exponent_bits) - 1) << fraction_bits)
    } else if unsigned.starts_with("nan(") {
        let payload = nan_payload(unsigned)?;
        if payload.starts_with('-') {
            return None;
        }
        let payload = parse_unsigned_integer(payload.strip_prefix('+').unwrap_or(payload))?;
        let fraction_mask = (1_u128 << fraction_bits) - 1;
        if payload == 0 || payload > fraction_mask {
            return None;
        }
        sign | (((1_u128 << exponent_bits) - 1) << fraction_bits) | payload
    } else {
        let (significand, exponent) = parse_numeric_float(&literal)?;
        encode_binary_float(
            significand,
            exponent,
            negative,
            precision,
            min_normal,
            max_normal,
            bias,
            exponent_bits,
        )?
    };
    let byte_count = usize::from(float_type.bits().div_ceil(8));
    Some(bits.to_le_bytes()[..byte_count].to_vec())
}

fn parse_hex_float(literal: &str) -> Option<(u128, i32)> {
    let unsigned = literal
        .strip_prefix('-')
        .or_else(|| literal.strip_prefix('+'))
        .unwrap_or(literal);
    let unsigned = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))?;
    let exponent_marker = unsigned.find(|character| matches!(character, 'p' | 'P'))?;
    let mantissa = &unsigned[..exponent_marker];
    let exponent = unsigned[exponent_marker + 1..].parse::<i32>().ok()?;
    let (whole, fractional) = match mantissa.split_once('.') {
        Some((whole, fractional)) if !fractional.contains('.') => (whole, fractional),
        Some(_) => return None,
        None => (mantissa, ""),
    };
    if whole.is_empty() && fractional.is_empty()
        || !whole.chars().all(|character| character.is_ascii_hexdigit())
        || !fractional
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    let digits = format!("{whole}{fractional}");
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return Some((0, 0));
    }
    let trailing_zeroes = trimmed
        .chars()
        .rev()
        .take_while(|character| *character == '0')
        .count();
    let significant = &trimmed[..trimmed.len().saturating_sub(trailing_zeroes)];
    let significand = u128::from_str_radix(significant, 16).ok()?;
    let binary_exponent = exponent
        .checked_sub(i32::try_from(fractional.len()).ok()?.checked_mul(4)?)?
        .checked_add(i32::try_from(trailing_zeroes).ok()?.checked_mul(4)?)?;
    Some((significand, binary_exponent))
}

fn parse_decimal_float(literal: &str) -> Option<(u128, i32)> {
    let unsigned = literal
        .strip_prefix('-')
        .or_else(|| literal.strip_prefix('+'))
        .unwrap_or(literal);
    let (mantissa, exponent) = match unsigned.find(|character| matches!(character, 'e' | 'E')) {
        Some(index) => (
            &unsigned[..index],
            unsigned[index + 1..].parse::<i32>().ok()?,
        ),
        None => (unsigned, 0),
    };
    let (whole, fractional) = match mantissa.split_once('.') {
        Some((whole, fractional)) if !fractional.contains('.') => (whole, fractional),
        Some(_) => return None,
        None => (mantissa, ""),
    };
    if whole.is_empty() && fractional.is_empty()
        || !whole.chars().all(|character| character.is_ascii_digit())
        || !fractional
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fractional}");
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        return Some((0, 0));
    }
    let significand = trimmed.parse::<u128>().ok()?;
    let scale = exponent.checked_sub(i32::try_from(fractional.len()).ok()?)?;
    if scale >= 0 {
        if scale > 55 {
            return None;
        }
        let five_power = checked_power(5, scale as u32)?;
        Some((significand.checked_mul(five_power)?, scale))
    } else {
        let denominator_power = scale.checked_neg()?;
        if denominator_power > 55 {
            return None;
        }
        let five_power = checked_power(5, denominator_power as u32)?;
        if significand % five_power != 0 {
            return None;
        }
        Some((significand / five_power, scale))
    }
}

fn parse_numeric_float(literal: &str) -> Option<(u128, i32)> {
    parse_hex_float(literal).or_else(|| parse_decimal_float(literal))
}

/// Parses an exactly representable hexadecimal floating literal as binary32.
#[must_use]
pub fn parse_f32_hex_literal(literal: &str) -> Option<f32> {
    let literal = literal.replace('_', "");
    if !(literal.contains('x') || literal.contains('X'))
        || !float_literal_is_exactly_representable(&literal, FloatType::F32)
    {
        return None;
    }
    let negative = literal.starts_with('-');
    let (significand, exponent) = parse_hex_float(&literal)?;
    let bits = encode_binary_float(significand, exponent, negative, 24, -126, 127, 127, 8)?;
    Some(f32::from_bits(u32::try_from(bits).ok()?))
}

/// Parses an exactly representable hexadecimal floating literal as binary64.
#[must_use]
pub fn parse_f64_hex_literal(literal: &str) -> Option<f64> {
    let literal = literal.replace('_', "");
    if !(literal.contains('x') || literal.contains('X'))
        || !float_literal_is_exactly_representable(&literal, FloatType::F64)
    {
        return None;
    }
    let negative = literal.starts_with('-');
    let (significand, exponent) = parse_hex_float(&literal)?;
    let bits = encode_binary_float(significand, exponent, negative, 53, -1022, 1023, 1023, 11)?;
    Some(f64::from_bits(u64::try_from(bits).ok()?))
}

fn encode_binary_float(
    significand: u128,
    exponent: i32,
    negative: bool,
    precision: u32,
    min_normal: i32,
    max_normal: i32,
    bias: i32,
    exponent_bits: u32,
) -> Option<u128> {
    let fraction_bits = precision.checked_sub(1)?;
    let sign = u128::from(negative) << (fraction_bits + exponent_bits);
    if significand == 0 {
        return Some(sign);
    }
    let significant_bits = 128 - significand.leading_zeros();
    let top_exponent = exponent.checked_add(significant_bits as i32 - 1)?;
    if top_exponent > max_normal {
        return None;
    }
    if top_exponent >= min_normal {
        let normalized = if significant_bits > precision {
            let shift = significant_bits - precision;
            let mask = (1_u128 << shift) - 1;
            if significand & mask != 0 {
                return None;
            }
            significand >> shift
        } else {
            significand << (precision - significant_bits)
        };
        let fraction = normalized.checked_sub(1_u128 << fraction_bits)?;
        let exponent_field = u128::try_from(top_exponent.checked_add(bias)?).ok()?;
        return Some(sign | (exponent_field << fraction_bits) | fraction);
    }

    let subnormal_exponent = min_normal.checked_sub(fraction_bits as i32)?;
    let shift = exponent.checked_sub(subnormal_exponent)?;
    let fraction = if shift >= 0 {
        let shift = u32::try_from(shift).ok()?;
        if shift >= 128 || significand > (u128::MAX >> shift) {
            return None;
        }
        significand << shift
    } else {
        let shift = u32::try_from(shift.checked_neg()?).ok()?;
        if shift >= 128 {
            return None;
        }
        let mask = (1_u128 << shift) - 1;
        if significand & mask != 0 {
            return None;
        }
        significand >> shift
    };
    if fraction == 0 || fraction >= (1_u128 << fraction_bits) {
        return None;
    }
    Some(sign | fraction)
}

fn f8e4m3_contains(significand: u128, exponent: i32) -> bool {
    let Some(value) = normalize(significand, exponent) else {
        return false;
    };
    for exponent_bits in 0..=15i32 {
        for mantissa_bits in 0..=7u128 {
            if exponent_bits == 15 && mantissa_bits == 7 {
                continue;
            }
            let encoded = if exponent_bits == 0 {
                (mantissa_bits, -9)
            } else {
                (8 + mantissa_bits, exponent_bits - 10)
            };
            if normalize(encoded.0, encoded.1) == Some(value) {
                return true;
            }
        }
    }
    false
}

fn exactly_representable(
    significand: u128,
    exponent: i32,
    precision: u32,
    min_normal: i32,
    max_normal: i32,
) -> bool {
    let Some((significand, exponent)) = normalize(significand, exponent) else {
        return false;
    };
    if significand == 0 {
        return true;
    }
    let bits = 128 - significand.leading_zeros();
    let top_exponent = match exponent.checked_add(bits as i32 - 1) {
        Some(exponent) => exponent,
        None => return false,
    };
    let min_subnormal = min_normal - (precision as i32 - 1);
    exponent >= min_subnormal
        && top_exponent <= max_normal
        && (top_exponent < min_normal || bits <= precision)
}

fn normalize(mut significand: u128, mut exponent: i32) -> Option<(u128, i32)> {
    if significand == 0 {
        return Some((0, 0));
    }
    let trailing_zeroes = significand.trailing_zeros();
    significand >>= trailing_zeroes;
    exponent = exponent.checked_add(trailing_zeroes as i32)?;
    Some((significand, exponent))
}

fn checked_power(base: u128, exponent: u32) -> Option<u128> {
    let mut result = 1u128;
    for _ in 0..exponent {
        result = result.checked_mul(base)?;
    }
    Some(result)
}
