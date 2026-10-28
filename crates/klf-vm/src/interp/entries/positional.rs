//! Entries for the Windows x64 convention, which gives each of a C call's
//! first four arguments the register of its position: an integer register
//! when the argument is an integer, a floating-point one when it is a float.
//! What an entry reads from a position therefore depends on the class of the
//! argument there, so there is one shape for each choice of classes for
//! those four, sixteen in all, numbered by a bit per float position, and the
//! stack words after them.

use super::{SLOTS, STACK_WORDS, run};
use crate::foreign::POSITIONAL_REGISTERS;

/// The address of slot `slot`'s entry of shape `shape`, whose result travels
/// in a floating-point register when `float_result`.
pub(super) fn entry(slot: usize, shape: usize, float_result: bool) -> usize {
    let shapes = if float_result {
        &FLOAT_SHAPES
    } else {
        &INTEGER_SHAPES
    };
    shapes[shape](slot)
}

/// Runs slot `slot`'s function with the words C passed, the register of
/// each position serving whichever class the plan places there.
fn enter(slot: usize, registers: [u64; POSITIONAL_REGISTERS], stack: [u64; STACK_WORDS]) -> u64 {
    run(slot, &registers, &registers, &stack)
}

/// A register's contents as a word.
trait Word {
    fn word(self) -> u64;
}

impl Word for u64 {
    fn word(self) -> u64 {
        self
    }
}

impl Word for f64 {
    fn word(self) -> u64 {
        self.to_bits()
    }
}

/// One shape: the entries of every slot whose first four arguments have the
/// classes `$r0` to `$r3`, returning an integer or a float.
macro_rules! shape {
    ($shape:ident: $r0:ty, $r1:ty, $r2:ty, $r3:ty) => {
        mod $shape {
            use super::{SLOTS, Word, enter};

            type Integer = extern "win64" fn(
                $r0,
                $r1,
                $r2,
                $r3,
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
            ) -> u64;
            type Float = extern "win64" fn(
                $r0,
                $r1,
                $r2,
                $r3,
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
            ) -> f64;

            #[allow(clippy::too_many_arguments)]
            extern "win64" fn integer<const SLOT: usize>(
                r0: $r0,
                r1: $r1,
                r2: $r2,
                r3: $r3,
                s0: u64,
                s1: u64,
                s2: u64,
                s3: u64,
                s4: u64,
                s5: u64,
                s6: u64,
                s7: u64,
                s8: u64,
                s9: u64,
                s10: u64,
                s11: u64,
            ) -> u64 {
                enter(
                    SLOT,
                    [r0.word(), r1.word(), r2.word(), r3.word()],
                    [s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11],
                )
            }

            /// A `float` result is the low 32 bits of the register.
            #[allow(clippy::too_many_arguments)]
            extern "win64" fn float<const SLOT: usize>(
                r0: $r0,
                r1: $r1,
                r2: $r2,
                r3: $r3,
                s0: u64,
                s1: u64,
                s2: u64,
                s3: u64,
                s4: u64,
                s5: u64,
                s6: u64,
                s7: u64,
                s8: u64,
                s9: u64,
                s10: u64,
                s11: u64,
            ) -> f64 {
                f64::from_bits(enter(
                    SLOT,
                    [r0.word(), r1.word(), r2.word(), r3.word()],
                    [s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11],
                ))
            }

            static INTEGERS: [Integer; SLOTS] = every_slot!(integer);
            static FLOATS: [Float; SLOTS] = every_slot!(float);

            pub(super) fn integer_entry(slot: usize) -> usize {
                INTEGERS[slot] as usize
            }

            pub(super) fn float_entry(slot: usize) -> usize {
                FLOATS[slot] as usize
            }
        }
    };
}

// Bit `n` of a shape's number set means position `n` holds a float.
shape!(s0: u64, u64, u64, u64);
shape!(s1: f64, u64, u64, u64);
shape!(s2: u64, f64, u64, u64);
shape!(s3: f64, f64, u64, u64);
shape!(s4: u64, u64, f64, u64);
shape!(s5: f64, u64, f64, u64);
shape!(s6: u64, f64, f64, u64);
shape!(s7: f64, f64, f64, u64);
shape!(s8: u64, u64, u64, f64);
shape!(s9: f64, u64, u64, f64);
shape!(s10: u64, f64, u64, f64);
shape!(s11: f64, f64, u64, f64);
shape!(s12: u64, u64, f64, f64);
shape!(s13: f64, u64, f64, f64);
shape!(s14: u64, f64, f64, f64);
shape!(s15: f64, f64, f64, f64);

