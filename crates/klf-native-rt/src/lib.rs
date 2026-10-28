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
mod strict;
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
mod process;
mod ridl;
mod service_abi;
pub mod services;
mod stack;
mod thread;
mod trap;

use klf_rt::Heap;
pub use trap::{klfrt_trap, klfrt_trap_message, klfsvc_trap_operands, klfsvc_trap_text, trap_report};
use std::cell::RefCell;
use std::mem::ManuallyDrop;

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
    PROCESS.load(std::sync::atomic::Ordering::Relaxed)
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

/// Kernel service `klf.io.put` (section 8.12): write one byte to stdout, prefixed
/// with `IO ` so the native outcome parser can tell service output from the
/// final `RET`/`TRAP` line. Returns the byte.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_io_put(byte: i32) -> i32 {
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let value = byte as u8;
    println!("IO {value}");
    byte
}

/// Kernel service `klf.io.puti`: write a decimal integer and return void.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_io_puti(value: i64) {
    println!("IO {value}");
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
pub unsafe extern "C" fn klfsvc_diag_detail(kind: i64, file: i64, start: i64, end: i64, text: *const u8, length: i64) {
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

fn report(record: klf_diag::RuntimeDiagnostic) {
    if !services::record_diagnostic(record.clone()) {
        match record.subject {
            Some((subject_start, subject_end)) => println!(
                "DIAGS {} {} {} {} {} {} {}",
                record.code, record.start, record.end, record.file, record.origin, subject_start, subject_end
            ),
            None => println!(
                "DIAG {} {} {} {} {}",
                record.code, record.start, record.end, record.file, record.origin
            ),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_frame_enter() {
    access::enter();
    klfrt_depth_enter();
    with(|rt| {
        let mark = rt.heap.arena_mark();
        rt.frames.push(Frame {
            mark,
            large: Vec::new(),
        });
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_frame_exit() {
    let freed = with(|rt| {
        let mut freed = true;
        if let Some(frame) = rt.frames.pop() {
            for slot in frame.large {
                freed &= rt.heap.free_raw(slot);
            }
            rt.heap.arena_release(frame.mark);
        }
        freed
    });
    trap_unless_freed(freed, None);
    access::leave();
    klfrt_depth_exit();
}

thread_local! {
    /// The call depth, for the recursion limit (`KLF-RT-0021`).
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Entry to a function whose locals are all native stack slots: only the
/// depth is counted.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_depth_enter() {
    let depth = DEPTH.with(|depth| {
        let next = depth.get() + 1;
        depth.set(next);
        next
    });
    if depth > MAX_DEPTH || stack::exhausted() {
        trap::trap_with(21, vec![format!("the call depth reached {depth} frames")]);
    }
}

/// The call depth, which an interrupt puts back with [`unwind_frames`].
pub(crate) fn depth() -> usize {
    DEPTH.with(std::cell::Cell::get)
}

/// How many functions with frames are running.
pub(crate) fn frame_count() -> usize {
    with(|rt| rt.frames.len())
}

/// Ends the frames of the functions an interrupt unwound, as their
/// `klfrt_frame_exit` would have, and puts the depth back to `depth`.
pub(crate) fn unwind_frames(count: usize, depth: usize) {
    let freed = with(|rt| {
        let mut freed = true;
        while rt.frames.len() > count {
            if let Some(frame) = rt.frames.pop() {
                for slot in frame.large {
                    freed &= rt.heap.free_raw(slot);
                }
                rt.heap.arena_release(frame.mark);
            }
        }
        freed
    });
    trap_unless_freed(freed, None);
    DEPTH.with(|current| current.set(depth));
}

/// Lets the exit function run after a trap at the recursion limit.
pub(crate) fn reset_depth() {
    DEPTH.with(|depth| depth.set(0));
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_depth_exit() {
    DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_stack_alloc(size: u64, align: u64) -> *mut u8 {
    with(|rt| {
        if !rt.frames.is_empty()
            && let Some(pointer) = rt.heap.arena_alloc(size as usize, align.max(1) as usize)
        {
            return pointer;
        }
        let pointer = rt.heap.alloc_raw(size as usize, (align.max(1)) as usize);
        if let Some(frame) = rt.frames.last_mut() {
            frame.large.push(pointer);
        }
        pointer
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_stack_dealloc(pointer: *mut u8) {
    // An arena allocation goes with its frame; a large one now.
    let freed = with(|rt| {
        if let Some(frame) = rt.frames.last_mut()
            && let Some(position) = frame.large.iter().position(|slot| *slot == pointer)
        {
            frame.large.swap_remove(position);
            return rt.heap.free_raw(pointer);
        }
        true
    });
    trap_unless_freed(freed, Some(pointer));
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_heap_alloc_counted(size: u64, align: u64) -> *mut u8 {
    with(|rt| {
        rt.heap
            .alloc_counted(size as usize, (align.max(1)) as usize, None)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_heap_alloc_unique(size: u64, align: u64) -> *mut u8 {
    with(|rt| rt.heap.alloc_unique(size as usize, (align.max(1)) as usize))
}

/// Untracked allocation for growable-array and string buffers: it does not
/// count toward the heap-balance check and is reclaimed when the process exits.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_heap_alloc_untracked(size: u64, align: u64) -> *mut u8 {
    with(|rt| {
        rt.heap
            .alloc_untracked(size as usize, (align.max(1)) as usize)
    })
}

/// Allocate and initialize one module-level global, mirroring the VM: an
/// untracked heap block (so it never counts toward the heap-balance check),
/// zeroed, with an optional scalar initializer written into its low bytes.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_global_alloc(size: u64, align: u64, has_init: u32, init: u64) -> *mut u8 {
    with(|rt| {
        let size = (size.max(1)) as usize;
        let pointer = rt.heap.alloc_untracked(size, (align.max(1)) as usize);
        let base = pointer as usize;
        let mut written = 0usize;
        while written < size {
            let chunk = u32::try_from(size - written).unwrap_or(8).min(8);
            rt.heap.write((base + written) as *mut u8, chunk, 0);
            written += chunk as usize;
        }
        if has_init != 0 {
            let width = u32::try_from(size).unwrap_or(8).min(8);
            rt.heap.write(pointer, width, init);
        }
        pointer
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_retain(pointer: *mut u8) {
    if strict::enabled() {
        strict::check(pointer, false);
    }
    with(|rt| rt.heap.retain(pointer));
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_release(pointer: *mut u8) {
    if strict::enabled() {
        strict::check(pointer, true);
    }
    with(|rt| rt.heap.release(pointer));
}

/// `ref.isunique`: 1 when `pointer` holds the only strong count.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_is_unique(pointer: *mut u8) -> u8 {
    with(|rt| u8::from(rt.heap.is_unique(pointer)))
}

/// `left % right` with the sign of `left`: the float remainder native code
/// calls, from the same table as `klf.math`.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_float_remainder(left: f64, right: f64) -> f64 {
    klf_rt::math::binary(26, left, right).unwrap_or(f64::NAN)
}

#[unsafe(no_mangle)]
pub extern "C" fn klfrt_free_raw(pointer: *mut u8) {
    trap_unless_freed(with(|rt| rt.heap.free_raw(pointer)), Some(pointer));
}

/// Traps with `KLF-RT-0006` when the heap refused a free because the slot was
/// already free. The trap is raised outside `with`, so the exit function and
/// the backtrace run with the runtime unborrowed.
fn trap_unless_freed(freed: bool, pointer: Option<*mut u8>) {
    if !freed {
        let details = pointer.map(|pointer| format!("the slot at {pointer:p} was freed before"));
        trap::trap_with(6, details.into_iter().collect());
    }
}

#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_load(pointer: *const u8, bytes: u32) -> u64 {
    match with(|rt| rt.heap.read(pointer, bytes)) {
        Some(value) => value,
        None => trap::trap_with(
            5,
            vec![format!("the program read {bytes} bytes at {pointer:p}, outside every live allocation")],
        ),
    }
}

#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_store(pointer: *mut u8, bytes: u32, value: u64) {
    if !with(|rt| rt.heap.write(pointer, bytes, value)) {
        trap::trap_with(
            5,
            vec![format!("the program wrote {bytes} bytes at {pointer:p}, outside every live allocation")],
        );
    }
}

/// A clean `return`: run the section-15.6 heap-balance check, then print the
/// return value (or `void`) and end the process.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_finish(value: u64, has_value: u32) -> ! {
    // Blocks other threads freed are counted live until this heap takes
    // them back.
    with(|rt| rt.heap.collect_remote());
    let live = with(|rt| rt.heap.live_blocks());
    if live != 0 {
        trap::trap_with(100, vec![format!("{live} blocks were still allocated when `main` returned")]);
    }
    ending::run_exit_function(ending::RETURNED, i64::from_ne_bytes(value.to_ne_bytes()));
    if has_value == 0 {
        println!("RET void");
    } else {
        println!("RET {value}");
    }
    std::process::exit(0)
}

/// A `fail`: print the failure value and end the process, matching `Outcome::Fail`.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_fail(value: u64) -> ! {
    interrupt::raise_if_serving(interrupt::Interrupt::Failed(value));
    ending::run_exit_function(ending::FAILED, i64::from_ne_bytes(value.to_ne_bytes()));
    if is_process() {
        eprintln!("uncaught failure {value}");
        std::process::exit(1)
    }
    println!("FAIL {value}");
    std::process::exit(0)
}

thread_local! {
    pub(crate) static ERROR_VALUE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Record a failure value and return a non-null error marker. The `ridlcc`
/// error register carries a pointer to an error object; here the object is a
/// thread-local value cell and the marker is an opaque non-null sentinel that is
/// only ever null-checked, never dereferenced.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_error_box(value: u64) -> *mut u8 {
    ERROR_VALUE.with(|cell| cell.set(value));
    std::ptr::without_provenance_mut(1)
}

/// Read the recorded failure value.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_error_value() -> u64 {
    ERROR_VALUE.with(std::cell::Cell::get)
}
