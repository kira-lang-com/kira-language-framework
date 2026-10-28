//! The parameters stand for argument slots of a C signature, so they are
//! positional and get positional names.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

use super::{CallPlan, ForeignClass, ForeignSignature, call};

extern "C" fn narrow(a: i8, b: u16, c: i32, d: u64, e: u32, f: u64) -> u64 {
    (i64::from(a) + i64::from(b) + i64::from(c) + d as i64 + i64::from(e) + f as i64) as u64
}

extern "C" fn stacked(
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
    g: u64,
    h: u64,
    i: u8,
    j: u16,
) -> u64 {
    a + b + c + d + e + f + g + h + u64::from(i) + u64::from(j)
}

extern "C" fn mixed(a: f32, b: u64, c: f64) -> f64 {
    f64::from(a) + b as f64 + c
}

#[test]
fn prepared_native_calls_preserve_register_and_stack_abis() {
    let plan = CallPlan::new(&ForeignSignature {
        parameters: vec![
            ForeignClass::Signed(8),
            ForeignClass::Unsigned(16),
            ForeignClass::Signed(32),
            ForeignClass::Unsigned(64),
            ForeignClass::Unsigned(32),
            ForeignClass::Unsigned(64),
        ],
        result: Some(ForeignClass::Unsigned(64)),
    });
    assert_eq!(plan.integer_arity, Some(6));
    assert_eq!(
        call(
            narrow as *const () as usize,
            &plan,
            &[255, 65_538, 4_294_967_293, 10, 20, 30]
        ),
        Ok(58)
    );
    assert_eq!(
        call(narrow as *const () as usize, &plan, &[]),
        Err("KLF-VM-0005".to_owned())
    );

    let mut parameters = vec![ForeignClass::Unsigned(64); 8];
    parameters.extend([ForeignClass::Unsigned(8), ForeignClass::Unsigned(16)]);
    let plan = CallPlan::new(&ForeignSignature {
        parameters,
        result: Some(ForeignClass::Unsigned(64)),
    });
    assert_eq!(plan.integer_arity, None);
    assert_eq!(
        call(
            stacked as *const () as usize,
            &plan,
            &[1, 2, 3, 4, 5, 6, 7, 8, 259, 65_540]
        ),
        Ok(43)
    );

    let plan = CallPlan::new(&ForeignSignature {
        parameters: vec![
            ForeignClass::F32,
            ForeignClass::Unsigned(64),
            ForeignClass::F64,
        ],
        result: Some(ForeignClass::F64),
    });
    assert_eq!(
        call(
            mixed as *const () as usize,
            &plan,
            &[u64::from(1.5f32.to_bits()), 3, 2.25f64.to_bits()]
        ),
        Ok(6.75f64.to_bits())
    );
}

extern "C" fn float_identity(value: f64) -> f64 {
    value
}
extern "C" fn float_pair(left: f64, right: f64) -> f64 {
    left + right
}
extern "C" fn float_zero() -> f64 {
    -0.0
}
extern "C" fn float_eight(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64) -> f64 {
    a + b * 2.0 + c * 3.0 + d * 4.0 + e * 5.0 + f * 6.0 + g * 7.0 + h * 8.0
}
extern "C" fn float_nine(
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
    g: f64,
    h: f64,
    i: f64,
) -> f64 {
    float_eight(a, b, c, d, e, f, g, h) + i * 9.0
}

#[test]
fn exact_f64_calls_preserve_arity_special_bits_and_stack_fallback() {
    let make_plan = |count| {
        CallPlan::new(&ForeignSignature {
            parameters: vec![ForeignClass::F64; count],
            result: Some(ForeignClass::F64),
        })
    };
    let plan = make_plan(0);
    assert_eq!(plan.f64_arity, Some(0));
    assert_eq!(
        call(float_zero as *const () as usize, &plan, &[]),
        Ok((-0.0f64).to_bits())
    );
    let plan = make_plan(1);
    for bits in [
        0,
        (-0.0f64).to_bits(),
        f64::INFINITY.to_bits(),
        0x7ff8_1234_5678_9abc,
    ] {
        assert_eq!(
            call(float_identity as *const () as usize, &plan, &[bits]),
            Ok(bits)
        );
    }
    assert_eq!(
        call(float_identity as *const () as usize, &plan, &[]),
        Err("KLF-VM-0005".to_owned())
    );
    let plan = make_plan(2);
    assert_eq!(
        call(
            float_pair as *const () as usize,
            &plan,
            &[1.25f64.to_bits(), 2.5f64.to_bits()]
        ),
        Ok(3.75f64.to_bits())
    );
    let words = [1.0f64.to_bits(); 9];
    let plan = make_plan(8);
    assert_eq!(plan.f64_arity, Some(8));
    assert_eq!(
        call(float_eight as *const () as usize, &plan, &words[..8]),
        Ok(36.0f64.to_bits())
    );
    let plan = make_plan(9);
    assert_eq!(plan.f64_arity, None);
    assert_eq!(
        call(float_nine as *const () as usize, &plan, &words),
        Ok(45.0f64.to_bits())
    );
    let mixed = CallPlan::new(&ForeignSignature {
        parameters: vec![ForeignClass::F32, ForeignClass::F64],
        result: Some(ForeignClass::F64),
    });
    assert_eq!(mixed.f64_arity, None);
}
