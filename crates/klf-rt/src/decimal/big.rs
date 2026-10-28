//! A signed 256-bit integer, for the products and quotients of two 65-bit
//! mantissas that do not fit an `i128`. Only the service's slow path reaches
//! it: every operation tries `i128` first.

use core::cmp::Ordering;

/// Sign and magnitude; the magnitude is `high * 2^128 + low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Big {
    negative: bool,
    high: u128,
    low: u128,
}

impl Big {
    pub(super) fn from_i128(value: i128) -> Self {
        Self {
            negative: value < 0,
            high: 0,
            low: value.unsigned_abs(),
        }
    }

    fn is_zero(self) -> bool {
        self.high == 0 && self.low == 0
    }

    /// The full product of two magnitudes.
    fn multiply_magnitudes(left: u128, right: u128) -> (u128, u128) {
        let mask = u128::from(u64::MAX);
        let (a, b) = (left >> 64, left & mask);
        let (c, d) = (right >> 64, right & mask);
        let low = b * d;
        let middle_one = a * d;
        let middle_two = b * c;
        let high = a * c;
        let (middle, carried) = middle_one.overflowing_add(middle_two);
        let (low, carry_low) = low.overflowing_add(middle << 64);
        let high = high + (middle >> 64) + (u128::from(carried) << 64) + u128::from(carry_low);
        (high, low)
    }

    pub(super) fn product(left: i128, right: i128) -> Self {
        let (high, low) = Self::multiply_magnitudes(left.unsigned_abs(), right.unsigned_abs());
        Self {
            negative: (left < 0) != (right < 0) && (high != 0 || low != 0),
            high,
            low,
        }
    }

    /// `self * factor`, or `None` past 256 bits.
    pub(super) fn times(self, factor: u128) -> Option<Self> {
        let (carry, low) = Self::multiply_magnitudes(self.low, factor);
        let (overflow, high) = Self::multiply_magnitudes(self.high, factor);
        if overflow != 0 {
            return None;
        }
        let high = high.checked_add(carry)?;
        Some(Self { low, high, ..self })
    }

    fn magnitude_order(self, other: Self) -> Ordering {
        (self.high, self.low).cmp(&(other.high, other.low))
    }

    fn magnitude_add(self, other: Self) -> Option<(u128, u128)> {
        let (low, carry) = self.low.overflowing_add(other.low);
        let high = self
            .high
            .checked_add(other.high)?
            .checked_add(u128::from(carry))?;
        Some((high, low))
    }

    /// `self - other` for magnitudes with `self >= other`.
    fn magnitude_sub(self, other: Self) -> (u128, u128) {
        let (low, borrow) = self.low.overflowing_sub(other.low);
        (self.high - other.high - u128::from(borrow), low)
    }

    pub(super) fn to_i128(self) -> Option<i128> {
        if self.high != 0 {
            return None;
        }
        if self.negative {
            if self.low == 1u128 << 127 {
                return Some(i128::MIN);
            }
            return i128::try_from(self.low).ok().map(|value| -value);
        }
        i128::try_from(self.low).ok()
    }

    /// `self / divisor`, rounded half-to-even, when the quotient fits an
    /// `i128`. Long division, one bit at a time.
    pub(super) fn divide_half_even(self, divisor: Self) -> Option<i128> {
        if divisor.is_zero() {
            return None;
        }
        let mut quotient = Self {
            negative: false,
            high: 0,
            low: 0,
        };
        let mut remainder = Self {
            negative: false,
            high: 0,
            low: 0,
        };
        let magnitude = Self {
            negative: false,
            ..divisor
        };
        for bit in (0..256).rev() {
            let carried = remainder.high >> 127;
            if carried != 0 {
                return None;
            }
            remainder.high = (remainder.high << 1) | (remainder.low >> 127);
            remainder.low <<= 1;
            let set = if bit >= 128 {
                (self.high >> (bit - 128)) & 1
            } else {
                (self.low >> bit) & 1
            };
            remainder.low |= set;
            quotient.high = (quotient.high << 1) | (quotient.low >> 127);
            quotient.low <<= 1;
            if remainder.magnitude_order(magnitude) != Ordering::Less {
                let (high, low) = remainder.magnitude_sub(magnitude);
                remainder.high = high;
                remainder.low = low;
                quotient.low |= 1;
            }
        }
        let twice = remainder.magnitude_add(remainder)?;
        let twice = Self {
            negative: false,
            high: twice.0,
            low: twice.1,
        };
        let round_away = match twice.magnitude_order(magnitude) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => quotient.low & 1 == 1,
        };
        let mut value = quotient.to_i128()?;
        if round_away {
            value = value.checked_add(1)?;
        }
        if self.negative != divisor.negative {
            value = -value;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::Big;

    #[test]
    fn products_past_i128_divide_back() {
        let left = (1i128 << 119) - 1;
        let product = Big::product(left, -(10i128.pow(30)));
        assert_eq!(product.to_i128(), None);
        let back = product.divide_half_even(Big::from_i128(10i128.pow(30)));
        assert_eq!(back, Some(-left));
    }

    #[test]
    fn division_rounds_half_to_even() {
        assert_eq!(
            Big::from_i128(25).divide_half_even(Big::from_i128(10)),
            Some(2)
        );
        assert_eq!(
            Big::from_i128(35).divide_half_even(Big::from_i128(10)),
            Some(4)
        );
        assert_eq!(
            Big::from_i128(-35).divide_half_even(Big::from_i128(10)),
            Some(-4)
        );
    }
}
