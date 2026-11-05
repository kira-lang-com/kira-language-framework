//! Fixed-width integer and floating-point primitives for the interpreter.

use crate::bytecode::{Opcode, OverflowMode, WidthCode, flags_width};
use crate::program::{Const, Program};

pub(crate) fn load_const(program: &Program, index: i32) -> Result<u64, &'static str> {
    match program.consts.get(index as usize) {
        Some(Const::Int(value)) => Ok(*value as u64),
        Some(Const::Float(bits)) => Ok(*bits),
        _ => Err("KLF-VM-0001"),
    }
}

fn mask(width: WidthCode) -> u128 {
    let bits = width.bits();
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

fn signed_value(raw: u64, width: WidthCode) -> i128 {
    let bits = width.bits();
    let value = u128::from(raw) & mask(width);
    if bits < 128 && value & (1u128 << (bits - 1)) != 0 {
        (value as i128) - (1i128 << bits)
    } else {
        value as i128
    }
}

pub(crate) fn integer_op(
    op: Opcode,
    lhs: u64,
    rhs: u64,
    width: WidthCode,
    signed: bool,
    overflow: OverflowMode,
) -> Result<u64, &'static str> {
    let bits = width.bits();
    let m = mask(width);
    let a = u128::from(lhs) & m;
    let b = u128::from(rhs) & m;
    let wrapped = |value: u128| (value & m) as u64;
    match op {
        Opcode::Add | Opcode::Sub | Opcode::Mul => checked_arith(op, a, b, width, signed, overflow),
        Opcode::And => Ok(wrapped(a & b)),
        Opcode::Or => Ok(wrapped(a | b)),
        Opcode::Xor => Ok(wrapped(a ^ b)),
        Opcode::UDiv => {
            if b == 0 {
                return Err("KLF-RT-0010");
            }
            Ok(wrapped(a / b))
        }
        Opcode::URem => {
            if b == 0 {
                return Err("KLF-RT-0010");
            }
            Ok(wrapped(a % b))
        }
        Opcode::SDiv | Opcode::SRem => signed_divide(op, lhs, rhs, width, overflow),
        Opcode::Shl | Opcode::LShr | Opcode::AShr => shift(op, lhs, rhs, width, overflow),
        Opcode::RotL | Opcode::RotR => {
            let amount = (u128::from(rhs) % u128::from(bits)) as u32;
            let value = a;
            let rotated = if amount == 0 {
                value
            } else if matches!(op, Opcode::RotL) {
                ((value << amount) | (value >> (bits - amount))) & m
            } else {
                ((value >> amount) | (value << (bits - amount))) & m
            };
            Ok(rotated as u64)
        }
        _ => Err("KLF-VM-0001"),
    }
}

fn checked_arith(
    op: Opcode,
    a: u128,
    b: u128,
    width: WidthCode,
    signed: bool,
    overflow: OverflowMode,
) -> Result<u64, &'static str> {
    let bits = width.bits();
    let m = mask(width);
    if signed {
        let sa = signed_value(a as u64, width);
        let sb = signed_value(b as u64, width);
        let full = match op {
            Opcode::Add => sa + sb,
            Opcode::Sub => sa - sb,
            _ => sa * sb,
        };
        let min = -(1i128 << (bits - 1));
        let max = (1i128 << (bits - 1)) - 1;
        if full < min || full > max {
            return match overflow {
                OverflowMode::Wrap => Ok(((full as u128) & m) as u64),
                OverflowMode::Trap => Err("KLF-RT-0002"),
                OverflowMode::Saturate => Ok(((full.clamp(min, max) as u128) & m) as u64),
            };
        }
        Ok(((full as u128) & m) as u64)
    } else {
        let full: u128 = match op {
            Opcode::Add => a.wrapping_add(b),
            Opcode::Sub => a.wrapping_sub(b),
            _ => a.wrapping_mul(b),
        };
        let overflowed = match op {
            Opcode::Add => full > m || full < a,
            Opcode::Sub => b > a,
            _ => a != 0 && full / a != b,
        };
        if overflowed {
            return match overflow {
                OverflowMode::Wrap => Ok((full & m) as u64),
                OverflowMode::Trap => Err("KLF-RT-0002"),
                OverflowMode::Saturate => Ok(if matches!(op, Opcode::Sub) {
                    0
                } else {
                    m as u64
                }),
            };
        }
        Ok((full & m) as u64)
    }
}

