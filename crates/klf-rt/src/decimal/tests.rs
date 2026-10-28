use super::*;

fn number(text: &str) -> [u64; 2] {
    Decimal::parse(text).expect("in range").words()
}

fn error(error: DecimalError) -> [u64; 2] {
    [0, error_second(error)]
}

/// The math function `code` on `text`, as the text of its answer.
fn math(code: u64, text: &str) -> [u64; 2] {
    perform(MATH_BASE + code, number(text), [0, 0])
}

fn math2(code: u64, left: &str, right: &str) -> [u64; 2] {
    perform(MATH_BASE + code, number(left), number(right))
}

#[test]
fn a_tenth_and_two_tenths_make_three_tenths() {
    assert_eq!(perform(0, number("0.1"), number("0.2")), number("0.3"));
}

#[test]
fn fractional_geometry_can_be_added_to_window_coordinates() {
    assert_eq!(
        perform(0, number("9.010000101327896117"), number("59.015625")),
        number("68.02562510132789612")
    );
    assert_eq!(
        perform(1, number("700"), number("9.010000101327896117")),
        number("690.9899998986721039")
    );
    assert_eq!(
        perform(0, number("18446744073709551615"), number("1")),
        error(DecimalError::Overflow)
    );
}

#[test]
fn equal_values_are_the_same_two_words() {
    assert_eq!(number("1.0"), number("1.00"));
    assert_eq!(number("1.0"), number("1"));
    assert_eq!(number("1.50"), [15, 1]);
}

#[test]
fn the_mantissa_holds_every_i64_and_u64() {
    assert_eq!(number("9223372036854775807"), [i64::MAX as u64, 0]);
    assert_eq!(number("0.000000000000000001"), [1, 18]);
    // Past `I64.max` the second word sets `EXTENDED`.
    assert_eq!(
        perform(0, number("9223372036854775807"), number("1")),
        [1 << 63, EXTENDED]
    );
    assert_eq!(number("18446744073709551615"), [u64::MAX, EXTENDED]);
    assert_eq!(number("-18446744073709551616"), [0, EXTENDED]);
    assert_eq!(number("-9223372036854775809"), [i64::MAX as u64, EXTENDED]);
    assert_eq!(number("1844674407370955161.5"), [u64::MAX, 1 | EXTENDED]);
    assert_eq!(
        perform(0, number("18446744073709551615"), number("1")),
        error(DecimalError::Overflow)
    );
    assert_eq!(
        perform(
            1,
            number("18446744073709551615"),
            number("18446744073709551614")
        ),
        number("1")
    );
    assert_eq!(
        Decimal::parse("18446744073709551616"),
        Err(DecimalError::Overflow)
    );
}

#[test]
fn whole_numbers_convert_to_u64_and_back() {
    assert_eq!(
        perform(10, number("18446744073709551615"), [0, 0]),
        [u64::MAX, 0]
    );
    assert_eq!(
        perform(10, number("1844674407370955161.5"), [0, 0]),
        [1844674407370955161, 0]
    );
    assert_eq!(
        perform(10, number("-1"), [0, 0]),
        error(DecimalError::Overflow)
    );
    assert_eq!(
        perform(7, number("9223372036854775808"), [0, 0]),
        error(DecimalError::Overflow)
    );
    assert_eq!(
        perform(7, number("-9223372036854775808"), [0, 0]),
        [i64::MIN as u64, 0]
    );
}

#[test]
fn bitwise_operations_work_on_whole_numbers() {
    let all = number("18446744073709551615");
    assert_eq!(perform(11, all, number("255")), number("255"));
    assert_eq!(
        perform(12, number("9223372036854775808"), number("1")),
        number("9223372036854775809")
    );
    assert_eq!(
        perform(13, all, number("18446744073709551614")),
        number("1")
    );
    assert_eq!(
        perform(14, number("1"), number("63")),
        number("9223372036854775808")
    );
    assert_eq!(
        perform(14, number("1"), number("64")),
        error(DecimalError::Overflow)
    );
    assert_eq!(perform(15, all, number("60")), number("15"));
    assert_eq!(perform(15, number("-8"), number("1")), number("-4"));
    assert_eq!(perform(16, number("0"), [0, 0]), number("-1"));
    assert_eq!(perform(16, all, [0, 0]), number("-18446744073709551616"));
    assert_eq!(
        perform(11, number("1.5"), number("1")),
        error(DecimalError::Domain)
    );
    assert_eq!(
        perform(14, number("1"), number("65")),
        error(DecimalError::Domain)
    );
}

