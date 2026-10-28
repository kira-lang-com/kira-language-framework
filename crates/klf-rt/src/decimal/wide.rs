//! The working precision of the decimal math functions: `±m * 10^e` with a
//! twenty-eight digit `m`, wide enough that a function's answer is right to
//! the eighteen fractional digits a `Number` keeps once rounded.

use super::{Decimal, DecimalError, MAX_SCALE, narrow};

/// The smallest and one past the largest normalized mantissa.
const LOW: u128 = 10u128.pow(27);
const HIGH: u128 = 10u128.pow(28);

/// Digits a series term may fall below its sum before it stops mattering.
const GUARD: i64 = 30;

/// A decimal at working precision. `m` is zero or in `[LOW, HIGH)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wide {
    neg: bool,
    m: u128,
    e: i64,
}

/// π/2 to twenty-eight digits.
pub(super) const HALF_PI: Wide = Wide::constant(1_570_796_326_794_896_619_231_321_692, -27);
/// π to twenty-eight digits.
pub(super) const PI: Wide = Wide::constant(3_141_592_653_589_793_238_462_643_383, -27);
/// ln 2 to twenty-eight digits.
pub(super) const LN2: Wide = Wide::constant(6_931_471_805_599_453_094_172_321_215, -28);
/// ln 10 to twenty-eight digits.
pub(super) const LN10: Wide = Wide::constant(2_302_585_092_994_045_684_017_991_455, -27);
pub(super) const ONE: Wide = Wide::constant(LOW, -27);
pub(super) const HALF: Wide = Wide::constant(5 * LOW, -28);
pub(super) const ZERO: Wide = Wide {
    neg: false,
    m: 0,
    e: 0,
};

/// `value / divisor`, rounded half up.
fn divide_rounded(value: u128, divisor: u128) -> u128 {
    let quotient = value / divisor;
    if (value % divisor) * 2 >= divisor {
        quotient + 1
    } else {
        quotient
    }
}

impl Wide {
    const fn constant(m: u128, e: i64) -> Self {
        Self { neg: false, m, e }
    }

    /// `±magnitude * 10^e`, normalized and rounded half up to twenty-eight
    /// digits.
    pub(super) fn new(neg: bool, mut magnitude: u128, mut e: i64) -> Self {
        if magnitude == 0 {
            return ZERO;
        }
        let mut round_up = false;
        while magnitude >= HIGH {
            round_up = magnitude % 10 >= 5;
            magnitude /= 10;
            e += 1;
        }
        while magnitude < LOW {
            magnitude *= 10;
            e -= 1;
        }
        if round_up {
            magnitude += 1;
            if magnitude == HIGH {
                magnitude = LOW;
                e += 1;
            }
        }
        Self {
            neg,
            m: magnitude,
            e,
        }
    }

    pub(super) fn from_decimal(value: Decimal) -> Self {
        let mantissa = value.mantissa();
        Self::new(
            mantissa < 0,
            mantissa.unsigned_abs(),
            -i64::from(value.scale()),
        )
    }

    pub(super) fn from_int(value: i64) -> Self {
        Self::new(value < 0, u128::from(value.unsigned_abs()), 0)
    }

    /// The nearest `f64`, for the seed of a Newton iteration.
    pub(super) fn to_f64(self) -> f64 {
        let magnitude: f64 = format!("{}e{}", self.m, self.e).parse().unwrap_or(f64::NAN);
        if self.neg { -magnitude } else { magnitude }
    }

