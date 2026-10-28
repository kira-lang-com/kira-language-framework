//! The VM's dynamic call trampoline for `call.foreign` (section 15.6). On a
//! development host it resolves the C symbol in the libraries the program
//! loaded or the process, and calls it through the platform C convention:
//! [`classed`] for System V and AAPCS64, which fill integer and
//! floating-point registers each in order, and [`positional`] for Windows
//! x64, which gives each of the first four arguments the register of its
//! position. This is the one unsafe region in the kernel: calling an
//! arbitrary foreign function cannot be expressed in safe Rust.
//!
//! Argument placement is prepared once for each import. Integer calls that
//! fit in registers use their exact arity; other calls pass all argument
//! registers and a fixed stack area through the general trampoline.

use crate::program::{ForeignClass, ForeignSignature};
mod plan;
#[cfg(any(
    all(target_arch = "x86_64", windows),
    all(test, target_arch = "x86_64")
))]
pub(crate) use plan::POSITIONAL_REGISTERS;
pub(crate) use plan::{CallPlan, Convention, Location};

#[cfg(all(unix, not(target_arch = "wasm32")))]
mod unix;
#[cfg(all(unix, not(target_arch = "wasm32")))]
pub(crate) use unix::{image_of, load, resolve};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{image_of, load, resolve};

#[cfg(any(target_arch = "aarch64", all(target_arch = "x86_64", not(windows))))]
mod classed;
#[cfg(any(target_arch = "aarch64", all(target_arch = "x86_64", not(windows))))]
pub(crate) use classed::call;

// Built on every x86_64 host under test, so the Windows convention is
// exercised where the tests run.
#[cfg(any(
    all(target_arch = "x86_64", windows),
    all(test, target_arch = "x86_64")
))]
mod positional;
#[cfg(all(target_arch = "x86_64", windows))]
pub(crate) use positional::call;

/// Where each argument of one call goes: the integer registers, the float
/// registers as bit patterns, and the stack words.
struct Layout {
    integers: [u64; 8],
    floats: [u64; 8],
    stack: [u64; 32],
}

impl Layout {
    fn of(plan: &CallPlan, arguments: &[u64]) -> Result<Self, String> {
        if plan.parameters.len() != arguments.len() || !plan.valid {
            return Err("KLF-VM-0005".to_owned());
        }
        let mut layout = Self {
            integers: [0; 8],
            floats: [0; 8],
            stack: [0; 32],
        };
        for ((class, location), word) in plan.parameters.iter().zip(arguments) {
            let value = argument_word(*class, *word);
            match *location {
                plan::Location::Integer(index) => layout.integers[index] = value,
                plan::Location::Float(index) => layout.floats[index] = value,
                plan::Location::Stack { offset, size } => {
                    let mask = u64::MAX >> ((8 - size) * 8);
                    layout.stack[offset / 8] |= (value & mask) << ((offset % 8) * 8);
                }
            }
        }
        Ok(layout)
    }
}

/// A register word as C expects the argument: a narrow integer extended to
/// the full register by its signedness, an `f32` in the low half.
fn argument_word(class: ForeignClass, word: u64) -> u64 {
    match class {
        ForeignClass::Signed(bits) if bits < 64 => {
            let shift = 64 - u32::from(bits);
            (((word << shift) as i64) >> shift) as u64
        }
        ForeignClass::Unsigned(bits) if bits < 64 => word & ((1u64 << bits) - 1),
        ForeignClass::F32 => word & 0xffff_ffff,
        _ => word,
    }
}

/// A C result as the VM holds it: an integer cut to its width, which the C
/// convention leaves undefined above, an `f32` in the low half.
fn result_word(class: Option<ForeignClass>, word: u64) -> u64 {
    match class {
        Some(ForeignClass::Signed(bits) | ForeignClass::Unsigned(bits)) if bits == 1 => {
            u64::from(word & 0xff != 0)
        }
        Some(ForeignClass::Signed(bits) | ForeignClass::Unsigned(bits)) if bits < 64 => {
            word & ((1u64 << bits) - 1)
        }
        Some(ForeignClass::F32) => word & 0xffff_ffff,
        None => 0,
        _ => word,
    }
}

/// Exact-arity calls through `extern "C"`, which is the platform's own
/// convention, for the prepared fast paths.
#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[allow(unsafe_code)]
fn call_f64(address: usize, arity: usize, arguments: &[u64]) -> f64 {
    macro_rules! invoke_f64 {
        ($($index:literal),*) => {{
            // SAFETY: the prepared signature contains only F64 parameters
            // and an F64 result, and the caller validated its exact arity.
            let function: extern "C" fn($(invoke_f64!(@type $index)),*) -> f64 =
                unsafe { std::mem::transmute(address) };
            function($(f64::from_bits(arguments[$index])),*)
        }};
        (@type $index:literal) => { f64 };
    }
    match arity {
        0 => invoke_f64!(),
        1 => invoke_f64!(0),
        2 => invoke_f64!(0, 1),
        3 => invoke_f64!(0, 1, 2),
        4 => invoke_f64!(0, 1, 2, 3),
        5 => invoke_f64!(0, 1, 2, 3, 4),
        6 => invoke_f64!(0, 1, 2, 3, 4, 5),
        7 => invoke_f64!(0, 1, 2, 3, 4, 5, 6),
        8 => invoke_f64!(0, 1, 2, 3, 4, 5, 6, 7),
        _ => unreachable!("prepared F64 call arity"),
    }
}

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
#[allow(unsafe_code)]
fn call_integer(address: usize, arity: usize, words: [u64; 8]) -> u64 {
    macro_rules! invoke_integer {
        ($($index:literal),*) => {{
            let function: extern "C" fn($(invoke_integer!(@type $index)),*) -> u64 =
                unsafe { std::mem::transmute(address) };
            function($(words[$index]),*)
        }};
        (@type $index:literal) => { u64 };
    }
    // SAFETY: the prepared signature assigns all arguments to integer registers.
    match arity {
        0 => invoke_integer!(),
        1 => invoke_integer!(0),
        2 => invoke_integer!(0, 1),
        3 => invoke_integer!(0, 1, 2),
        4 => invoke_integer!(0, 1, 2, 3),
        5 => invoke_integer!(0, 1, 2, 3, 4),
        6 => invoke_integer!(0, 1, 2, 3, 4, 5),
        7 => invoke_integer!(0, 1, 2, 3, 4, 5, 6),
        8 => invoke_integer!(0, 1, 2, 3, 4, 5, 6, 7),
        _ => unreachable!("prepared integer call arity"),
    }
}

// On `wasm32` a foreign call is a WebAssembly module import, which the
// embedder resolves; other hosts have no loader or trampoline yet.
#[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
mod no_libraries {
    pub fn resolve(_libraries: &[usize], _symbol: &str) -> Option<usize> {
        None
    }

    pub fn image_of(_address: usize) -> Option<String> {
        None
    }

    pub fn load(path: &str) -> Result<usize, String> {
        Err(format!(
            "cannot load {path}: this host loads no native libraries"
        ))
    }
}
#[cfg(not(any(windows, all(unix, not(target_arch = "wasm32")))))]
pub(crate) use no_libraries::{image_of, load, resolve};

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(crate) fn call(_address: usize, _plan: &CallPlan, _arguments: &[u64]) -> Result<u64, String> {
    Err("KLF-VM-0006".to_owned())
}

#[cfg(all(
    test,
    any(target_arch = "aarch64", all(target_arch = "x86_64", not(windows)))
))]
mod tests;