#[test]
fn products_keep_the_widest_scale_that_fits() {
    assert_eq!(perform(2, number("1.5"), number("1.5")), number("2.25"));
    let product = perform(2, number("0.123456789123456789"), number("3"));
    assert_eq!(product, number("0.370370367370370367"));
}

#[test]
fn division_rounds_half_to_even() {
    assert_eq!(
        perform(3, number("1"), number("3")),
        number("0.333333333333333333")
    );
    assert_eq!(perform(3, number("1"), number("8")), number("0.125"));
    assert_eq!(
        perform(3, number("2"), number("3")),
        number("0.666666666666666667")
    );
}

#[test]
fn division_by_zero_is_an_error() {
    assert_eq!(
        perform(3, number("1"), number("0")),
        error(DecimalError::DivideByZero)
    );
}

#[test]
fn conversions_truncate_and_order() {
    assert_eq!(perform(7, number("-9.99"), [0, 0])[0] as i64, -9);
    assert_eq!(perform(5, number("1.5"), number("1.25"))[0], 1);
    assert_eq!(perform(5, number("-2"), number("1"))[0], u64::MAX);
    assert_eq!(f64::from_bits(perform(9, number("0.1"), [0, 0])[0]), 0.1);
    assert_eq!(perform(8, [0.25f64.to_bits(), 0], [0, 0]), number("0.25"));
}

#[test]
fn the_service_keeps_the_second_word() {
    assert_eq!(service(2, number("1.5"), number("1.5")), 225);
    assert_eq!(service_second(), 2);
    let _ = service(3, number("1"), number("0"));
    assert_eq!(service_second(), error_second(DecimalError::DivideByZero));
}

#[test]
fn square_roots_are_decimal() {
    assert_eq!(math(0, "2"), number("1.414213562373095049"));
    assert_eq!(math(0, "4"), number("2"));
    assert_eq!(math(0, "0.0001"), number("0.01"));
    assert_eq!(math(0, "-1"), error(DecimalError::Domain));
}

#[test]
fn trigonometry_is_decimal() {
    assert_eq!(math(1, "1"), number("0.841470984807896507"));
    assert_eq!(math(2, "1"), number("0.540302305868139717"));
    assert_eq!(
        math(3, "0.7853981633974483"),
        number("0.999999999999999981")
    );
    assert_eq!(math(1, "3.141592653589793238"), number("0"));
    assert_eq!(math(2, "0"), number("1"));
}

#[test]
fn large_angles_reduce_exactly() {
    assert_eq!(math(1, "1000000"), number("-0.349993502171292952"));
    assert_eq!(
        math(1, "-123456789.123456789"),
        number("-0.999850930871930925")
    );
    assert_eq!(
        math(2, "9000000000000000000"),
        number("0.876167011211776074")
    );
}

#[test]
fn inverse_trigonometry_is_decimal() {
    assert_eq!(math(16, "1"), number("0.78539816339744831"));
    assert_eq!(math(16, "3"), number("1.249045772398254426"));
    assert_eq!(math(14, "1"), number("1.570796326794896619"));
    assert_eq!(math(15, "1"), number("0"));
    assert_eq!(math(14, "1.5"), error(DecimalError::Domain));
    assert_eq!(math2(21, "1", "-1"), number("2.356194490192344929"));
}

