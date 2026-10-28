//! The call trampoline for the Windows x64 convention, which gives each of
//! the first four arguments the register of its position, an integer
//! register for an integer and a floating-point one for a float, and the
//! rest a stack word each. What the callee reads from a position depends on
//! the class of the argument there, so the trampoline calls through one of
//! sixteen shapes, one per choice of classes for those four, each passing a
//! fixed stack run of which the callee reads its own.
#![allow(unsafe_code)]

use super::plan::{Location, POSITIONAL_REGISTERS};
use super::{CallPlan, Layout};

/// The stack words every shape passes.
const STACK_WORDS: usize = 32;

pub fn call(address: usize, plan: &CallPlan, arguments: &[u64]) -> Result<u64, String> {
    if plan.parameters.len() != arguments.len() || !plan.valid {
        return Err("KLF-VM-0005".to_owned());
    }
    // `extern "C"` is this convention only on Windows itself; a test host
    // calls every shape through the general path.
    #[cfg(windows)]
    if let Some(arity) = plan.integer_arity {
        let mut words = [0; 8];
        for (index, ((class, _), value)) in plan.parameters.iter().zip(arguments).enumerate() {
            words[index] = super::argument_word(*class, *value);
        }
        return Ok(super::result_word(
            plan.result,
            super::call_integer(address, arity, words),
        ));
    }
    #[cfg(windows)]
    if let Some(arity) = plan.f64_arity {
        return Ok(super::call_f64(address, arity, arguments).to_bits());
    }
    let layout = Layout::of(plan, arguments)?;
    let registers: [u64; POSITIONAL_REGISTERS] = std::array::from_fn(|position| {
        match plan.parameters.get(position).map(|(_, location)| *location) {
            Some(Location::Float(_)) => layout.floats[position],
            _ => layout.integers[position],
        }
    });
    let stack: [u64; STACK_WORDS] = std::array::from_fn(|index| layout.stack[index]);
    let float_result = plan.float_result();
    // SAFETY: `address` is a function resolved from the process's loaded
    // images and the KIR verifier trusts its declaration (an unmanaged
    // instruction, section 11.5). The shape puts each register argument in
    // the register its class selects, and passes the stack run.
    let value = unsafe {
        match plan.positional_shape() {
            0 => shaped::<u64, u64, u64, u64>(address, registers, stack, float_result),
            1 => shaped::<f64, u64, u64, u64>(address, registers, stack, float_result),
            2 => shaped::<u64, f64, u64, u64>(address, registers, stack, float_result),
            3 => shaped::<f64, f64, u64, u64>(address, registers, stack, float_result),
            4 => shaped::<u64, u64, f64, u64>(address, registers, stack, float_result),
            5 => shaped::<f64, u64, f64, u64>(address, registers, stack, float_result),
            6 => shaped::<u64, f64, f64, u64>(address, registers, stack, float_result),
            7 => shaped::<f64, f64, f64, u64>(address, registers, stack, float_result),
            8 => shaped::<u64, u64, u64, f64>(address, registers, stack, float_result),
            9 => shaped::<f64, u64, u64, f64>(address, registers, stack, float_result),
            10 => shaped::<u64, f64, u64, f64>(address, registers, stack, float_result),
            11 => shaped::<f64, f64, u64, f64>(address, registers, stack, float_result),
            12 => shaped::<u64, u64, f64, f64>(address, registers, stack, float_result),
            13 => shaped::<f64, u64, f64, f64>(address, registers, stack, float_result),
            14 => shaped::<u64, f64, f64, f64>(address, registers, stack, float_result),
            _ => shaped::<f64, f64, f64, f64>(address, registers, stack, float_result),
        }
    };
    Ok(super::result_word(plan.result, value))
}

/// A register argument as the parameter type its class passes in.
trait Register: Copy {
    fn of(word: u64) -> Self;
}

impl Register for u64 {
    fn of(word: u64) -> Self {
        word
    }
}

/// A `float` is the low 32 bits of the register.
impl Register for f64 {
    fn of(word: u64) -> Self {
        f64::from_bits(word)
    }
}

