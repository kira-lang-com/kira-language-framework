//! Native runtime shim: the section-18 `klf-rt` heap and counting exposed as
//! `extern "C"` `klfrt_*` symbols that code emitted by `klf-llvm` links against.
//!
//! One thread-local [`Heap`] backs every allocation, a per-frame list mirrors
//! one entry function runs to a terminator and the shim prints the outcome in
// Android's thread locals are emulated, and the lint misreads the
// `thread_local!` expansion there as a non-`const` initializer.
#![cfg_attr(target_os = "android", allow(clippy::missing_const_for_thread_local))]

mod access;
mod arguments;
mod backtrace;
mod crypto;
mod ending;
mod environment;
mod fault;
mod files;
mod host;
mod image;
pub mod interrupt;
mod names;
mod net;
mod plugins;
mod process;
mod process_host;
mod ridl;
pub use ridl::klfrt_ridl_remote_scalar;
mod service_abi;
pub mod services;
mod stack;
mod strict;
mod thread;
mod trap;

use klf_rt::Heap;
use std::cell::RefCell;
use std::mem::ManuallyDrop;
pub use trap::{
    klfrt_trap, klfrt_trap_message, klfsvc_trap_operands, klfsvc_trap_text, trap_report,
};

/// A backstop on the call depth; the recursion limit proper is the end of
/// the thread's stack (`stack::exhausted`).
const MAX_DEPTH: usize = 1 << 20;

/// A function's frame: where the heap's frame arena stood at entry, and the
/// stack allocations too large for the arena, freed at exit.
struct Frame {
    mark: (usize, usize),
    large: Vec<*mut u8>,
}

struct Runtime {
    /// Every thread's heap shares the process page map, so a block one
    /// thread allocates can be read, retained and freed on any other.
    heap: ManuallyDrop<Heap>,
    frames: Vec<Frame>,
}

impl Runtime {
    fn new() -> Self {
        Self {
            heap: ManuallyDrop::new(Heap::shared()),
            frames: Vec::new(),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // The thread is ending, but what it allocated may live on in other
        // threads: the heap is orphaned, not freed.
        // SAFETY: the heap is taken once, here, and not touched after.
        unsafe { ManuallyDrop::take(&mut self.heap) }.orphan();
    }
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::new());
}

fn with<R>(action: impl FnOnce(&mut Runtime) -> R) -> R {
    RUNTIME.with(|cell| action(&mut cell.borrow_mut()))
}

/// The number of tracked heap blocks that are still live.
///
/// The section 15.6 check reads this: on the VM a unit gets its own machine
/// and the count is compared against zero, while natively every unit shares
/// the process heap and the count is compared against what was live when the
/// unit started.
#[must_use]
pub fn live_blocks() -> usize {
    with(|rt| {
        rt.heap.collect_remote();
        rt.heap.live_blocks()
    })
}

/// Set once by a program built with the process entry convention: it ends as
/// a C process does instead of printing an outcome line.
static PROCESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set with [`PROCESS`] when a trap ends the process with exit status 1
/// rather than aborting it.
static TRAP_EXITS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn is_process() -> bool {
    process_host::klfrt_process_mode() & 1 != 0
}

/// Reports the executable runtime's process entry state to attached libraries.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_process_active() -> i32 {
    i32::from(PROCESS.load(std::sync::atomic::Ordering::Relaxed))
        | (i32::from(TRAP_EXITS.load(std::sync::atomic::Ordering::Relaxed)) << 1)
}

/// Called first by a process-convention `main`; `trap_exits` is nonzero when
/// a trap exits with status 1 instead of aborting.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_process_start(trap_exits: i32) {
    PROCESS.store(true, std::sync::atomic::Ordering::Relaxed);
    TRAP_EXITS.store(trap_exits != 0, std::sync::atomic::Ordering::Relaxed);
    fault::install();
}

