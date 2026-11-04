use super::*;

pub(super) fn report(record: klf_diag::RuntimeDiagnostic) {
    if !services::record_diagnostic(record.clone()) {
        match record.subject {
            Some((subject_start, subject_end)) => println!(
                "DIAGS {} {} {} {} {} {} {}",
                record.code,
                record.start,
                record.end,
                record.file,
                record.origin,
                subject_start,
                subject_end
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
            vec![format!(
                "the program read {bytes} bytes at {pointer:p}, outside every live allocation"
            )],
        ),
    }
}

#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_store(pointer: *mut u8, bytes: u32, value: u64) {
    if !with(|rt| rt.heap.write(pointer, bytes, value)) {
        trap::trap_with(
            5,
            vec![format!(
                "the program wrote {bytes} bytes at {pointer:p}, outside every live allocation"
            )],
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
        trap::trap_with(
            100,
            vec![format!(
                "{live} blocks were still allocated when `main` returned"
            )],
        );
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