type Shape<R0, R1, R2, R3, T> = extern "win64" fn(
    R0,
    R1,
    R2,
    R3,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
    u64,
) -> T;

/// Calls `address` with the register arguments as `R0` to `R3` and the
/// stack run, returning the result register as a word.
///
/// # Safety
/// `address` must be a function taking its register arguments in the
/// classes `R0` to `R3` and returning in the class `float_result` selects.
unsafe fn shaped<R0: Register, R1: Register, R2: Register, R3: Register>(
    address: usize,
    registers: [u64; POSITIONAL_REGISTERS],
    s: [u64; STACK_WORDS],
    float_result: bool,
) -> u64 {
    let (r0, r1, r2, r3) = (
        R0::of(registers[0]),
        R1::of(registers[1]),
        R2::of(registers[2]),
        R3::of(registers[3]),
    );
    let pointer = address as *const ();
    if float_result {
        // SAFETY: the caller promises a function of this shape.
        let function: Shape<R0, R1, R2, R3, f64> = unsafe { std::mem::transmute(pointer) };
        function(
            r0, r1, r2, r3, s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], s[10],
            s[11], s[12], s[13], s[14], s[15], s[16], s[17], s[18], s[19], s[20], s[21], s[22],
            s[23], s[24], s[25], s[26], s[27], s[28], s[29], s[30], s[31],
        )
        .to_bits()
    } else {
        // SAFETY: the caller promises a function of this shape.
        let function: Shape<R0, R1, R2, R3, u64> = unsafe { std::mem::transmute(pointer) };
        function(
            r0, r1, r2, r3, s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8], s[9], s[10],
            s[11], s[12], s[13], s[14], s[15], s[16], s[17], s[18], s[19], s[20], s[21], s[22],
            s[23], s[24], s[25], s[26], s[27], s[28], s[29], s[30], s[31],
        )
    }
}

#[cfg(test)]
mod tests {
    //! The parameters stand for argument slots of a C signature, so they are
    //! positional and get positional names.
    #![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

    use super::call;
    use crate::foreign::plan::{CallPlan, Convention};
    use crate::program::{ForeignClass, ForeignSignature};

    fn plan(parameters: Vec<ForeignClass>, result: ForeignClass) -> CallPlan {
        CallPlan::with(
            &ForeignSignature {
                parameters,
                result: Some(result),
            },
            Convention::Positional,
        )
    }

    extern "win64" fn mixed(a: i8, b: f32, c: u64, d: f64, e: u16, f: f64, g: f32) -> f64 {
        f64::from(a) + f64::from(b) + c as f64 + d + f64::from(e) + f + f64::from(g)
    }

    extern "win64" fn integers(a: u64, b: i32, c: u64, d: u64, e: u8) -> u64 {
        a + b as u64 + c + d + u64::from(e)
    }

    #[test]
    fn each_argument_reaches_the_register_or_stack_word_of_its_position() {
        let mixed_plan = plan(
            vec![
                ForeignClass::Signed(8),
                ForeignClass::F32,
                ForeignClass::Unsigned(64),
                ForeignClass::F64,
                ForeignClass::Unsigned(16),
                ForeignClass::F64,
                ForeignClass::F32,
            ],
            ForeignClass::F64,
        );
        assert_eq!(
            call(
                mixed as *const () as usize,
                &mixed_plan,
                &[
                    0xff,
                    u64::from(0.5f32.to_bits()),
                    20,
                    300.0f64.to_bits(),
                    0x1_0004,
                    5000.0f64.to_bits(),
                    u64::from(0.25f32.to_bits()),
                ],
            ),
            Ok(5323.75f64.to_bits())
        );
        let integer_plan = plan(
            vec![
                ForeignClass::Unsigned(64),
                ForeignClass::Signed(32),
                ForeignClass::Unsigned(64),
                ForeignClass::Unsigned(64),
                ForeignClass::Unsigned(8),
            ],
            ForeignClass::Unsigned(64),
        );
        assert_eq!(
            call(
                integers as *const () as usize,
                &integer_plan,
                &[1, 20, 300, 4000, 0x105]
            ),
            Ok(4326)
        );
    }
}
