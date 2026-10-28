//! `klf.time`, `klf.process.exit` and `klf.runtime.at_exit`: the clocks and
//! sleep, ending the process early, and the one function a program registers
//! to run as it ends, whether it returns, exits, fails or traps.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The reason a program ends, passed to its exit function. The VM uses the
/// same numbers.
pub(crate) const RETURNED: i64 = 0;
pub(crate) const TRAPPED: i64 = 1;
pub(crate) const FAILED: i64 = 2;

static EXIT_FUNCTION: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_time_monotonic() -> i64 {
    klf_host_process::time::monotonic_nanos()
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_time_wall() -> i64 {
    klf_host_process::time::wall_nanos()
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_time_sleep(nanos: i64) {
    klf_host_process::time::sleep_nanos(nanos);
}

/// Kernel service `klf.process.exit`: ends the process with `status`, as a
/// return from the entry with that status does.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_process_exit(status: i64) -> ! {
    crate::klfrt_exit(i32::try_from(status).unwrap_or(1))
}

/// Kernel service `klf.runtime.at_exit`: `function` is the address of a KIR
/// function taking the reason and its detail, or 0 for none. Answers the
/// function it replaces.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_runtime_at_exit(function: i64) -> i64 {
    let previous = EXIT_FUNCTION.swap(usize::try_from(function).unwrap_or(0), Ordering::AcqRel);
    i64::try_from(previous).unwrap_or(0)
}

/// Runs the registered exit function once. It is taken before the call, so
/// a trap inside it ends the program without running it again.
pub(crate) fn run_exit_function(reason: i64, detail: i64) {
    let address = EXIT_FUNCTION.swap(0, Ordering::AcqRel);
    if address == 0 {
        return;
    }
    crate::reset_depth();
    // SAFETY: `klfsvc_runtime_at_exit` only stores addresses a program took
    // with `func.ref` of a function lowered with the platform C convention
    // and the `(i64, i64) -> void` signature the service documents.
    let function: extern "C" fn(i64, i64) = unsafe { std::mem::transmute(address) };
    function(reason, detail);
}
