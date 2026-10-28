//! The call trampoline for the System V and AAPCS64 conventions, which
//! place integer arguments in the general registers and floating-point ones
//! in the vector registers, each class in order, and the rest on the stack.
//! The general shape passes every argument register and a fixed stack run,
//! of which the callee reads its own.
#![allow(unsafe_code, clippy::too_many_arguments)]

use super::{CallPlan, ForeignClass, Layout, call_f64, call_integer};

#[cfg(target_arch = "aarch64")]
macro_rules! shape {
    ($result:ty) => {
        extern "C" fn(
            u64, u64, u64, u64, u64, u64, u64, u64,
            f64, f64, f64, f64, f64, f64, f64, f64,
            u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64,
            u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64,
        ) -> $result
    };
}

#[cfg(target_arch = "x86_64")]
macro_rules! shape {
    ($result:ty) => {
        extern "C" fn(
            u64, u64, u64, u64, u64, u64,
            f64, f64, f64, f64, f64, f64, f64, f64,
            u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64,
            u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64,
        ) -> $result
    };
}

macro_rules! invoke {
    ($f:expr, $i:expr, $d:expr, $s:expr) => {{
        let (i, d, s) = ($i, $d, $s);
        #[cfg(target_arch = "aarch64")]
        let value = $f(
            i[0], i[1], i[2], i[3], i[4], i[5], i[6], i[7], d[0], d[1], d[2], d[3], d[4], d[5],
            d[6], d[7], s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], s[10], s[11],
            s[12], s[13], s[14], s[15], s[16], s[17], s[18], s[19], s[20], s[21], s[22], s[23],
            s[24], s[25], s[26], s[27], s[28], s[29], s[30], s[31],
        );
        #[cfg(target_arch = "x86_64")]
        let value = $f(
            i[0], i[1], i[2], i[3], i[4], i[5], d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7],
            s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], s[10], s[11], s[12], s[13],
            s[14], s[15], s[16], s[17], s[18], s[19], s[20], s[21], s[22], s[23], s[24], s[25],
            s[26], s[27], s[28], s[29], s[30], s[31],
        );
        value
    }};
}

pub fn call(address: usize, plan: &CallPlan, arguments: &[u64]) -> Result<u64, String> {
    if plan.parameters.len() != arguments.len() || !plan.valid {
        return Err("KLF-VM-0005".to_owned());
    }
    if let Some(arity) = plan.integer_arity {
        let mut words = [0; 8];
        for (index, ((class, _), value)) in plan.parameters.iter().zip(arguments).enumerate() {
            words[index] = super::argument_word(*class, *value);
        }
        return Ok(super::result_word(
            plan.result,
            call_integer(address, arity, words),
        ));
    }
    if let Some(arity) = plan.f64_arity {
        return Ok(call_f64(address, arity, arguments).to_bits());
    }
    let layout = Layout::of(plan, arguments)?;
    let floats = layout.floats.map(f64::from_bits);
    let pointer = address as *const ();
    // SAFETY: `address` is a function resolved from the process's loaded
    // images and the KIR verifier trusts its declaration (an unmanaged
    // instruction, section 11.5). The shape passes every argument
    // register and the stack run, of which the callee reads its own.
    let value = unsafe {
        if let Some(ForeignClass::F32 | ForeignClass::F64) = plan.result {
            let f: shape!(f64) = std::mem::transmute(pointer);
            invoke!(f, layout.integers, floats, layout.stack).to_bits()
        } else {
            let f: shape!(u64) = std::mem::transmute(pointer);
            invoke!(f, layout.integers, floats, layout.stack)
        }
    };
    Ok(super::result_word(plan.result, value))
}

#[cfg(test)]
mod tests {
    use super::call;
    use crate::foreign::{ForeignClass, ForeignSignature, plan::CallPlan};

    /// A foreign function whose parameters are exactly the ones a variadic
    /// C declaration of `lilids` shape carries after promotion: a pointer,
    /// three `long`s, a `double` and another pointer. Every one of them is
    /// folded into the result, so a value that arrived in the wrong
    /// register changes the answer.
    extern "C" fn mixed(
        kinds: u64,
        first: i64,
        second: i64,
        third: i64,
        half: f64,
        extra: u64,
    ) -> u64 {
        kinds ^ first as u64 ^ second as u64 ^ third as u64 ^ half.to_bits() ^ extra
    }

    /// A `double` crosses in the float registers, counted apart from the
    /// integer ones, so a shape that mixes a pointer, three `long`s and a
    /// `double` still puts the `double` in the first float register. The
    /// integer-only fast path would have taken the whole call instead.
    #[test]
    fn a_float_argument_reaches_the_float_register_and_not_the_integer_one() {
        let plan = CallPlan::new(&ForeignSignature {
            parameters: vec![
                ForeignClass::Unsigned(64),
                ForeignClass::Signed(64),
                ForeignClass::Signed(64),
                ForeignClass::Signed(64),
                ForeignClass::F64,
                ForeignClass::Unsigned(64),
            ],
            result: Some(ForeignClass::Signed(64)),
        });
        let got = call(
            mixed as *const () as usize,
            &plan,
            &[11, 1, 2, 3, 4.5f64.to_bits(), 1],
        )
        .expect("the mixed call is prepared");
        assert_eq!(got, 0xB ^ 0x1 ^ 0x2 ^ 0x3 ^ 4.5f64.to_bits() ^ 0x1);
    }

    /// A call whose arguments are all `double`s and whose result is one
    /// goes down the dedicated path, which has to agree with the general
    /// one rather than being a second answer.
    #[test]
    fn the_all_float_path_agrees_with_the_general_one() {
        let all = CallPlan::new(&ForeignSignature {
            parameters: vec![ForeignClass::F64, ForeignClass::F64],
            result: Some(ForeignClass::Unsigned(64)),
        });
        let pair = [0.5f64.to_bits().to_owned(), 2.25f64.to_bits().to_owned()];
        let general = CallPlan::new(&ForeignSignature {
            parameters: vec![
                ForeignClass::Unsigned(64),
                ForeignClass::F64,
                ForeignClass::F64,
            ],
            result: Some(ForeignClass::Unsigned(64)),
        });
        assert_eq!(
            call(two_floats as *const () as usize, &all, &pair).unwrap(),
            call(
                two_floats as *const () as usize,
                &general,
                &[0, pair[0], pair[1]]
            )
            .unwrap(),
        );
    }

    extern "C" fn two_floats(first: f64, second: f64) -> u64 {
        first.to_bits() ^ second.to_bits()
    }
}
