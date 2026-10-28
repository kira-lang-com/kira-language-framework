//! Kernel services `klf.thread.*` (section 18.9): threads a native program
//! starts, joins and parks.
//!
//! A thread runs a C-callable entry with one word of argument. It allocates
//! from a heap of its own on the shared page map (`Heap::shared`), so what it
//! makes can be handed to any other thread, and when the entry returns its
//! heap is orphaned for whoever still holds its blocks. A thread is named by
//! a process-wide number, which `park` and `unpark` take; a spawned thread is
//! also held by a handle until it is joined or detached.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread::{JoinHandle, Thread};
use std::time::Duration;

/// The stack a spawned thread gets: as deep as a main thread's, since Kira
/// code recurses as far there as anywhere.
const STACK_BYTES: usize = 8 << 20;

/// The failure a service answers below zero: the platform refused a thread.
const REFUSED: i64 = -1;
/// The handle or thread number names nothing.
const UNKNOWN: i64 = -2;

static NEXT_ID: AtomicI64 = AtomicI64::new(1);
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

/// Every thread that has a number, by it, for `unpark`.
static THREADS: Mutex<Option<HashMap<i64, Thread>>> = Mutex::new(None);
/// Spawned threads not yet joined or detached, by handle.
static HANDLES: Mutex<Option<HashMap<i64, JoinHandle<()>>>> = Mutex::new(None);

thread_local! {
    static ID: Registration = Registration::new();
}

/// The calling thread's number, entered in [`THREADS`] for as long as the
/// thread lives.
struct Registration(i64);

impl Registration {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        THREADS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(id, std::thread::current());
        Self(id)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(threads) = THREADS.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            threads.remove(&self.0);
        }
    }
}

fn current_id() -> i64 {
    ID.with(|registration| registration.0)
}

/// A word C may carry to another thread: the entry and its argument.
struct Start {
    entry: extern "C" fn(i64),
    argument: i64,
}

// SAFETY: the entry is a C function pointer and the argument a plain word;
// whatever the argument stands for, the program handed it over on purpose.
unsafe impl Send for Start {}

/// Kernel service `klf.thread.spawn`: starts a thread running `entry`
/// (`void (*)(int64_t)`) on `argument`. Answers the thread's handle, above
/// zero, or a failure below zero.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_spawn(entry: *const u8, argument: i64) -> i64 {
    if entry.is_null() {
        return UNKNOWN;
    }
    // SAFETY: the caller passes a C function taking one 64-bit word.
    let entry = unsafe { std::mem::transmute::<*const u8, extern "C" fn(i64)>(entry) };
    let start = Start { entry, argument };
    let spawned = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let start = start;
            current_id();
            (start.entry)(start.argument);
        });
    let Ok(handle) = spawned else {
        return REFUSED;
    };
    let number = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    HANDLES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(number, handle);
    number
}

fn take_handle(handle: i64) -> Option<JoinHandle<()>> {
    HANDLES.lock().unwrap_or_else(PoisonError::into_inner).as_mut()?.remove(&handle)
}

/// Kernel service `klf.thread.join`: waits for the thread `handle` names to
/// end. Answers 1, or a failure below zero when the handle names nothing or
/// the thread's entry unwound.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_join(handle: i64) -> i64 {
    match take_handle(handle) {
        Some(thread) => {
            if thread.join().is_ok() {
                1
            } else {
                REFUSED
            }
        }
        None => UNKNOWN,
    }
}

/// Kernel service `klf.thread.detach`: lets the thread `handle` names run
/// on unjoined. Answers 1, or a failure below zero.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_detach(handle: i64) -> i64 {
    if take_handle(handle).is_some() { 1 } else { UNKNOWN }
}

/// Kernel service `klf.thread.current`: the calling thread's number, above
/// zero and never reused while the process runs.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_current() -> i64 {
    current_id()
}

/// Kernel service `klf.thread.park`: blocks the calling thread until it is
/// unparked or `timeout` nanoseconds pass (forever when negative). An
/// `unpark` that came first makes it return at once. Answers 1; it may also
/// return spuriously, so a caller rechecks what it waits for.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_park(timeout: i64) -> i64 {
    current_id();
    match u64::try_from(timeout) {
        Ok(nanos) => std::thread::park_timeout(Duration::from_nanos(nanos)),
        Err(_) => std::thread::park(),
    }
    1
}

/// Kernel service `klf.thread.unpark`: wakes the thread numbered `id` from
/// its park, or makes its next park return at once. Answers 1, or a failure
/// below zero when no living thread has that number.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_unpark(id: i64) -> i64 {
    let thread = THREADS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|threads| threads.get(&id).cloned());
    match thread {
        Some(thread) => {
            thread.unpark();
            1
        }
        None => UNKNOWN,
    }
}

/// Kernel service `klf.thread.yield`: offers the processor to another thread.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_yield() {
    std::thread::yield_now();
}

/// Kernel service `klf.thread.parallelism`: how many threads can run at once,
/// at least 1.
#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_thread_parallelism() -> i64 {
    std::thread::available_parallelism()
        .map_or(1, |count| i64::try_from(count.get()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI64;

    static SEEN: AtomicI64 = AtomicI64::new(0);

    extern "C" fn record(argument: i64) {
        SEEN.fetch_add(argument, Ordering::SeqCst);
    }

    #[test]
    fn a_spawned_thread_runs_its_entry_and_joins() {
        let handle = klfsvc_thread_spawn(record as *const u8, 40);
        assert!(handle > 0);
        assert_eq!(klfsvc_thread_join(handle), 1);
        assert_eq!(klfsvc_thread_join(handle), UNKNOWN);
        assert!(SEEN.load(Ordering::SeqCst) >= 40);
    }

    static PARKED: AtomicI64 = AtomicI64::new(0);

    extern "C" fn park_then_note(_: i64) {
        PARKED.store(klfsvc_thread_current(), Ordering::SeqCst);
        while PARKED.load(Ordering::SeqCst) > 0 {
            klfsvc_thread_park(-1);
        }
    }

    #[test]
    fn unpark_wakes_a_parked_thread_by_number() {
        let handle = klfsvc_thread_spawn(park_then_note as *const u8, 0);
        let id = loop {
            let id = PARKED.load(Ordering::SeqCst);
            if id > 0 {
                break id;
            }
            klfsvc_thread_yield();
        };
        assert_ne!(id, klfsvc_thread_current());
        PARKED.store(-1, Ordering::SeqCst);
        assert_eq!(klfsvc_thread_unpark(id), 1);
        assert_eq!(klfsvc_thread_join(handle), 1);
        assert_eq!(klfsvc_thread_unpark(id), UNKNOWN);
    }

    #[test]
    fn a_park_with_a_timeout_returns() {
        assert_eq!(klfsvc_thread_park(1_000), 1);
        assert!(klfsvc_thread_parallelism() >= 1);
    }
}
