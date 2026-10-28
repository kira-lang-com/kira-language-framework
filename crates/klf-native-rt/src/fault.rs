//! Memory and instruction faults in a process-convention program.
//!
//! Compiled Kira never reaches memory it does not own, so a fault is a bug in
//! the compiler or the runtime, or in C code the program calls. The fault is
//! reported as a trap is, with the address and the frames that led to it,
//! and then the signal takes its default course, so the process still ends
//! as a crashed one and a debugger or crash reporter sees the same fault.
//!
//! The handler runs on a stack of its own, so a fault from a stack that ran
//! out can still be reported. Writing the report allocates; a second fault
//! while it is written ends the process at once (`SA_RESETHAND`).

use klf_diag::report::Report;

/// Bytes of the stack the handler runs on.
const HANDLER_STACK: usize = 256 * 1024;

#[cfg(unix)]
pub(crate) fn install() {
    // SAFETY: the alternate stack is allocated once and never freed, and the
    // handler is an `extern "C"` function with the `SA_SIGINFO` signature.
    unsafe {
        let stack = std::alloc::alloc(std::alloc::Layout::from_size_align_unchecked(HANDLER_STACK, 16));
        if stack.is_null() {
            return;
        }
        let alternate = libc::stack_t {
            ss_sp: stack.cast(),
            ss_flags: 0,
            ss_size: HANDLER_STACK,
        };
        libc::sigaltstack(&raw const alternate, std::ptr::null_mut());
        for signal in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE] {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const () as usize;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK | libc::SA_RESETHAND;
            libc::sigemptyset(&raw mut action.sa_mask);
            libc::sigaction(signal, &raw const action, std::ptr::null_mut());
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn install() {}

#[cfg(unix)]
extern "C" fn handler(signal: libc::c_int, info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    // SAFETY: the kernel passes a valid `siginfo_t` to an `SA_SIGINFO` handler.
    let address = unsafe { (*info).si_addr() } as usize;
    let (name, what) = match signal {
        libc::SIGSEGV => ("SIGSEGV", "invalid memory access"),
        libc::SIGBUS => ("SIGBUS", "misaligned or unmapped memory access"),
        libc::SIGILL => ("SIGILL", "illegal instruction"),
        _ => ("SIGFPE", "arithmetic fault"),
    };
    let mut report = Report {
        code: name.to_owned(),
        headline: format!("the program crashed: {what} at {address:#x}"),
        place: crate::backtrace::innermost_place(),
        notes: vec![
            "compiled Kira only reaches memory it owns, so this is a bug in the compiler, the \
             runtime, or C code the program calls"
                .to_owned(),
        ],
        helps: vec![
            "report it with this backtrace; `KLF_HEAP_STRICT=1` names the frees that came too \
             early, and `KLF_BACKTRACE=full` lists the runtime's own frames"
                .to_owned(),
        ],
    };
    if address < 4096 {
        report.notes.push("the address is in the first page: a null pointer was read through".to_owned());
    }
    eprint!("{}", report.render());
    crate::backtrace::report();
}
