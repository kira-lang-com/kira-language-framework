//! The math functions on `Number`, computed in decimal. Operation numbers are
//! the `klf.math` ones. Each function works at twenty-eight digits and rounds
//! once to a `Number`; `floor`, `fmod` and their kind are exact.

use core::cmp::Ordering;

use super::wide::{HALF, HALF_PI, LN2, LN10, ONE, PI, Wide, ZERO};
use super::{Decimal, DecimalError};

/// π/2 as `P1 * 10^-18 + D2 * 10^-36 + D3 * 10^-54`, for exact argument
/// reduction.
const HALF_PI_1: i128 = 1_570_796_326_794_896_619;
const HALF_PI_2: i128 = 231_321_691_639_751_442;
const HALF_PI_3: u128 = 98_584_699_687_552_910;

/// Newton steps from an `f64` seed: two reach working precision, the third
/// settles the last digit.
const NEWTON_STEPS: usize = 3;

/// Whether the operation takes two operands.
#[must_use]
pub fn is_binary(code: u64) -> bool {
    (20..=26).contains(&code)
}

pub fn unary(code: u64, value: Decimal) -> Result<Decimal, DecimalError> {
    let x = Wide::from_decimal(value);
    let answer = match code {
        0 => sqrt(x)?,
        1 => sin_cos(value).0,
        2 => sin_cos(value).1,
        3 => {
            let (sine, cosine) = sin_cos(value);
            sine.divide(cosine)?
        }
        4 => return floor(value),
        5 => return ceil(value),
        6 => return abs(value),
        7 => exp(x)?,
        8 => log(x)?,
        9 => log(x)?.divide(LN2)?,
        10 => log(x)?.divide(LN10)?,
        11 => {
            if value.scale() == 0 {
                return power(Decimal::from_i64(2), value);
            }
            exp(x.multiply(LN2))?
        }
        12 => return round(value),
        13 => return trunc(value),
        14 => asin(x)?,
        15 => HALF_PI.subtract(asin(x)?),
        16 => atan(x)?,
        17 => sinh(x)?,
        18 => cosh(x)?,
        19 => tanh(x)?,
        27 => cbrt(x)?,
        _ => return Err(DecimalError::Parse),
    };
    answer.to_decimal()
}

pub fn binary(code: u64, left: Decimal, right: Decimal) -> Result<Decimal, DecimalError> {
    match code {
        20 => power(left, right),
        21 => atan2(Wide::from_decimal(left), Wide::from_decimal(right))?.to_decimal(),
        22 => Ok(if right.compare(left) == Ordering::Less {
            right
        } else {
            left
        }),
        23 => Ok(if right.compare(left) == Ordering::Greater {
            right
        } else {
            left
        }),
        24 => {
            let (a, b) = (Wide::from_decimal(left), Wide::from_decimal(right));
            sqrt(a.multiply(a).add(b.multiply(b)))?.to_decimal()
        }
        25 => {
            let magnitude = abs(left)?;
            if right.mantissa() < 0 {
                magnitude.checked_neg()
            } else {
                Ok(magnitude)
            }
        }
        26 => remainder(left, right),
        _ => Err(DecimalError::Parse),
    }
}

fn abs(value: Decimal) -> Result<Decimal, DecimalError> {
    if value.mantissa() < 0 {
        value.checked_neg()
    } else {
        Ok(value)
    }
}

fn trunc(value: Decimal) -> Result<Decimal, DecimalError> {
    super::narrow(value.integer_part(), 0)
}

fn floor(value: Decimal) -> Result<Decimal, DecimalError> {
    let unit = 10i128.pow(value.scale());
    super::narrow(value.mantissa().div_euclid(unit), 0)
}

fn ceil(value: Decimal) -> Result<Decimal, DecimalError> {
    let unit = 10i128.pow(value.scale());
    super::narrow(-((-value.mantissa()).div_euclid(unit)), 0)
}

/// Half away from zero, as `round` does on a `Float`.
fn round(value: Decimal) -> Result<Decimal, DecimalError> {
    let unit = 10i128.pow(value.scale());
    let mantissa = value.mantissa();
    let magnitude = (mantissa.abs() + unit / 2) / unit;
    super::narrow(if mantissa < 0 { -magnitude } else { magnitude }, 0)
}

/// `left - trunc(left / right) * right`, exact, with the sign of `left`.
fn remainder(left: Decimal, right: Decimal) -> Result<Decimal, DecimalError> {
    if right.mantissa() == 0 {
        return Err(DecimalError::Domain);
    }
    let scale = left.scale().max(right.scale());
    let a = left.mantissa() * 10i128.pow(scale - left.scale());
    let b = right.mantissa() * 10i128.pow(scale - right.scale());
    super::narrow(a % b, scale)
}

