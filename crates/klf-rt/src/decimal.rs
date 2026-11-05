//! The exact base-10 decimal behind Kira's `Number`.
//!
//! The value is `mantissa * 10^-scale` with a signed 65-bit mantissa, every
//! `I64` and every `U64`, and a scale of at most eighteen: two words, as the
//! program holds it. A mantissa that fits an `I64` is the first word and the
//! scale the second, so the program's inline paths stay 64-bit. One past it
//! sets bit 8 of the second word, `EXTENDED`, and the first word holds its low
//! 64 bits: a `U64` past `I64.max` as itself, a mantissa below `I64.min` as
//! that `U64` less 2^64. Work is done in `i128`; a product or quotient that
//! does not fit one goes through `big`'s 256 bits and narrows back, keeping the
//! widest scale whose mantissa fits.
//!
//! Every value is canonical: trailing fractional zeros are shed, so `1.0` and
//! `1.00` are the same two words and equality compares words.
//!
//! The math functions (`sqrt`, `sin`, `exp`, ...) run on the decimal itself, in
//! `functions`, at twenty-eight working digits, and round once to a `Number`.

use core::cell::Cell;
use core::cmp::Ordering;

mod big;
mod functions;
#[cfg(test)]
mod tests;
mod wide;

/// The largest number of fractional digits a `Number` keeps.
pub const MAX_SCALE: u32 = 18;

/// The second word of an answer that carries an error code instead of a
/// scale: the code is added to it. A scale is at most `MAX_SCALE`, so a low
/// byte at or above this is an error.
pub const ERROR_BASE: u64 = 32;

/// The bits a mantissa has, sign included.
pub const MANTISSA_BITS: u32 = 65;

/// The bit of the second word set when the mantissa does not fit an `I64`.
pub const EXTENDED: u64 = 1 << 8;

/// One past the largest mantissa; its negation is the smallest.
const MANTISSA_LIMIT: i128 = 1 << (MANTISSA_BITS - 1);

/// Why an operation had no answer. The discriminant is the code an answer
/// carries above `ERROR_BASE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalError {
    Overflow = 1,
    DivideByZero = 2,
    Parse = 3,
    /// An argument outside the function's domain: the square root of a
    /// negative, the logarithm of zero.
    Domain = 4,
}

/// The operations `klfsvc_number` performs, numbered as the service numbers
/// them. A math function is `MATH_BASE` plus its `klf.math` number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberOp {
    Add = 0,
    Subtract = 1,
    Multiply = 2,
    Divide = 3,
    Negate = 4,
    Compare = 5,
    ToInt = 7,
    FromFloat = 8,
    ToFloat = 9,
    /// The integer part as a `U64`'s bits.
    ToUnsigned = 10,
    And = 11,
    Or = 12,
    Xor = 13,
    ShiftLeft = 14,
    ShiftRight = 15,
    Complement = 16,
}

/// Where the math functions start in the service's numbering.
pub const MATH_BASE: u64 = 32;

impl NumberOp {
    #[must_use]
    pub fn from_code(code: u64) -> Option<Self> {
        Some(match code {
            0 => Self::Add,
            1 => Self::Subtract,
            2 => Self::Multiply,
            3 => Self::Divide,
            4 => Self::Negate,
            5 => Self::Compare,
            7 => Self::ToInt,
            8 => Self::FromFloat,
            9 => Self::ToFloat,
            10 => Self::ToUnsigned,
            11 => Self::And,
            12 => Self::Or,
            13 => Self::Xor,
            14 => Self::ShiftLeft,
            15 => Self::ShiftRight,
            16 => Self::Complement,
            _ => return None,
        })
    }
}

/// An exact base-10 decimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decimal {
    mantissa: i128,
    scale: u32,
}

thread_local! {
    /// The second word of this thread's last `service` answer.
    static LAST_SECOND: Cell<u64> = const { Cell::new(0) };
}

/// One `klfsvc_number` call: answers the first word and keeps the second for
/// `service_second`, which the caller reads next. Every service returns one
/// word, and a `Number` is two.
#[must_use]
pub fn service(operation: u64, left: [u64; 2], right: [u64; 2]) -> u64 {
    let [first, second] = perform(operation, left, right);
    LAST_SECOND.with(|cell| cell.set(second));
    first
}

/// The second word of this thread's last `service` answer: a scale, or
/// `ERROR_BASE` plus an error code.
#[must_use]
pub fn service_second() -> u64 {
    LAST_SECOND.with(Cell::get)
}

