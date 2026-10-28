//! Platform-independent floating-point kernel math.
//! Operation numbers are the Kira MathOp ABI. libm keeps the same software
//! implementation available on every target instead of inheriting host libc.

#[must_use]
pub fn unary(operation: u64, value: f64) -> Option<f64> {
    Some(match operation {
        0 => libm::sqrt(value),
        1 => libm::sin(value),
        2 => libm::cos(value),
        3 => libm::tan(value),
        4 => libm::floor(value),
        5 => libm::ceil(value),
        6 => libm::fabs(value),
        7 => libm::exp(value),
        8 => libm::log(value),
        9 => libm::log2(value),
        10 => libm::log10(value),
        11 => libm::exp2(value),
        12 => libm::round(value),
        13 => libm::trunc(value),
        14 => libm::asin(value),
        15 => libm::acos(value),
        16 => libm::atan(value),
        17 => libm::sinh(value),
        18 => libm::cosh(value),
        19 => libm::tanh(value),
        27 => libm::cbrt(value),
        _ => return None,
    })
}

#[must_use]
pub fn binary(operation: u64, left: f64, right: f64) -> Option<f64> {
    Some(match operation {
        20 => libm::pow(left, right),
        21 => libm::atan2(left, right),
        22 => libm::fmin(left, right),
        23 => libm::fmax(left, right),
        24 => libm::hypot(left, right),
        25 => libm::copysign(left, right),
        26 => libm::fmod(left, right),
        _ => return None,
    })
}