    /// The decimal an `f64` holds, for a Newton seed.
    pub(super) fn from_f64(value: f64) -> Self {
        let text = format!("{value:e}");
        let (digits, exponent) = text.split_once('e').unwrap_or((&text, "0"));
        let exponent: i64 = exponent.parse().unwrap_or(0);
        let (negative, digits) = match digits.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, digits),
        };
        let (integer, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        let mut magnitude: u128 = 0;
        for byte in integer.bytes().chain(fraction.bytes()) {
            magnitude = magnitude * 10 + u128::from(byte - b'0');
        }
        let places = i64::try_from(fraction.len()).unwrap_or(0);
        Self::new(negative, magnitude, exponent - places)
    }

    /// The nearest `Number`, rounded half-to-even once at the widest scale
    /// whose mantissa fits.
    pub(super) fn to_decimal(self) -> Result<Decimal, DecimalError> {
        if self.m == 0 {
            return Ok(Decimal::ZERO);
        }
        let limit = if self.neg {
            1u128 << 64
        } else {
            (1u128 << 64) - 1
        };
        for scale in (0..=MAX_SCALE).rev() {
            let shift = self.e + i64::from(scale);
            let magnitude = if shift >= 0 {
                match u32::try_from(shift)
                    .ok()
                    .and_then(|power| 10u128.checked_pow(power))
                {
                    Some(factor) => match self.m.checked_mul(factor) {
                        Some(magnitude) => magnitude,
                        None => continue,
                    },
                    None => continue,
                }
            } else if shift < -38 {
                0
            } else {
                let divisor = 10u128.pow(u32::try_from(-shift).unwrap_or(38));
                let quotient = self.m / divisor;
                let twice = (self.m % divisor) * 2;
                if twice > divisor || (twice == divisor && quotient % 2 == 1) {
                    quotient + 1
                } else {
                    quotient
                }
            };
            if magnitude <= limit {
                let signed = i128::try_from(magnitude).map_err(|_| DecimalError::Overflow)?;
                return narrow(if self.neg { -signed } else { signed }, scale);
            }
        }
        Err(DecimalError::Overflow)
    }

    pub(super) fn is_zero(self) -> bool {
        self.m == 0
    }

    pub(super) fn is_negative(self) -> bool {
        self.neg && self.m != 0
    }

    pub(super) fn abs(self) -> Self {
        Self { neg: false, ..self }
    }

    pub(super) fn negate(self) -> Self {
        if self.m == 0 {
            self
        } else {
            Self {
                neg: !self.neg,
                ..self
            }
        }
    }

    /// The power of ten of the leading digit.
    pub(super) fn magnitude(self) -> i64 {
        self.e + 27
    }

    /// Whether a series term no longer changes `sum` at working precision.
    pub(super) fn negligible_beside(self, sum: Self) -> bool {
        self.m == 0 || (sum.m != 0 && self.magnitude() < sum.magnitude() - GUARD)
    }

    pub(super) fn add(self, other: Self) -> Self {
        if self.m == 0 {
            return other;
        }
        if other.m == 0 {
            return self;
        }
        let (big, small) = if self.e >= other.e {
            (self, other)
        } else {
            (other, self)
        };
        let gap = big.e - small.e;
        if gap > GUARD + 2 {
            return big;
        }
        // Ten more digits on the larger operand keep the smaller one's
        // digits that still count, within 128 bits.
        let (big_m, small_m, e) = if gap <= 10 {
            (
                big.m * 10u128.pow(u32::try_from(gap).unwrap_or(0)),
                small.m,
                small.e,
            )
        } else {
            let drop = u32::try_from(gap - 10).unwrap_or(0);
            (
                big.m * 10u128.pow(10),
                divide_rounded(small.m, 10u128.pow(drop)),
                big.e - 10,
            )
        };
        if big.neg == small.neg {
            return Self::new(big.neg, big_m + small_m, e);
        }
        if big_m >= small_m {
            Self::new(big.neg, big_m - small_m, e)
        } else {
            Self::new(small.neg, small_m - big_m, e)
        }
    }

    pub(super) fn subtract(self, other: Self) -> Self {
        self.add(other.negate())
    }

    pub(super) fn multiply(self, other: Self) -> Self {
        if self.m == 0 || other.m == 0 {
            return ZERO;
        }
        // Split at 10^14 so every partial product fits 128 bits:
        // a*b = hh*10^28 + (hl+lh)*10^14 + ll.
        let split = 10u128.pow(14);
        let (a_high, a_low) = (self.m / split, self.m % split);
        let (b_high, b_low) = (other.m / split, other.m % split);
        let middle = a_high * b_low + a_low * b_high;
        let low = a_low * b_low + (middle % split) * split;
        let mut high = a_high * b_high + middle / split + low / HIGH;
        let mut rest = low % HIGH;
        let mut e = self.e + other.e + 28;
        // `high` is the product over 10^28; take one more digit from the
        // rest when it is short of twenty-eight.
        if high < LOW {
            high = high * 10 + rest / LOW;
            rest %= LOW;
            rest *= 10;
            e -= 1;
        }
        let rounded = if rest >= 5 * LOW { high + 1 } else { high };
        Self::new(self.neg != other.neg, rounded, e)
    }

    pub(super) fn divide(self, other: Self) -> Result<Self, DecimalError> {
        if other.m == 0 {
            return Err(DecimalError::DivideByZero);
        }
        if self.m == 0 {
            return Ok(ZERO);
        }
        // Long division in steps of at most ten digits keeps the running
        // remainder times the step within 128 bits.
        let mut quotient = self.m / other.m;
        let mut remainder = self.m % other.m;
        for step in [10u32, 9, 9] {
            remainder *= 10u128.pow(step);
            quotient = quotient * 10u128.pow(step) + remainder / other.m;
            remainder %= other.m;
        }
        if remainder * 2 >= other.m {
            quotient += 1;
        }
        Ok(Self::new(
            self.neg != other.neg,
            quotient,
            self.e - other.e - 28,
        ))
    }

    pub(super) fn scaled_by_power_of_ten(self, power: i64) -> Self {
        if self.m == 0 {
            self
        } else {
            Self {
                e: self.e + power,
                ..self
            }
        }
    }

    /// The nearest integer, half away from zero, or `None` past `i64`.
    pub(super) fn round_to_int(self) -> Option<i64> {
        if self.m == 0 {
            return Some(0);
        }
        let magnitude = if self.e >= 0 {
            self.m
                .checked_mul(10u128.checked_pow(u32::try_from(self.e).ok()?)?)?
        } else if self.e < -38 {
            0
        } else {
            divide_rounded(self.m, 10u128.pow(u32::try_from(-self.e).ok()?))
        };
        let signed = i64::try_from(magnitude).ok()?;
        Some(if self.neg { -signed } else { signed })
    }
}