/// The answer as its two words.
fn words(answer: Result<Decimal, DecimalError>) -> [u64; 2] {
    match answer {
        Ok(value) => value.words(),
        Err(error) => [0, error_second(error)],
    }
}

/// The second word that carries `error`.
#[must_use]
pub fn error_second(error: DecimalError) -> u64 {
    ERROR_BASE + error as u64
}

/// One operation on two-word operands. `Compare` answers -1, 0 or 1, `ToInt`
/// an `Int` and `ToFloat` the bits of an `f64` in the first word; every other
/// operation a `Number`. A failed operation answers its error in the second
/// word.
#[must_use]
pub fn perform(operation: u64, left: [u64; 2], right: [u64; 2]) -> [u64; 2] {
    if operation == u64::from(NumberOp::FromFloat as u8) {
        return words(Decimal::from_f64(f64::from_bits(left[0])));
    }
    let first = match Decimal::from_words(left) {
        Ok(value) => value,
        Err(error) => return [0, error_second(error)],
    };
    if operation >= MATH_BASE {
        let code = operation - MATH_BASE;
        if functions::is_binary(code) {
            return match Decimal::from_words(right) {
                Ok(second) => words(functions::binary(code, first, second)),
                Err(error) => [0, error_second(error)],
            };
        }
        return words(functions::unary(code, first));
    }
    let Some(operation) = NumberOp::from_code(operation) else {
        return [0, error_second(DecimalError::Parse)];
    };
    match operation {
        NumberOp::ToInt => match i64::try_from(first.integer_part()) {
            Ok(value) => [value as u64, 0],
            Err(_) => [0, error_second(DecimalError::Overflow)],
        },
        NumberOp::ToUnsigned => match u64::try_from(first.integer_part()) {
            Ok(value) => [value, 0],
            Err(_) => [0, error_second(DecimalError::Overflow)],
        },
        NumberOp::ToFloat => [first.to_f64().to_bits(), 0],
        NumberOp::Negate => words(first.checked_neg()),
        NumberOp::Complement => words(first.whole().and_then(|value| narrow(!value, 0))),
        NumberOp::FromFloat => unreachable!("answered above"),
        NumberOp::And
        | NumberOp::Or
        | NumberOp::Xor
        | NumberOp::ShiftLeft
        | NumberOp::ShiftRight => {
            let second = match Decimal::from_words(right) {
                Ok(value) => value,
                Err(error) => return [0, error_second(error)],
            };
            words(first.bitwise(operation, second))
        }
        NumberOp::Compare
        | NumberOp::Add
        | NumberOp::Subtract
        | NumberOp::Multiply
        | NumberOp::Divide => {
            let second = match Decimal::from_words(right) {
                Ok(value) => value,
                Err(error) => return [0, error_second(error)],
            };
            match operation {
                NumberOp::Compare => [first.compare(second) as i64 as u64, 0],
                NumberOp::Add => words(first.checked_add(second)),
                NumberOp::Subtract => words(first.checked_sub(second)),
                NumberOp::Multiply => words(first.checked_mul(second)),
                _ => words(first.checked_div(second)),
            }
        }
    }
}

/// Ten to the `power` as an `i128`, or `None` when it overflows one.
pub(crate) fn pow10(power: u32) -> Option<i128> {
    10i128.checked_pow(power)
}

/// A 128-bit result narrowed to the mantissa, trailing fractional zeros shed
/// first so a value that only overflows because of its scale still fits.
fn narrow(mut mantissa: i128, mut scale: u32) -> Result<Decimal, DecimalError> {
    while scale > 0 && mantissa % 10 == 0 {
        mantissa /= 10;
        scale -= 1;
    }
    if !(-MANTISSA_LIMIT..MANTISSA_LIMIT).contains(&mantissa) {
        return Err(DecimalError::Overflow);
    }
    Ok(Decimal { mantissa, scale })
}

/// A 256-bit mantissa at `scale` as the decimal at the widest scale up to
/// `MAX_SCALE` whose mantissa fits, rounded half-to-even once.
fn fitted_big(value: big::Big, scale: u32) -> Result<Decimal, DecimalError> {
    for target in (0..=scale.min(MAX_SCALE)).rev() {
        let mantissa = if target == scale {
            value.to_i128()
        } else {
            pow10(scale - target)
                .and_then(|divisor| value.divide_half_even(big::Big::from_i128(divisor)))
        };
        if let Some(Ok(decimal)) = mantissa.map(|mantissa| narrow(mantissa, target)) {
            return Ok(decimal);
        }
    }
    Err(DecimalError::Overflow)
}