fn signed_divide(
    op: Opcode,
    lhs: u64,
    rhs: u64,
    width: WidthCode,
    overflow: OverflowMode,
) -> Result<u64, &'static str> {
    let sb = signed_value(rhs, width);
    if sb == 0 {
        return Err("KLF-RT-0010");
    }
    let sa = signed_value(lhs, width);
    let bits = width.bits();
    let min = -(1i128 << (bits - 1));
    let m = mask(width);
    if sa == min && sb == -1 {
        return match overflow {
            OverflowMode::Trap => Err("KLF-RT-0002"),
            _ => Ok(if matches!(op, Opcode::SDiv) {
                ((min as u128) & m) as u64
            } else {
                0
            }),
        };
    }
    let result = if matches!(op, Opcode::SDiv) {
        sa / sb
    } else {
        sa % sb
    };
    Ok(((result as u128) & m) as u64)
}

fn shift(
    op: Opcode,
    lhs: u64,
    rhs: u64,
    width: WidthCode,
    overflow: OverflowMode,
) -> Result<u64, &'static str> {
    let bits = width.bits();
    let m = mask(width);
    let amount = u128::from(rhs);
    let amount = if amount >= u128::from(bits) {
        if matches!(overflow, OverflowMode::Trap) {
            return Err("KLF-RT-0002");
        }
        (amount % u128::from(bits)) as u32
    } else {
        amount as u32
    };
    let value = u128::from(lhs) & m;
    let result = match op {
        Opcode::Shl => (value << amount) & m,
        Opcode::LShr => value >> amount,
        _ => {
            let signed = signed_value(lhs, width) >> amount;
            (signed as u128) & m
        }
    };
    Ok(result as u64)
}

pub(crate) fn integer_compare(predicate: u8, lhs: u64, rhs: u64, width: WidthCode) -> bool {
    let m = mask(width);
    let ua = u128::from(lhs) & m;
    let ub = u128::from(rhs) & m;
    let sa = signed_value(lhs, width);
    let sb = signed_value(rhs, width);
    match predicate {
        0 => ua == ub,
        1 => ua != ub,
        2 => sa < sb,
        3 => sa <= sb,
        4 => sa > sb,
        5 => sa >= sb,
        6 => ua < ub,
        7 => ua <= ub,
        8 => ua > ub,
        _ => ua >= ub,
    }
}

pub(crate) fn convert_integer(op: Opcode, value: u64, flags: u8) -> Option<u64> {
    let source = flags_width(flags)?;
    let dest = flags_width(flags >> 3)?;
    let result = match op {
        Opcode::Trunc => (u128::from(value) & mask(source)) & mask(dest),
        Opcode::ZExt => u128::from(value) & mask(source),
        _ => (signed_value(value, source) as u128) & mask(dest),
    };
    Some(result as u64)
}

pub(crate) fn int_to_float(op: Opcode, value: u64, flags: u8) -> Option<u64> {
    let source = flags_width(flags)?;
    let is_64 = (flags >> 3) & 1 == 1;
    #[allow(clippy::cast_precision_loss)]
    let float = if matches!(op, Opcode::SiToFp) {
        signed_value(value, source) as f64
    } else {
        (u128::from(value) & mask(source)) as f64
    };
    Some(from_float(float, is_64))
}

pub(crate) fn float_to_int(op: Opcode, value: u64, flags: u8) -> Result<u64, &'static str> {
    let dest = flags_width(flags).ok_or("KLF-VM-0001")?;
    let is_64 = (flags >> 3) & 1 == 1;
    let float = as_float(value, is_64).trunc();
    if float.is_nan() {
        return Err("KLF-RT-0011");
    }
    let bits = dest.bits();
    if matches!(op, Opcode::FpToSi) {
        let min = -(1i128 << (bits - 1));
        let max = (1i128 << (bits - 1)) - 1;
        #[allow(clippy::cast_precision_loss)]
        if float < min as f64 || float > max as f64 {
            return Err("KLF-RT-0011");
        }
        Ok(((float as i128) as u128 & mask(dest)) as u64)
    } else {
        let max = mask(dest);
        #[allow(clippy::cast_precision_loss)]
        if float < 0.0 || float > max as f64 {
            return Err("KLF-RT-0011");
        }
        Ok((float as u128 & mask(dest)) as u64)
    }
}

/// `fptosi [sat]` and `fptoui [sat]` (section 12.4): out-of-range inputs clamp
/// to the destination's extremes and NaN converts to zero.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn float_to_int_saturating(signed: bool, value: u64, flags: u8) -> Option<u64> {
    let dest = flags_width(flags)?;
    let is_64 = (flags >> 3) & 1 == 1;
    let float = as_float(value, is_64).trunc();
    if float.is_nan() {
        return Some(0);
    }
    let bits = dest.bits();
    if signed {
        let min = -(1i128 << (bits - 1));
        let max = (1i128 << (bits - 1)) - 1;
        let clamped = if float < min as f64 {
            min
        } else if float > max as f64 {
            max
        } else {
            float as i128
        };
        Some((clamped as u128 & mask(dest)) as u64)
    } else {
        let max = mask(dest);
        let clamped = if float < 0.0 {
            0
        } else if float > max as f64 {
            max
        } else {
            float as u128
        };
        Some((clamped & mask(dest)) as u64)
    }
}

