//! Interrupting definition code in a language server (`KLF-EDITOR.md`
//! section 8.6).
//!
//! `klf build` gives a definition's functions unwind tables, cleanup pads
//! that release the counted values a frame owns, and a poll at every function
//! entry and loop back edge. The poll loads [`klfrt_interrupt_pending`] and
//! calls [`klfrt_poll`] only when it is nonzero. A cancelled analysis, a trap
//! or a `fail` raises an [`Interrupt`] as a Rust unwind, which runs those pads
//! on its way to [`catching`] at the phase boundary.
//!
//! Outside [`serving`] nothing raises: a trap ends the process as it always
//! did, so a command-line driver behaves exactly as before.

use std::cell::{Cell, RefCell};
use std::ffi::c_int;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

/// The number of cancellations no analysis has acknowledged yet. Every poll
/// in definition code reads it, so it stays zero unless one is pending.
#[unsafe(no_mangle)]
pub static klfrt_interrupt_pending: AtomicU32 = AtomicU32::new(0);

/// Why definition code stopped before its phase returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Interrupt {
    /// The analysis it ran for was cancelled.
    Cancelled,
    /// A `KLF-RT` trap with this code.
    Trapped(i32),
    /// A language check that failed, named by its message: the source it
    /// checked and where that stands.
    CheckFailed(String),
    /// An uncaught `fail` with this value.
    Failed(u64),
    /// A panic in kernel code the definition called.
    Panicked(String),
}

thread_local! {
    static SERVING: Cell<bool> = const { Cell::new(false) };
    static UNWINDING: Cell<bool> = const { Cell::new(false) };
    static ARMED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Whether this thread runs definition code for a language server, where
/// traps and cancellation unwind instead of ending the process.
#[must_use]
pub fn serving() -> bool {
    SERVING.with(Cell::get)
}

/// Runs `body` as definition code of a server: `cancelled` turning true stops
/// it at its next poll, and traps unwind to [`catching`].
pub fn serve<R>(cancelled: Arc<AtomicBool>, body: impl FnOnce() -> R) -> R {
    struct Restore(bool, Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SERVING.with(|serving| serving.set(self.0));
            ARMED.with(|armed| *armed.borrow_mut() = self.1.take());
        }
    }
    let previous = ARMED.with(|armed| armed.borrow_mut().replace(cancelled));
    let _restore = Restore(SERVING.with(|serving| serving.replace(true)), previous);
    body()
}

/// One analysis's share of [`klfrt_interrupt_pending`]: [`Pending::signal`]
/// raises it when the analysis is cancelled, and dropping the guard lowers it
/// again if it was raised, whichever happens first.
#[derive(Default)]
pub struct Pending(Arc<AtomicU8>);

const RUNNING: u8 = 0;
const SIGNALLED: u8 = 1;
const DONE: u8 = 2;

impl Pending {
    /// A callback for the analysis's cancellation that makes polls look.
    pub fn signal(&self) -> impl Fn() + Send + Sync + 'static {
        let state = Arc::clone(&self.0);
        move || {
            if state
                .compare_exchange(RUNNING, SIGNALLED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                klfrt_interrupt_pending.fetch_add(1, Ordering::Release);
            }
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.0.swap(DONE, Ordering::AcqRel) == SIGNALLED {
            klfrt_interrupt_pending.fetch_sub(1, Ordering::Release);
        }
    }
}

/// Whether this thread serves an analysis that was cancelled.
#[must_use]
pub fn cancel_requested() -> bool {
    serving() && cancelled()
}

fn cancelled() -> bool {
    ARMED.with(|armed| {
        armed
            .borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    })
}

/// Raises `interrupt` when this thread serves and is not already unwinding.
pub(crate) fn raise_if_serving(interrupt: Interrupt) {
    if serving() && !UNWINDING.with(Cell::get) {
        raise(interrupt);
    }
}

fn raise(interrupt: Interrupt) -> ! {
    UNWINDING.with(|unwinding| unwinding.set(true));
    std::panic::resume_unwind(Box::new(interrupt))
}

/// The slow path of a poll: raises [`Interrupt::Cancelled`] when this
/// thread's analysis was cancelled.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_poll() {
    if cancelled() {
        raise_if_serving(Interrupt::Cancelled);
    }
}

/// The poll every kernel service makes before it runs.
pub(crate) fn poll_service() {
    if klfrt_interrupt_pending.load(Ordering::Relaxed) != 0 {
        klfrt_poll();
    }
}