/// Rounds a truncated division `(quotient, remainder)` half-to-even.
fn round_quotient_half_even(
    quotient: i128,
    remainder: i128,
    numerator: i128,
    denominator: i128,
) -> Result<i128, DecimalError> {
    if remainder == 0 {
        return Ok(quotient);
    }
    let twice = remainder
        .unsigned_abs()
        .checked_mul(2)
        .ok_or(DecimalError::Overflow)?;
    let round_away = match twice.cmp(&denominator.unsigned_abs()) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => quotient % 2 != 0,
    };
    if !round_away {
        return Ok(quotient);
    }
    let sign: i128 = if (numerator < 0) ^ (denominator < 0) {
        -1
    } else {
        1
    };
    quotient.checked_add(sign).ok_or(DecimalError::Overflow)
}

/// `numerator / denominator`, rounded half-to-even, by exact integer division.
fn divide_round_half_even(numerator: i128, denominator: i128) -> Result<i128, DecimalError> {
    round_quotient_half_even(
        numerator / denominator,
        numerator % denominator,
        numerator,
        denominator,
    )
}

/// The same quotient through an `f64` estimate corrected by unit steps, or
/// `None` when the estimate is too far off, which sends the caller to the
/// exact divide.
fn divide_round_half_even_hinted(numerator: i128, denominator: i128) -> Option<i128> {
    let estimate = (numerator as f64 / denominator as f64).round();
    // Past a float's exact-integer range the estimate's low digits are noise.
    if !estimate.is_finite() || estimate.abs() >= 9.007e15 {
        return None;
    }
    let mut quotient = estimate as i128;
    let mut remainder = numerator.checked_sub(quotient.checked_mul(denominator)?)?;
    let magnitude = denominator.unsigned_abs();
    let mut budget = 4;
    while !(remainder.unsigned_abs() < magnitude
        && (remainder == 0 || (remainder < 0) == (numerator < 0)))
    {
        if budget == 0 {
            return None;
        }
        budget -= 1;
        if (remainder > 0) == (denominator > 0) {
            quotient = quotient.checked_add(1)?;
            remainder -= denominator;
        } else {
            quotient = quotient.checked_sub(1)?;
            remainder += denominator;
        }
    }
    round_quotient_half_even(quotient, remainder, numerator, denominator).ok()
}

/// A 128-bit mantissa at `scale` rounded to `target` fractional digits,
/// half-to-even.
fn round_to_scale(mantissa: i128, scale: u32, target: u32) -> Result<i128, DecimalError> {
    if target >= scale {
        let factor = pow10(target - scale).ok_or(DecimalError::Overflow)?;
        return mantissa.checked_mul(factor).ok_or(DecimalError::Overflow);
    }
    let divisor = pow10(scale - target).ok_or(DecimalError::Overflow)?;
    divide_round_half_even(mantissa, divisor)
}

/// A 128-bit mantissa at `scale` as the decimal at the widest scale up to
/// `MAX_SCALE` whose mantissa fits, rounded half-to-even once.
fn fitted(mantissa: i128, scale: u32) -> Result<Decimal, DecimalError> {
    let mut target = scale.min(MAX_SCALE);
    loop {
        if let Ok(decimal) =
            round_to_scale(mantissa, scale, target).and_then(|value| narrow(value, target))
        {
            return Ok(decimal);
        }
        if target == 0 {
            return Err(DecimalError::Overflow);
        }
        target -= 1;
    }
}

impl Decimal {
    /// Zero.
    pub const ZERO: Self = Self {
        mantissa: 0,
        scale: 0,
    };

    /// The decimal two words hold: a mantissa and a scale, `EXTENDED` set
    /// when the mantissa does not fit an `I64`.
    pub fn from_words(words: [u64; 2]) -> Result<Self, DecimalError> {
        let scale = (words[1] & 0xFF) as u32;
        if scale > MAX_SCALE || words[1] & !(EXTENDED | 0xFF) != 0 {
            return Err(DecimalError::Parse);
        }
        if words[1] & EXTENDED == 0 {
            return narrow(i128::from(words[0] as i64), scale);
        }
        // Past `I64.max` the top bit is set; below `I64.min` it is clear.
        let mantissa = if words[0] >> 63 == 1 {
            i128::from(words[0])
        } else {
            i128::from(words[0]) - (1i128 << 64)
        };
        narrow(mantissa, scale)
    }

    /// The two words that hold this decimal.
    #[must_use]
    pub fn words(self) -> [u64; 2] {
        let scale = u64::from(self.scale);
        if i64::try_from(self.mantissa).is_ok() {
            return [self.mantissa as u64, scale];
        }
        [self.mantissa as u64, scale | EXTENDED]
    }