/// Adds terms from `next` to `first` until one no longer counts.
fn series(
    first: Wide,
    mut next: impl FnMut(Wide, i64) -> Result<Wide, DecimalError>,
) -> Result<Wide, DecimalError> {
    let mut sum = first;
    let mut term = first;
    for index in 1..200 {
        term = next(term, index)?;
        if term.negligible_beside(sum) {
            break;
        }
        sum = sum.add(term);
    }
    Ok(sum)
}

/// sin and cos of `r` with |r| at most about π/4, by Taylor series.
fn sin_cos_reduced(r: Wide) -> Result<(Wide, Wide), DecimalError> {
    let square = r.multiply(r);
    let sine = series(r, |term, n| {
        term.multiply(square)
            .divide(Wide::from_int(2 * n * (2 * n + 1)))
            .map(Wide::negate)
    })?;
    let cosine = series(ONE, |term, n| {
        term.multiply(square)
            .divide(Wide::from_int((2 * n - 1) * (2 * n)))
            .map(Wide::negate)
    })?;
    Ok((sine, cosine))
}

/// sin and cos of a `Number`, reduced exactly by a multiple of π/2 first.
fn sin_cos(value: Decimal) -> (Wide, Wide) {
    let x = Wide::from_decimal(value);
    let quarter = x
        .divide(HALF_PI)
        .ok()
        .and_then(Wide::round_to_int)
        .unwrap_or(0);
    // r = x - k*π/2 with π/2 in three parts: the first two products are
    // exact in 128 bits, the third leaves an error near k * 10^-54.
    let k = i128::from(quarter);
    let scaled = value.mantissa() * 10i128.pow(18 - value.scale());
    let first = scaled - k * HALF_PI_1;
    let second = first * 10i128.pow(18) - k * HALF_PI_2;
    let third = u128::from(quarter.unsigned_abs()) * HALF_PI_3;
    let r = Wide::new(second < 0, second.unsigned_abs(), -36).subtract(Wide::new(
        quarter < 0,
        third,
        -54,
    ));
    let (sine, cosine) = sin_cos_reduced(r).unwrap_or((ZERO, ONE));
    match quarter.rem_euclid(4) {
        0 => (sine, cosine),
        1 => (cosine, sine.negate()),
        2 => (sine.negate(), cosine.negate()),
        _ => (cosine.negate(), sine),
    }
}

fn exp(x: Wide) -> Result<Wide, DecimalError> {
    if x.is_zero() {
        return Ok(ONE);
    }
    // Past these the answer overflows a Number or rounds to zero.
    if x.magnitude() > 3 {
        return if x.is_negative() {
            Ok(ZERO)
        } else {
            Err(DecimalError::Overflow)
        };
    }
    // e^x = 10^k * e^r with r = x - k*ln 10 and |r| under 1.2.
    let k = x
        .divide(LN10)?
        .round_to_int()
        .ok_or(DecimalError::Overflow)?;
    let r = x.subtract(LN10.multiply(Wide::from_int(k)));
    let sum = series(ONE, |term, n| term.multiply(r).divide(Wide::from_int(n)))?;
    Ok(sum.scaled_by_power_of_ten(k))
}

fn log(a: Wide) -> Result<Wide, DecimalError> {
    if a.is_zero() || a.is_negative() {
        return Err(DecimalError::Domain);
    }
    // a = t * 10^j with t in [1, 10); Halley's step on e^y = t.
    let j = a.magnitude();
    let t = a.scaled_by_power_of_ten(-j);
    let mut y = Wide::from_f64(t.to_f64().ln());
    for _ in 0..NEWTON_STEPS {
        let power = exp(y)?;
        let step = t.subtract(power).divide(t.add(power))?;
        y = y.add(step.add(step));
    }
    Ok(y.add(LN10.multiply(Wide::from_int(j))))
}

fn sqrt(a: Wide) -> Result<Wide, DecimalError> {
    if a.is_negative() {
        return Err(DecimalError::Domain);
    }
    if a.is_zero() {
        return Ok(ZERO);
    }
    let mut y = Wide::from_f64(a.to_f64().sqrt());
    for _ in 0..NEWTON_STEPS {
        y = y.add(a.divide(y)?).multiply(HALF);
    }
    Ok(y)
}

fn cbrt(a: Wide) -> Result<Wide, DecimalError> {
    if a.is_zero() {
        return Ok(ZERO);
    }
    let magnitude = a.abs();
    let mut y = Wide::from_f64(magnitude.to_f64().cbrt());
    let three = Wide::from_int(3);
    for _ in 0..NEWTON_STEPS {
        y = y
            .add(y)
            .add(magnitude.divide(y.multiply(y))?)
            .divide(three)?;
    }
    Ok(if a.is_negative() { y.negate() } else { y })
}