/// Runs `body`, which calls into definition code, and catches an interrupt
/// raised inside it. The call depth and the frames the unwound functions
/// pushed are put back as they were when `body` started.
///
/// # Errors
/// Returns the interrupt that stopped `body`.
pub fn catching<R>(body: impl FnOnce() -> R) -> Result<R, Interrupt> {
    let depth = crate::depth();
    let frames = crate::frame_count();
    let result = std::panic::catch_unwind(AssertUnwindSafe(body));
    UNWINDING.with(|unwinding| unwinding.set(false));
    result.map_err(|payload| {
        crate::unwind_frames(frames, depth);
        match payload.downcast::<Interrupt>() {
            Ok(interrupt) => *interrupt,
            Err(payload) => Interrupt::Panicked(
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic".to_owned()),
            ),
        }
    })
}

#[repr(C)]
pub struct UnwindException {
    _private: [u8; 0],
}

#[repr(C)]
pub struct UnwindContext {
    _private: [u8; 0],
}

unsafe extern "C" {
    fn _Unwind_GetLanguageSpecificData(context: *mut UnwindContext) -> *const u8;
    fn _Unwind_GetRegionStart(context: *mut UnwindContext) -> usize;
    fn _Unwind_GetIPInfo(context: *mut UnwindContext, before: *mut c_int) -> usize;
    fn _Unwind_SetGR(context: *mut UnwindContext, index: c_int, value: usize);
    fn _Unwind_SetIP(context: *mut UnwindContext, value: usize);
}

const SEARCH_PHASE: c_int = 1;
const FATAL_PHASE1_ERROR: c_int = 3;
const INSTALL_CONTEXT: c_int = 7;
const CONTINUE_UNWIND: c_int = 8;

/// The personality of definition code. Its language-specific data is a
/// table `klf build` writes per function: a 32-bit count, then that many
/// pairs of 32-bit offsets from the function's start, a call's return
/// address and the cleanup pad for it, sorted by return address. A pad
/// releases what the frame owns and resumes the unwind, so the search phase
/// passes every definition frame by.
///
/// # Safety
/// Called by the system unwinder with its own context and exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfrt_personality(
    version: c_int,
    actions: c_int,
    _class: u64,
    exception: *mut UnwindException,
    context: *mut UnwindContext,
) -> c_int {
    if version != 1 {
        return FATAL_PHASE1_ERROR;
    }
    if actions & SEARCH_PHASE != 0 {
        return CONTINUE_UNWIND;
    }
    // SAFETY: the unwinder passes a live context for the frame being unwound.
    let table = unsafe { _Unwind_GetLanguageSpecificData(context) };
    if table.is_null() {
        return CONTINUE_UNWIND;
    }
    let mut before: c_int = 0;
    // SAFETY: as above.
    let (address, start) = unsafe {
        (
            _Unwind_GetIPInfo(context, &raw mut before),
            _Unwind_GetRegionStart(context),
        )
    };
    let offset = address.wrapping_sub(start) + usize::from(before != 0);
    // SAFETY: `klf build` wrote the table this function's FDE points at in
    // the shape documented above.
    let pad = unsafe { landing_pad(table, offset) };
    let Some(pad) = pad else {
        return CONTINUE_UNWIND;
    };
    // SAFETY: as above; registers 0 and 1 are the exception data registers
    // of every target `klf build` writes tables for.
    unsafe {
        _Unwind_SetGR(context, 0, exception as usize);
        _Unwind_SetGR(context, 1, 0);
        _Unwind_SetIP(context, start + pad);
    }
    INSTALL_CONTEXT
}

/// # Safety
/// `table` must point at a landing table as [`klfrt_personality`] describes.
unsafe fn landing_pad(table: *const u8, offset: usize) -> Option<usize> {
    // SAFETY: the table starts with its count, then the pairs, unaligned.
    let read = |index: usize| unsafe { table.add(index * 4).cast::<u32>().read_unaligned() };
    let count = read(0) as usize;
    let (mut low, mut high) = (0, count);
    while low < high {
        let middle = low + (high - low) / 2;
        let ret = read(1 + middle * 2) as usize;
        match ret.cmp(&offset) {
            std::cmp::Ordering::Equal => return Some(read(2 + middle * 2) as usize),
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
        }
    }
    None
}