    /// The integer part, truncated toward zero.
    #[must_use]
    pub fn integer_part(self) -> i128 {
        self.mantissa / 10i128.pow(self.scale)
    }

    /// The mantissa of a whole number; a fraction is outside the domain of
    /// the bitwise operations.
    fn whole(self) -> Result<i128, DecimalError> {
        if self.scale != 0 {
            return Err(DecimalError::Domain);
        }
        Ok(self.mantissa)
    }

    /// `&`, `|`, `^`, `<<` and `>>` on two whole numbers, as on a two's
    /// complement integer of the mantissa's width. A shift past it is outside
    /// the domain, and a left shift that leaves the width overflows.
    fn bitwise(self, operation: NumberOp, other: Self) -> Result<Self, DecimalError> {
        let left = self.whole()?;
        let right = other.whole()?;
        match operation {
            NumberOp::And => narrow(left & right, 0),
            NumberOp::Or => narrow(left | right, 0),
            NumberOp::Xor => narrow(left ^ right, 0),
            NumberOp::ShiftLeft | NumberOp::ShiftRight => {
                let amount = u32::try_from(right).map_err(|_| DecimalError::Domain)?;
                if amount >= MANTISSA_BITS {
                    return Err(DecimalError::Domain);
                }
                if operation == NumberOp::ShiftRight {
                    return narrow(left >> amount, 0);
                }
                narrow(
                    left.checked_mul(1i128 << amount)
                        .ok_or(DecimalError::Overflow)?,
                    0,
                )
            }
            _ => Err(DecimalError::Parse),
        }
    }

    /// The decimal `mantissa * 10^-scale`, canonical.
    pub fn new(mantissa: i64, scale: u32) -> Result<Self, DecimalError> {
        if scale > MAX_SCALE {
            return Err(DecimalError::Parse);
        }
        narrow(i128::from(mantissa), scale)
    }

    /// An integer as an exact decimal.
    #[must_use]
    pub const fn from_i64(value: i64) -> Self {
        Self {
            mantissa: value as i128,
            scale: 0,
        }
    }

    #[must_use]
    pub const fn mantissa(self) -> i128 {
        self.mantissa
    }

    #[must_use]
    pub const fn scale(self) -> u32 {
        self.scale
    }

    /// Both mantissas at their common scale, in 128 bits.
    fn align(self, other: Self) -> (i128, i128, u32) {
        let scale = self.scale.max(other.scale);
        // Scales stay within MAX_SCALE and mantissas within 65 bits, so the
        // product fits an i128.
        let left = self.mantissa * pow10(scale - self.scale).unwrap_or(1);
        let right = other.mantissa * pow10(scale - other.scale).unwrap_or(1);
        (left, right, scale)
    }

    /// Sum, exact when it fits, otherwise rounded at the widest fitting scale.
    pub fn checked_add(self, other: Self) -> Result<Self, DecimalError> {
        let (left, right, scale) = self.align(other);
        fitted(left + right, scale)
    }

    /// Difference, exact when it fits, otherwise rounded at the widest fitting scale.
    pub fn checked_sub(self, other: Self) -> Result<Self, DecimalError> {
        let (left, right, scale) = self.align(other);
        fitted(left - right, scale)
    }

    /// Product, exact when it fits, rounded half-to-even once at the widest
    /// scale that does.
    pub fn checked_mul(self, other: Self) -> Result<Self, DecimalError> {
        match self.mantissa.checked_mul(other.mantissa) {
            Some(wide) => fitted(wide, self.scale + other.scale),
            None => fitted_big(
                big::Big::product(self.mantissa, other.mantissa),
                self.scale + other.scale,
            ),
        }
    }

    /// Negation.
    pub fn checked_neg(self) -> Result<Self, DecimalError> {
        narrow(-self.mantissa, self.scale)
    }

    /// Quotient rounded half-to-even once, at the largest scale up to
    /// `MAX_SCALE` whose result fits the mantissa.
    pub fn checked_div(self, other: Self) -> Result<Self, DecimalError> {
        if other.mantissa == 0 {
            return Err(DecimalError::DivideByZero);
        }
        let shift = i64::from(other.scale) - i64::from(self.scale);
        let mut scale = MAX_SCALE;
        loop {
            if let Some(decimal) = Self::quotient_at_scale(
                self.mantissa,
                other.mantissa,
                shift + i64::from(scale),
                scale,
            ) {
                return Ok(decimal);
            }
            if scale == 0 {
                return Err(DecimalError::Overflow);
            }
            scale -= 1;
        }
    }

