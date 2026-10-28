//! Entries for the System V and AAPCS64 conventions, which place a C call's
//! integer and floating-point arguments in separate registers, each class in
//! order, and the rest on the stack. One shape, every integer and
//! floating-point argument register and the stack words after them,
//! receives any signature, so there is an integer-result and a float-result
//! entry per slot.

use super::{SLOTS, STACK_WORDS, run};

/// The integer argument registers: six on `x86_64`, eight on AAPCS64.
#[cfg(target_arch = "x86_64")]
macro_rules! integer_registers {
    ($callback:ident) => {
        $callback! { i0 i1 i2 i3 i4 i5 }
    };
}
#[cfg(target_arch = "aarch64")]
macro_rules! integer_registers {
    ($callback:ident) => {
        $callback! { i0 i1 i2 i3 i4 i5 i6 i7 }
    };
}

/// The address of slot `slot`'s entry, whose result travels in a
/// floating-point register when `float_result`.
pub(super) fn entry(slot: usize, float_result: bool) -> usize {
    if float_result {
        FLOAT_ENTRIES[slot] as usize
    } else {
        INTEGER_ENTRIES[slot] as usize
    }
}

macro_rules! entries {
    ($($integer:ident)*) => {
        type IntegerEntry = extern "C" fn(
            $(entries!(@word $integer),)*
            f64, f64, f64, f64, f64, f64, f64, f64,
            u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64,
        ) -> u64;
        type FloatEntry = extern "C" fn(
            $(entries!(@word $integer),)*
            f64, f64, f64, f64, f64, f64, f64, f64,
            u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64,
        ) -> f64;

        #[allow(clippy::too_many_arguments)]
        extern "C" fn integer_entry<const SLOT: usize>(
            $($integer: u64,)*
            f0: f64, f1: f64, f2: f64, f3: f64, f4: f64, f5: f64, f6: f64, f7: f64,
            s0: u64, s1: u64, s2: u64, s3: u64, s4: u64, s5: u64,
            s6: u64, s7: u64, s8: u64, s9: u64, s10: u64, s11: u64,
        ) -> u64 {
            run(
                SLOT,
                &[$($integer),*],
                &[f0, f1, f2, f3, f4, f5, f6, f7].map(f64::to_bits),
                &[s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11],
            )
        }

        /// A `float` result is the low 32 bits of the register.
        #[allow(clippy::too_many_arguments)]
        extern "C" fn float_entry<const SLOT: usize>(
            $($integer: u64,)*
            f0: f64, f1: f64, f2: f64, f3: f64, f4: f64, f5: f64, f6: f64, f7: f64,
            s0: u64, s1: u64, s2: u64, s3: u64, s4: u64, s5: u64,
            s6: u64, s7: u64, s8: u64, s9: u64, s10: u64, s11: u64,
        ) -> f64 {
            f64::from_bits(run(
                SLOT,
                &[$($integer),*],
                &[f0, f1, f2, f3, f4, f5, f6, f7].map(f64::to_bits),
                &[s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11],
            ))
        }
    };
    (@word $integer:ident) => { u64 };
}

integer_registers!(entries);

const _: () = assert!(STACK_WORDS == 12, "each entry names twelve stack words");

static INTEGER_ENTRIES: [IntegerEntry; SLOTS] = every_slot!(integer_entry);
static FLOAT_ENTRIES: [FloatEntry; SLOTS] = every_slot!(float_entry);

#[cfg(test)]
mod tests {
    use super::super::bind;
    use crate::interp::Instance;
    use crate::program::{ForeignClass, ForeignSignature};
    use crate::{Capabilities, lower_module};

    /// `@spread` takes ten integers and two floats, more than the registers
    /// hold, so the last ones reach it on the stack.
    const SOURCE: &str = r#"module @Classed lang "test" target "x86_64-unknown-linux-gnu" stage raw

func module @spread(%a: i64, %b: i64, %c: i64, %d: i64, %e: i64, %f: i64, %g: f64, %h: i64, %i: i64, %j: i32, %k: i64, %l: f64) -> i64 engine(runtime) {
bb0(%a: i64, %b: i64, %c: i64, %d: i64, %e: i64, %f: i64, %g: f64, %h: i64, %i: i64, %j: i32, %k: i64, %l: f64):
  %gi = fptosi %g to i64
  %ji = sext %j to i64
  %li = fptosi %l to i64
  %s1 = add %a, %b
  %s2 = add %s1, %c
  %s3 = add %s2, %d
  %s4 = add %s3, %e
  %s5 = add %s4, %f
  %s6 = add %s5, %gi
  %s7 = add %s6, %h
  %s8 = add %s7, %i
  %s9 = add %s8, %ji
  %s10 = add %s9, %k
  %all = add %s10, %li
  return %all
}
"#;

    #[test]
    #[allow(unsafe_code)]
    fn an_entry_reads_the_arguments_past_the_registers_from_the_stack() {
        let module = klf_kir::parse_module(SOURCE).expect("module parses");
        let program = lower_module(&module).expect("module lowers");
        let instance = Instance::load(program, &std::env::temp_dir(), Capabilities::new())
            .expect("instance loads");
        let mut parameters = vec![ForeignClass::Signed(64); 6];
        parameters.extend([
            ForeignClass::F64,
            ForeignClass::Signed(64),
            ForeignClass::Signed(64),
            ForeignClass::Signed(32),
            ForeignClass::Signed(64),
            ForeignClass::F64,
        ]);
        let signature = ForeignSignature {
            parameters,
            result: Some(ForeignClass::Signed(64)),
        };
        let function = instance
            .function_index("spread")
            .and_then(|index| u32::try_from(index).ok())
            .expect("function exists");
        let address = bind(&instance, "spread", function, &signature).expect("entry binds");
        // SAFETY: the entry was bound to `@spread` with this C signature.
        let entry: extern "C" fn(
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            f64,
            i64,
            i64,
            i32,
            i64,
            f64,
        ) -> i64 = unsafe { std::mem::transmute(address) };
        assert_eq!(
            entry(1, 2, 3, 4, 5, 6, 70.0, 800, 9000, -10, 100_000, 2_000_000.0),
            2_109_881
        );
    }
}
