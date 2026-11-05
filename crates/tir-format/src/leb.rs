use std::io::{self, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LebError {
    Truncated,
    Overflow,
    NonMinimal,
    InvalidSign,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntegerImmediate {
    pub negative: bool,
    pub magnitude: u128,
}

pub fn read_uleb(input: &[u8], cursor: &mut usize) -> Result<u64, LebError> {
    let start = *cursor;
    let mut value = 0u64;

    for index in 0..10 {
        let byte = *input.get(*cursor).ok_or(LebError::Truncated)?;
        *cursor += 1;
        let payload = u64::from(byte & 0x7f);
        let shift = index * 7;
        if shift == 63 && payload > 1 {
            return Err(LebError::Overflow);
        }
        value |= payload.checked_shl(shift).ok_or(LebError::Overflow)?;
        if byte & 0x80 == 0 {
            if *cursor - start != uleb_len(value) {
                return Err(LebError::NonMinimal);
            }
            return Ok(value);
        }
    }

    Err(LebError::Overflow)
}

pub fn read_uleb128(input: &[u8], cursor: &mut usize) -> Result<u128, LebError> {
    let start = *cursor;
    let mut value = 0u128;

    for index in 0..19 {
        let byte = *input.get(*cursor).ok_or(LebError::Truncated)?;
        *cursor += 1;
        let payload = u128::from(byte & 0x7f);
        let shift = index * 7;
        if shift == 126 && payload > 3 {
            return Err(LebError::Overflow);
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            if *cursor - start != uleb128_len(value) {
                return Err(LebError::NonMinimal);
            }
            return Ok(value);
        }
    }

    Err(LebError::Overflow)
}

pub fn read_integer_immediate(
    input: &[u8],
    cursor: &mut usize,
) -> Result<IntegerImmediate, LebError> {
    let sign = *input.get(*cursor).ok_or(LebError::Truncated)?;
    *cursor += 1;
    let negative = match sign {
        0 => false,
        1 => true,
        _ => return Err(LebError::InvalidSign),
    };
    let magnitude = read_uleb128(input, cursor)?;
    if negative && (magnitude == 0 || magnitude > 1u128 << 127) {
        return Err(if magnitude == 0 {
            LebError::InvalidSign
        } else {
            LebError::Overflow
        });
    }
    Ok(IntegerImmediate {
        negative,
        magnitude,
    })
}

pub fn read_sleb(input: &[u8], cursor: &mut usize) -> Result<i64, LebError> {
    let start = *cursor;
    let mut value = 0i128;
    let mut shift = 0u32;

    for _ in 0..10 {
        let byte = *input.get(*cursor).ok_or(LebError::Truncated)?;
        *cursor += 1;
        value |= i128::from(byte & 0x7f) << shift;
        shift += 7;

        if byte & 0x80 == 0 {
            if byte & 0x40 != 0 {
                value |= (!0i128) << shift;
            }
            let value = i64::try_from(value).map_err(|_| LebError::Overflow)?;
            if *cursor - start != sleb_len(value) {
                return Err(LebError::NonMinimal);
            }
            return Ok(value);
        }
    }

    Err(LebError::Overflow)
}

#[must_use]
pub const fn uleb_len(mut value: u64) -> usize {
    let mut length = 1;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

#[must_use]
pub const fn sleb_len(mut value: i64) -> usize {
    let mut length = 0;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        length += 1;
        if (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0) {
            return length;
        }
    }
}

pub fn write_uleb(mut value: u64, output: &mut impl Write) -> io::Result<()> {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.write_all(&[byte])?;
        if value == 0 {
            return Ok(());
        }
    }
}

pub fn write_uleb128(mut value: u128, output: &mut impl Write) -> io::Result<()> {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.write_all(&[byte])?;
        if value == 0 {
            return Ok(());
        }
    }
}

pub fn write_integer_immediate(
    negative: bool,
    magnitude: u128,
    output: &mut impl Write,
) -> io::Result<()> {
    if negative && (magnitude == 0 || magnitude > 1u128 << 127) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "integer immediate is outside the TIR integer range",
        ));
    }
    output.write_all(&[u8::from(negative)])?;
    write_uleb128(magnitude, output)
}

pub fn read_integer_bits(
    input: &[u8],
    cursor: &mut usize,
    bit_width: u16,
) -> Result<u128, LebError> {
    if !(1..=128).contains(&bit_width) {
        return Err(LebError::Overflow);
    }
    let byte_count = usize::from(bit_width).div_ceil(8);
    let end = cursor
        .checked_add(byte_count)
        .filter(|end| *end <= input.len())
        .ok_or(LebError::Truncated)?;
    let mut value = 0u128;
    for (index, byte) in input[*cursor..end].iter().enumerate() {
        value |= u128::from(*byte) << (index * 8);
    }
    *cursor = end;
    let unused_bits = bit_width % 8;
    if unused_bits != 0 && input[end - 1] >> unused_bits != 0 {
        return Err(LebError::Overflow);
    }
    Ok(value)
}

pub fn write_integer_bits(value: u128, bit_width: u16, output: &mut impl Write) -> io::Result<()> {
    if !(1..=128).contains(&bit_width) || (bit_width < 128 && value >> bit_width != 0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "integer bit payload exceeds its declared width",
        ));
    }
    let byte_count = usize::from(bit_width).div_ceil(8);
    for index in 0..byte_count {
        output.write_all(&[(value >> (index * 8)) as u8])?;
    }
    Ok(())
}

#[must_use]
pub const fn uleb128_len(mut value: u128) -> usize {
    let mut length = 1;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

pub fn write_sleb(mut value: i64, output: &mut impl Write) -> io::Result<()> {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        if !done {
            byte |= 0x80;
        }
        output.write_all(&[byte])?;
        if done {
            return Ok(());
        }
    }
}