static INTEGER_SHAPES: [fn(usize) -> usize; 16] = [
    s0::integer_entry,
    s1::integer_entry,
    s2::integer_entry,
    s3::integer_entry,
    s4::integer_entry,
    s5::integer_entry,
    s6::integer_entry,
    s7::integer_entry,
    s8::integer_entry,
    s9::integer_entry,
    s10::integer_entry,
    s11::integer_entry,
    s12::integer_entry,
    s13::integer_entry,
    s14::integer_entry,
    s15::integer_entry,
];
static FLOAT_SHAPES: [fn(usize) -> usize; 16] = [
    s0::float_entry,
    s1::float_entry,
    s2::float_entry,
    s3::float_entry,
    s4::float_entry,
    s5::float_entry,
    s6::float_entry,
    s7::float_entry,
    s8::float_entry,
    s9::float_entry,
    s10::float_entry,
    s11::float_entry,
    s12::float_entry,
    s13::float_entry,
    s14::float_entry,
    s15::float_entry,
];

#[cfg(test)]
mod tests {
    use super::super::bind_as;
    use crate::foreign::Convention;
    use crate::interp::Instance;
    use crate::program::{ForeignClass, ForeignSignature};
    use crate::{Capabilities, lower_module};

    /// `@mix` adds six arguments, alternating integers and floats so that
    /// two of each travel on the stack; `@second` returns its float.
    const SOURCE: &str = r#"module @Positional lang "test" target "x86_64-pc-windows-msvc" stage raw

func module @mix(%a: i64, %b: f64, %c: i32, %d: f64, %e: i64, %f: f64) -> i64 engine(runtime) {
bb0(%a: i64, %b: f64, %c: i32, %d: f64, %e: i64, %f: f64):
  %bi = fptosi %b to i64
  %ci = sext %c to i64
  %di = fptosi %d to i64
  %fi = fptosi %f to i64
  %ab = add %a, %bi
  %abc = add %ab, %ci
  %abcd = add %abc, %di
  %abcde = add %abcd, %e
  %all = add %abcde, %fi
  return %all
}

func module @second(%a: i64, %b: f64) -> f64 engine(runtime) {
bb0(%a: i64, %b: f64):
  return %b
}
"#;

    fn instance() -> Box<Instance> {
        let module = klf_kir::parse_module(SOURCE).expect("module parses");
        let program = lower_module(&module).expect("module lowers");
        Instance::load(program, &std::env::temp_dir(), Capabilities::new()).expect("instance loads")
    }

    fn bound(instance: &Instance, name: &str, signature: &ForeignSignature) -> usize {
        let function = instance
            .function_index(name)
            .and_then(|index| u32::try_from(index).ok())
            .expect("function exists");
        bind_as(Convention::Positional, instance, name, function, signature).expect("entry binds")
    }

    #[test]
    #[allow(unsafe_code)]
    fn a_windows_entry_reads_each_argument_from_its_position() {
        let instance = instance();
        let mix = ForeignSignature {
            parameters: vec![
                ForeignClass::Signed(64),
                ForeignClass::F64,
                ForeignClass::Signed(32),
                ForeignClass::F64,
                ForeignClass::Signed(64),
                ForeignClass::F64,
            ],
            result: Some(ForeignClass::Signed(64)),
        };
        // SAFETY: the entry was bound to `@mix` with this C signature.
        let entry: extern "win64" fn(i64, f64, i32, f64, i64, f64) -> i64 =
            unsafe { std::mem::transmute(bound(&instance, "mix", &mix)) };
        assert_eq!(entry(1, 20.0, -300, 4000.0, 50_000, 600_000.0), 653_721);

        let second = ForeignSignature {
            parameters: vec![ForeignClass::Signed(64), ForeignClass::F64],
            result: Some(ForeignClass::F64),
        };
        // SAFETY: the entry was bound to `@second` with this C signature.
        let entry: extern "win64" fn(i64, f64) -> f64 =
            unsafe { std::mem::transmute(bound(&instance, "second", &second)) };
        assert_eq!(entry(7, 2.5).to_bits(), 2.5f64.to_bits());
    }

    #[test]
    fn a_shape_has_a_bit_for_each_float_in_a_register() {
        let signature = ForeignSignature {
            parameters: vec![
                ForeignClass::Unsigned(8),
                ForeignClass::F32,
                ForeignClass::Signed(64),
                ForeignClass::F64,
                ForeignClass::F64,
            ],
            result: None,
        };
        let plan = crate::foreign::CallPlan::with(&signature, Convention::Positional);
        assert_eq!(plan.positional_shape(), 0b1010);
    }
}