/// The end of a process-convention program: `status` is its exit status.
/// `exit` runs the C library's `atexit` handlers. With `KLF_HEAP_REPORT` set,
/// what the program left allocated is written to stderr first.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_exit(status: i32) -> ! {
    ending::run_exit_function(ending::RETURNED, i64::from(status));
    if std::env::var_os("KLF_HEAP_REPORT").is_some_and(|value| !value.is_empty()) {
        let report = with(|rt| rt.heap.exit_report());
        eprintln!(
            "heap at exit: {} tracked ({} bytes), {} untracked ({} bytes)",
            report.tracked, report.tracked_bytes, report.untracked, report.untracked_bytes
        );
    }
    std::process::exit(status)
}

/// Kernel service `klf.io.put` (section 8.12): write one byte and return it.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_io_put(byte: i32) -> i32 {
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let value = byte as u8;
    if is_process() {
        let mut output = std::io::stdout().lock();
        let _ = std::io::Write::write_all(&mut output, &[value])
            .and_then(|()| std::io::Write::flush(&mut output));
    } else {
        println!("IO {value}");
    }
    i32::from(value)
}

/// Kernel service `klf.io.puti`: write a decimal integer and return void.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_io_puti(value: i64) {
    if is_process() {
        println!("{value}");
    } else {
        println!("IO {value}");
    }
}

/// Kernel service `klf.io.write`: writes `length` bytes to standard output
/// and answers how many, or -1 when they cannot be written.
///
/// # Safety
///
/// `data` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_io_write(data: *const u8, length: i64) -> i64 {
    let Ok(length) = usize::try_from(length) else {
        return -1;
    };
    if length > klf_rt::MAX_SERVICE_BUFFER_BYTES || (length != 0 && data.is_null()) {
        return -1;
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    let mut output = std::io::stdout().lock();
    if std::io::Write::write_all(&mut output, bytes)
        .and_then(|()| std::io::Write::flush(&mut output))
        .is_err()
    {
        return -1;
    }
    i64::try_from(length).unwrap_or(i64::MAX)
}

/// Kernel service `klf.io.write_error`: [`klfsvc_io_write`] to standard
/// error.
///
/// # Safety
///
/// `data` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_io_write_error(data: *const u8, length: i64) -> i64 {
    let Ok(length) = usize::try_from(length) else {
        return -1;
    };
    if length > klf_rt::MAX_SERVICE_BUFFER_BYTES || (length != 0 && data.is_null()) {
        return -1;
    }
    let bytes = if length == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(data, length) }
    };
    let mut output = std::io::stderr().lock();
    if std::io::Write::write_all(&mut output, bytes)
        .and_then(|()| std::io::Write::flush(&mut output))
        .is_err()
    {
        return -1;
    }
    i64::try_from(length).unwrap_or(i64::MAX)
}

/// Kernel service `klf.text.float`: the byte length of the shortest text that
/// reads back as the `f64` with `bits`, copied to `destination` when it fits
/// in `capacity` bytes.
///
/// # Safety
///
/// `destination` must be writable for `capacity` bytes when the text fits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_text_float(bits: i64, destination: *mut u8, capacity: i64) -> i64 {
    let value = f64::from_bits(u64::from_ne_bytes(bits.to_ne_bytes()));
    let text = value.to_string();
    let bytes = text.as_bytes();
    let length = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let fits = usize::try_from(capacity).is_ok_and(|capacity| bytes.len() <= capacity);
    if fits && !bytes.is_empty() {
        if destination.is_null() {
            return -1;
        }
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len()) };
    }
    length
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_math_unary(operation: i64, bits: i64) -> i64 {
    let value = f64::from_bits(u64::from_ne_bytes(bits.to_ne_bytes()));
    let result = u64::try_from(operation)
        .ok()
        .and_then(|operation| klf_rt::math::unary(operation, value))
        .unwrap_or(f64::NAN);
    i64::from_ne_bytes(result.to_bits().to_ne_bytes())
}

/// Kernel number service: one operation of Kira's exact decimal `Number`,
/// answering the first word of the result.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_number(
    operation: i64,
    left: i64,
    left_scale: i64,
    right: i64,
    right_scale: i64,
) -> i64 {
    let word = |value: i64| u64::from_ne_bytes(value.to_ne_bytes());
    let answer = klf_rt::decimal::service(
        word(operation),
        [word(left), word(left_scale)],
        [word(right), word(right_scale)],
    );
    i64::from_ne_bytes(answer.to_ne_bytes())
}

