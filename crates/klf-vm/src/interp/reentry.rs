//! C calling back into bytecode (section 26.4 on the VM). A foreign call
//! publishes the machine it runs on for its thread; the native entry of a
//! bytecode function C calls, generated beside the program, passes its
//! arguments as words to [`klfvm_invoke`] with the instance its library is
//! bound to, which runs the function there, or, for a program no instance
//! runs, on the machine the foreign call published.
#![allow(unsafe_code)]

use super::{Capabilities, Instance, Machine, Outcome, ResolvedCapabilities, execute};
use crate::program::Program;
use std::cell::Cell;

type Active = Option<(
    *mut Machine<'static>,
    *const Capabilities,
    *const ResolvedCapabilities<'static>,
)>;

thread_local! {
    static ACTIVE: Cell<Active> = const { Cell::new(None) };
}

/// Runs `native` with `machine` published for the callbacks it makes.
pub(super) fn call<T>(
    machine: &mut Machine,
    capabilities: &Capabilities,
    resolved_capabilities: &ResolvedCapabilities<'_>,
    native: impl FnOnce() -> T,
) -> T {
    let published = (
        std::ptr::from_mut(machine).cast::<Machine<'static>>(),
        std::ptr::from_ref(capabilities),
        std::ptr::from_ref(resolved_capabilities).cast::<ResolvedCapabilities<'static>>(),
    );
    let previous = ACTIVE.replace(Some(published));
    let value = native();
    ACTIVE.set(previous);
    value
}

/// Runs bytecode function `function` (section 17.3) with its arguments as
/// words from `arguments`, and writes its result word to `results`. The
/// function runs on `instance`, or, for a program no instance runs, on the
/// machine whose foreign call C is inside. A run that cannot return to C, a
/// trap or an uncaught failure, ends the process as a trap in the program
/// would.
///
/// # Safety
/// `instance` must be null or a live [`Instance`], `arguments` must point at
/// one readable word per parameter of the function, and `results` at one
/// writable word.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfvm_invoke(
    instance: *const Instance,
    function: u32,
    arguments: *const u64,
    results: *mut u64,
) {
    let function = function as usize;
    // SAFETY: the caller promises a live instance or null.
    let outcome = if let Some(instance) = unsafe { instance.as_ref() } {
        // SAFETY: the entry passes one word per parameter.
        let arguments = unsafe { words(instance.parameter_count(function), arguments) };
        instance.call(function, arguments)
    } else {
        let Some((machine, capabilities, resolved_capabilities)) = ACTIVE.get() else {
            eprint!("{}", klf_diag::report::trap_description("KLF-VM-0007").render());
            std::process::exit(1);
        };
        // SAFETY: the machine and capabilities were published by a foreign
        // call still running on this thread, which does not touch them until
        // C returns to it.
        unsafe {
            let machine = &mut *machine;
            let arguments = words(parameter_count(machine.program(), function), arguments);
            execute::<false>(
                machine,
                function,
                arguments,
                &*capabilities,
                &*resolved_capabilities,
                None,
            )
        }
    };
    let value = match outcome {
        Outcome::Return(value) => value.unwrap_or(0),
        Outcome::Fail(value) => {
            eprintln!("uncaught failure {value}");
            std::process::exit(1);
        }
        Outcome::Trap(code) => {
            if klf_diag::report::report_code(&code).is_some() {
                eprint!("{code}");
            } else {
                eprint!("{}", klf_diag::report::trap_description(&code).render());
            }
            std::process::exit(1);
        }
    };
    if !results.is_null() {
        // SAFETY: the caller promises one writable word.
        unsafe { results.write(value) };
    }
}

/// How many parameters `function` of `program` takes. The program is the
/// version the call enters, which a push may have given other indices.
pub(super) fn parameter_count(program: &Program, function: usize) -> usize {
    program
        .functions
        .get(function)
        .map_or(0, |function| function.param_count as usize)
}

/// The `count` argument words at `arguments`.
unsafe fn words<'a>(count: usize, arguments: *const u64) -> &'a [u64] {
    if count == 0 || arguments.is_null() {
        return &[];
    }
    // SAFETY: the caller promises one word per parameter.
    unsafe { std::slice::from_raw_parts(arguments, count) }
}