#[test]
fn exponentials_and_logarithms_are_decimal() {
    assert_eq!(math(7, "1"), number("2.718281828459045235"));
    assert_eq!(math(7, "-5"), number("0.006737946999085467"));
    assert_eq!(math(8, "2"), number("0.693147180559945309"));
    assert_eq!(math(8, "123456.789"), number("11.723646487185880981"));
    assert_eq!(math(8, "1"), number("0"));
    assert_eq!(math(10, "1000"), number("3"));
    assert_eq!(math(9, "8"), number("3"));
    assert_eq!(math(11, "10"), number("1024"));
    assert_eq!(math(8, "0"), error(DecimalError::Domain));
    assert_eq!(math(7, "100"), error(DecimalError::Overflow));
}

#[test]
fn powers_are_exact_for_integer_exponents() {
    assert_eq!(math2(20, "1.1", "2"), number("1.21"));
    assert_eq!(math2(20, "2", "-2"), number("0.25"));
    assert_eq!(math2(20, "10", "18"), number("1000000000000000000"));
    assert_eq!(math2(20, "10", "19"), number("10000000000000000000"));
    assert_eq!(math2(20, "10", "20"), error(DecimalError::Overflow));
    assert_eq!(math2(20, "4", "0.5"), number("2"));
    assert_eq!(math2(20, "-4", "0.5"), error(DecimalError::Domain));
}

#[test]
fn rounding_functions_are_exact() {
    assert_eq!(math(4, "-1.5"), number("-2"));
    assert_eq!(math(5, "-1.5"), number("-1"));
    assert_eq!(math(5, "1.2"), number("2"));
    assert_eq!(math(12, "2.5"), number("3"));
    assert_eq!(math(12, "-2.5"), number("-3"));
    assert_eq!(math(13, "-2.7"), number("-2"));
    assert_eq!(math(6, "-0.5"), number("0.5"));
}

#[test]
fn binary_functions_are_exact() {
    assert_eq!(math2(22, "0.1", "0.2"), number("0.1"));
    assert_eq!(math2(23, "0.1", "0.2"), number("0.2"));
    assert_eq!(math2(24, "3", "4"), number("5"));
    assert_eq!(math2(25, "1.5", "-2"), number("-1.5"));
    assert_eq!(math2(26, "5.5", "2"), number("1.5"));
    assert_eq!(math2(26, "-5.5", "2"), number("-1.5"));
    assert_eq!(math2(26, "1", "0"), error(DecimalError::Domain));
}

#[test]
fn hyperbolic_functions_are_decimal() {
    assert_eq!(math(17, "0"), number("0"));
    assert_eq!(math(18, "0"), number("1"));
    assert_eq!(math(19, "30"), number("1"));
    assert_eq!(math(27, "27"), number("3"));
    assert_eq!(math(27, "-8"), number("-2"));
}

/// The decimal Rust's exact formatting gives an `f64` at the widest scale
/// that fits, which `from_f64` must match without the text.
fn formatted(value: f64) -> Result<Decimal, DecimalError> {
    for places in (0..=MAX_SCALE as usize).rev() {
        if let Ok(decimal) = Decimal::parse(&format!("{value:.places$}")) {
            return Ok(decimal);
        }
    }
    Err(DecimalError::Overflow)
}

#[test]
fn floats_convert_as_their_exact_text_rounds() {
    let mut values = vec![
        0.0,
        -0.0,
        0.1,
        0.5,
        1.5,
        2.5,
        -2.5,
        0.125,
        1e-18,
        5e-19,
        1.5e-18,
        2.5e-18,
        1e-19,
        f64::MIN_POSITIVE,
        5e-324,
        1e18,
        1.8446744073709552e19,
        1.844674407370955e19,
        9.223372036854776e18,
        123456.789,
        -1.0,
        1e300,
        -1e300,
        0.30000000000000004,
    ];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..200_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = f64::from_bits(state);
        if value.is_finite() {
            values.push(value);
        }
        // Values near the ranges that matter: small exponents and fractions.
        let near = f64::from_bits(
            (state & 0x800F_FFFF_FFFF_FFFF) | ((0x3A0 + (state >> 52) % 0xC0) << 52),
        );
        values.push(near);
    }
    for value in values {
        let direct = Decimal::from_f64(value).map(Decimal::words);
        let through = formatted(value).map(Decimal::words);
        assert_eq!(direct, through, "{value:e}");
    }
}