/// The second word of this thread's last `klfsvc_number` answer: a scale, or
/// 32 plus an error code.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_number_second() -> i64 {
    i64::from_ne_bytes(klf_rt::decimal::service_second().to_ne_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_math_binary(operation: i64, left: i64, right: i64) -> i64 {
    let left = f64::from_bits(u64::from_ne_bytes(left.to_ne_bytes()));
    let right = f64::from_bits(u64::from_ne_bytes(right.to_ne_bytes()));
    let result = u64::try_from(operation)
        .ok()
        .and_then(|operation| klf_rt::math::binary(operation, left, right))
        .unwrap_or(f64::NAN);
    i64::from_ne_bytes(result.to_bits().to_ne_bytes())
}

/// Kernel source service: return the byte length of the current input buffer.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_source_len() -> i64 {
    services::with_source(|source| source.len().try_into().unwrap_or(i64::MAX))
}

/// Kernel source service: read one byte from the current input buffer. Out of
/// range indices return zero so a definition can probe its end marker safely.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_source_byte(index: i64) -> i64 {
    let Ok(index) = usize::try_from(index) else {
        return 0;
    };
    services::with_source(|source| source.get(index).copied().map_or(0, i64::from))
}

/// Kernel source service: copy the whole current input buffer to `address`,
/// which must hold exactly `length` bytes. One on success.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_source_copy(address: i64, length: i64) -> i64 {
    services::with_source(|source| {
        if usize::try_from(length).ok() != Some(source.len()) {
            return 0;
        }
        if !source.is_empty() {
            if address == 0 {
                return 0;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(source.as_ptr(), address as *mut u8, source.len())
            };
        }
        1
    })
}

/// Kernel diagnostic service: report a numeric code and source byte range in
/// the file the phase is running on.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_diag_error(code: i64, start: i64, end: i64) {
    report(klf_diag::RuntimeDiagnostic::new(code, start, end));
}

/// The same, for a range in a named file: a whole-program check runs on one
/// unit and points at another.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_diag_error_in(file: i64, code: i64, start: i64, end: i64) {
    report(klf_diag::RuntimeDiagnostic {
        file,
        ..klf_diag::RuntimeDiagnostic::new(code, start, end)
    });
}

/// The same, for a range in the registered source whose content hash is
/// `origin`: a unit that splices in an included header points into it.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_diag_error_origin(origin: i64, code: i64, start: i64, end: i64) {
    report(klf_diag::RuntimeDiagnostic {
        origin,
        ..klf_diag::RuntimeDiagnostic::new(code, start, end)
    });
}

/// The same, with the name the report is about: the headline quotes it.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_diag_error_subject(
    file: i64,
    code: i64,
    start: i64,
    end: i64,
    subject_start: i64,
    subject_end: i64,
) {
    report(klf_diag::RuntimeDiagnostic {
        file,
        subject: Some((subject_start, subject_end)),
        ..klf_diag::RuntimeDiagnostic::new(code, start, end)
    });
}

/// Kernel diagnostic service: attach `length` bytes of UTF-8 text to the
/// diagnostic reported last, as a note, a help, its label or, with `kind` 3,
/// a further place in `file` from `start` to `end`.
///
/// # Safety
///
/// `text` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_diag_detail(
    kind: i64,
    file: i64,
    start: i64,
    end: i64,
    text: *const u8,
    length: i64,
) {
    let Some(kind) = klf_diag::DetailKind::from_code(kind) else {
        return;
    };
    let length = usize::try_from(length).unwrap_or(0);
    let bytes = if length == 0 || text.is_null() {
        &[][..]
    } else {
        // SAFETY: the caller passes a buffer of `length` bytes.
        unsafe { std::slice::from_raw_parts(text, length) }
    };
    let detail = klf_diag::RuntimeDetail {
        kind,
        file,
        start,
        end,
        text: String::from_utf8_lossy(bytes).into_owned(),
    };
    let line = detail.protocol_line();
    if !services::record_detail(detail) {
        println!("{line}");
    }
}

mod ffi;

pub(crate) use ffi::report;
pub use ffi::*;
pub(crate) use ffi::{depth, frame_count, reset_depth, unwind_frames};