fn atan(x: Wide) -> Result<Wide, DecimalError> {
    if x.is_zero() {
        return Ok(ZERO);
    }
    if x.is_negative() {
        return atan(x.negate()).map(Wide::negate);
    }
    if x.subtract(ONE).is_negative() || x == ONE {
        // Newton on sin y - x cos y = 0, whose derivative is 1/cos y at
        // the root; y stays within π/4, where the series needs no reduction.
        let mut y = Wide::from_f64(x.to_f64().atan());
        for _ in 0..NEWTON_STEPS {
            let (sine, cosine) = sin_cos_reduced(y)?;
            y = y.subtract(cosine.multiply(sine.subtract(x.multiply(cosine))));
        }
        return Ok(y);
    }
    Ok(HALF_PI.subtract(atan(ONE.divide(x)?)?))
}

fn asin(x: Wide) -> Result<Wide, DecimalError> {
    let excess = x.abs().subtract(ONE);
    if excess.is_zero() {
        return Ok(if x.is_negative() {
            HALF_PI.negate()
        } else {
            HALF_PI
        });
    }
    if !excess.is_negative() {
        return Err(DecimalError::Domain);
    }
    let cosine = sqrt(ONE.subtract(x).multiply(ONE.add(x)))?;
    atan(x.divide(cosine)?)
}

fn atan2(y: Wide, x: Wide) -> Result<Wide, DecimalError> {
    if x.is_zero() {
        return Ok(if y.is_zero() {
            ZERO
        } else if y.is_negative() {
            HALF_PI.negate()
        } else {
            HALF_PI
        });
    }
    let angle = atan(y.divide(x)?)?;
    if !x.is_negative() {
        return Ok(angle);
    }
    Ok(if y.is_negative() {
        angle.subtract(PI)
    } else {
        angle.add(PI)
    })
}

fn sinh(x: Wide) -> Result<Wide, DecimalError> {
    if x.abs().subtract(ONE).is_negative() {
        // The series keeps the digits e^x - e^-x would cancel.
        let square = x.multiply(x);
        return series(x, |term, n| {
            term.multiply(square)
                .divide(Wide::from_int(2 * n * (2 * n + 1)))
        });
    }
    Ok(exp(x)?.subtract(exp(x.negate())?).multiply(HALF))
}

fn cosh(x: Wide) -> Result<Wide, DecimalError> {
    Ok(exp(x)?.add(exp(x.negate())?).multiply(HALF))
}

fn tanh(x: Wide) -> Result<Wide, DecimalError> {
    // tanh 25 is 1 - 4e-22: one at a Number's eighteen digits.
    if x.abs().subtract(Wide::from_int(25)).is_negative() {
        return sinh(x)?.divide(cosh(x)?);
    }
    Ok(if x.is_negative() { ONE.negate() } else { ONE })
}

/// `base` to `exponent`: exact by squaring for an integer exponent, else
/// `exp(exponent * log base)`.
fn power(base: Decimal, exponent: Decimal) -> Result<Decimal, DecimalError> {
    let b = Wide::from_decimal(base);
    if exponent.scale() == 0 {
        let n = exponent.mantissa();
        if n == 0 {
            return Ok(Decimal::from_i64(1));
        }
        if b.is_zero() {
            return if n < 0 {
                Err(DecimalError::DivideByZero)
            } else {
                Ok(Decimal::ZERO)
            };
        }
        let grows = ONE.subtract(b.abs()).is_negative();
        let mut result = ONE;
        let mut square = b;
        let mut remaining = n.unsigned_abs();
        while remaining > 0 {
            if remaining & 1 == 1 {
                result = result.multiply(square);
            }
            remaining >>= 1;
            if remaining > 0 {
                square = square.multiply(square);
            }
            // Past these the answer overflows a Number or rounds to zero, and
            // the exponent would soon overflow too.
            if result.magnitude().abs() > 60 || square.magnitude().abs() > 1_000_000 {
                return if grows == (n > 0) {
                    Err(DecimalError::Overflow)
                } else {
                    Ok(Decimal::ZERO)
                };
            }
        }
        if n < 0 {
            result = ONE.divide(result)?;
        }
        return result.to_decimal();
    }
    if b.is_negative() {
        return Err(DecimalError::Domain);
    }
    if b.is_zero() {
        return if exponent.mantissa() < 0 {
            Err(DecimalError::DivideByZero)
        } else {
            Ok(Decimal::ZERO)
        };
    }
    exp(Wide::from_decimal(exponent).multiply(log(b)?))?.to_decimal()
}