/// The unary integer operations of section 12.2, evaluated in the destination
/// width so the result stays in range.
pub(crate) fn integer_unary(op: Opcode, value: u64, width: WidthCode) -> u64 {
    let bits = width.bits();
    let truncated = value & (mask(width) as u64);
    let result = match op {
        Opcode::Not => !truncated,
        Opcode::Neg => truncated.wrapping_neg(),
        Opcode::Ctlz => u64::from(truncated.leading_zeros()) - u64::from(64 - bits),
        Opcode::Cttz => {
            if truncated == 0 {
                u64::from(bits)
            } else {
                u64::from(truncated.trailing_zeros())
            }
        }
        Opcode::Ctpop => u64::from(truncated.count_ones()),
        Opcode::Bswap => truncated.swap_bytes() >> (64 - bits),
        _ => truncated.reverse_bits() >> (64 - bits),
    };
    result & (mask(width) as u64)
}

/// The `atomic.rmw` operators of section 12.14, in the order the lowering
/// encodes them: xchg, add, sub, and, or, xor, nand, max, min, umax, umin.
pub(crate) fn atomic_apply(operator: u8, previous: u64, operand: u64, width: WidthCode) -> u64 {
    let limit = mask(width) as u64;
    let signed = |value: u64| signed_value(value, width);
    let result = match operator {
        0 => operand,
        1 => previous.wrapping_add(operand),
        2 => previous.wrapping_sub(operand),
        3 => previous & operand,
        4 => previous | operand,
        5 => previous ^ operand,
        6 => !(previous & operand),
        7 => {
            if signed(previous) >= signed(operand) {
                previous
            } else {
                operand
            }
        }
        8 => {
            if signed(previous) <= signed(operand) {
                previous
            } else {
                operand
            }
        }
        9 => previous.max(operand),
        _ => previous.min(operand),
    };
    result & limit
}

pub(crate) fn as_float(bits: u64, is_64: bool) -> f64 {
    if is_64 {
        f64::from_bits(bits)
    } else {
        #[allow(clippy::cast_possible_truncation)]
        f64::from(f32::from_bits(bits as u32))
    }
}

pub(crate) fn from_float(value: f64, is_64: bool) -> u64 {
    if is_64 {
        value.to_bits()
    } else {
        #[allow(clippy::cast_possible_truncation)]
        u64::from((value as f32).to_bits())
    }
}

pub(crate) fn float_binary(op: Opcode, lhs: u64, rhs: u64, is_64: bool) -> u64 {
    let a = as_float(lhs, is_64);
    let b = as_float(rhs, is_64);
    let result = match op {
        Opcode::FAdd => a + b,
        Opcode::FSub => a - b,
        Opcode::FMul => a * b,
        Opcode::FDiv => a / b,
        Opcode::FRem => a % b,
        Opcode::FMin => a.min(b),
        _ => a.max(b),
    };
    from_float(result, is_64)
}

pub(crate) fn float_compare(predicate: u8, lhs: u64, rhs: u64, is_64: bool) -> bool {
    use std::cmp::Ordering;
    let a = as_float(lhs, is_64);
    let b = as_float(rhs, is_64);
    let ordering = a.partial_cmp(&b);
    let unordered = ordering.is_none();
    match predicate {
        0 => ordering == Some(Ordering::Equal),
        1 => matches!(ordering, Some(Ordering::Less | Ordering::Greater)),
        2 => ordering == Some(Ordering::Less),
        3 => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        4 => ordering == Some(Ordering::Greater),
        5 => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        6 => !unordered,
        7 => unordered,
        8 => unordered || ordering == Some(Ordering::Equal),
        9 => unordered || matches!(ordering, Some(Ordering::Less | Ordering::Greater)),
        10 => unordered || ordering == Some(Ordering::Less),
        11 => unordered || matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        12 => unordered || ordering == Some(Ordering::Greater),
        _ => unordered || matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
    }
}

/// `fma %a, %b, %c` (section 12.3): a single-rounding fused multiply-add.
pub(crate) fn float_fma(first: u64, second: u64, addend: u64, is_64: bool) -> u64 {
    let result = as_float(first, is_64).mul_add(as_float(second, is_64), as_float(addend, is_64));
    from_float(result, is_64)
}

pub(crate) fn float_unary(op: Opcode, value: u64, is_64: bool) -> u64 {
    let a = as_float(value, is_64);
    let result = match op {
        Opcode::FNeg => -a,
        Opcode::FAbs => a.abs(),
        Opcode::FSqrt => a.sqrt(),
        Opcode::FFloor => a.floor(),
        Opcode::FCeil => a.ceil(),
        Opcode::FTrunc => a.trunc(),
        _ => a.round_ties_even(),
    };
    from_float(result, is_64)
}