    fn quotient_at_scale(left: i128, right: i128, power: i64, scale: u32) -> Option<Self> {
        let factor = pow10(u32::try_from(power.unsigned_abs()).ok()?)?;
        let narrow_fit = if power >= 0 {
            left.checked_mul(factor).map(|numerator| (numerator, right))
        } else {
            right
                .checked_mul(factor)
                .map(|denominator| (left, denominator))
        };
        let Some((numerator, denominator)) = narrow_fit else {
            let (mut numerator, mut denominator) =
                (big::Big::from_i128(left), big::Big::from_i128(right));
            if power >= 0 {
                numerator = numerator.times(factor.unsigned_abs())?;
            } else {
                denominator = denominator.times(factor.unsigned_abs())?;
            }
            return narrow(numerator.divide_half_even(denominator)?, scale).ok();
        };
        let mantissa = match divide_round_half_even_hinted(numerator, denominator) {
            Some(mantissa) => mantissa,
            None => divide_round_half_even(numerator, denominator).ok()?,
        };
        narrow(mantissa, scale).ok()
    }

    /// The numeric ordering.
    #[must_use]
    pub fn compare(self, other: Self) -> Ordering {
        let (left, right, _) = self.align(other);
        left.cmp(&right)
    }

    /// The nearest `f64`.
    #[must_use]
    pub fn to_f64(self) -> f64 {
        // Both operands exact, so the one division rounds once.
        if self.mantissa.unsigned_abs() < (1 << 53) {
            return self.mantissa as f64 / 10f64.powi(self.scale as i32);
        }
        format!("{}e-{}", self.mantissa, self.scale)
            .parse()
            .unwrap_or(f64::NAN)
    }

    /// The decimal nearest an `f64`'s binary value, at the widest scale whose
    /// digits fit the mantissa.
    ///
    /// The value is `fraction * 2^exponent` exactly, so each scale's mantissa
    /// is one shift of `fraction * 10^scale`, rounded half-to-even, with no
    /// text in between.
    pub fn from_f64(value: f64) -> Result<Self, DecimalError> {
        if !value.is_finite() {
            return Err(DecimalError::Parse);
        }
        let bits = value.to_bits();
        let biased = ((bits >> 52) & 0x7FF) as i32;
        let low = bits & ((1 << 52) - 1);
        let (fraction, exponent) = if biased == 0 {
            (low, -1074)
        } else {
            (low | (1 << 52), biased - 1075)
        };
        let sign = |magnitude: i128| if value < 0.0 { -magnitude } else { magnitude };
        if exponent >= 0 {
            // A whole number: it fits at scale 0 or not at all.
            if exponent >= 64 {
                return if fraction == 0 {
                    Ok(Self::ZERO)
                } else {
                    Err(DecimalError::Overflow)
                };
            }
            return narrow(sign(i128::from(fraction) << exponent), 0);
        }
        let shift = exponent.unsigned_abs();
        for scale in (0..=MAX_SCALE).rev() {
            // fraction < 2^53 and 10^18 < 2^60, so the product fits.
            let scaled = u128::from(fraction) * 10u128.pow(scale);
            let rounded = if shift >= 128 {
                0
            } else {
                let quotient = scaled >> shift;
                let remainder = scaled & ((1u128 << shift) - 1);
                let half = 1u128 << (shift - 1);
                if remainder > half || (remainder == half && quotient & 1 == 1) {
                    quotient + 1
                } else {
                    quotient
                }
            };
            if let Ok(decimal) = narrow(sign(rounded as i128), scale) {
                return Ok(decimal);
            }
        }
        Err(DecimalError::Overflow)
    }

    /// Parses an optional sign, digits, an optional `.` and more digits.
    pub fn parse(text: &str) -> Result<Self, DecimalError> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (integer, fraction) = rest.split_once('.').unwrap_or((rest, ""));
        if integer.is_empty() && fraction.is_empty() {
            return Err(DecimalError::Parse);
        }
        let mut mantissa: i128 = 0;
        for byte in integer.bytes().chain(fraction.bytes()) {
            if !byte.is_ascii_digit() {
                return Err(DecimalError::Parse);
            }
            mantissa = mantissa
                .checked_mul(10)
                .and_then(|value| value.checked_add(i128::from(byte - b'0')))
                .ok_or(DecimalError::Overflow)?;
        }
        let scale = u32::try_from(fraction.len()).map_err(|_| DecimalError::Overflow)?;
        if scale > MAX_SCALE {
            return Err(DecimalError::Parse);
        }
        narrow(if negative { -mantissa } else { mantissa }, scale)
    }
}
