// The native stack bound behind the recursion limit.

/// The recursion limit natively: the lowest stack address a function may start at.
#[unsafe(no_mangle)]
pub static mut klfrt_stack_limit: usize = 0;

const STACK_MARGIN: usize = 256 * 1024;

#[cfg(unix)]
fn stack_address() -> usize {
    let marker = 0u8;
    std::ptr::addr_of!(marker) as usize
}

/// Initializes the native stack bound. `top` is the synthesized `main`'s
/// own stack pointer, which nothing reads now that no collection scans the
/// stack.
#[unsafe(no_mangle)]
pub extern "C" fn klfrt_stack_init(top: usize) {
    let _ = top;
    let limit = stack_floor().saturating_add(STACK_MARGIN);
    unsafe {
        if klfrt_stack_limit == 0 {
            klfrt_stack_limit = limit;
        }
    }
}

#[cfg(unix)]
fn stack_floor() -> usize {
    unsafe extern "C" {
        fn getrlimit(resource: i32, limits: *mut [u64; 2]) -> i32;
    }
    let mut limits = [0u64; 2];
    let size = if unsafe { getrlimit(3, &mut limits) } == 0 && limits[0] < (1u64 << 40) {
        usize::try_from(limits[0]).unwrap_or(8 << 20)
    } else {
        256 << 20
    };
    stack_address().saturating_sub(size.max(STACK_MARGIN + (1 << 20)))
}

#[cfg(windows)]
fn stack_floor() -> usize {
    unsafe extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }
    let (mut low, mut high) = (0usize, 0usize);
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
    low
}

#[cfg(not(any(unix, windows)))]
fn stack_floor() -> usize {
    0
}

thread_local! {
    /// The calling thread's stack bounds: its lowest address and the address
    /// below which a function may not start, found on its first counted call.
    static BOUNDS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Whether the calling thread is within `STACK_MARGIN` of the end of its
/// stack. A frame outside the thread's own stack, such as one on a stack a
/// task was given, is never counted as exhausted.
#[cfg(unix)]
pub(crate) fn exhausted() -> bool {
    let here = stack_address();
    BOUNDS.with(|bounds| {
        let (mut low, mut limit) = bounds.get();
        if limit == 0 {
            match thread_stack_floor() {
                Some(floor) => {
                    low = floor;
                    limit = floor.saturating_add(STACK_MARGIN);
                }
                None => limit = 1,
            }
            bounds.set((low, limit));
        }
        here >= low && here < limit
    })
}

#[cfg(not(unix))]
pub(crate) fn exhausted() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn thread_stack_floor() -> Option<usize> {
    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_stackaddr_np(thread: usize) -> usize;
        fn pthread_get_stacksize_np(thread: usize) -> usize;
    }
    // SAFETY: both read the calling thread's own attributes.
    let (top, size) = unsafe {
        let thread = pthread_self();
        (pthread_get_stackaddr_np(thread), pthread_get_stacksize_np(thread))
    };
    (top > size && size > STACK_MARGIN).then(|| top - size)
}

#[cfg(target_os = "linux")]
fn thread_stack_floor() -> Option<usize> {
    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attributes: *mut [u64; 16]) -> i32;
        fn pthread_attr_getstack(attributes: *const [u64; 16], address: *mut usize, size: *mut usize) -> i32;
        fn pthread_attr_destroy(attributes: *mut [u64; 16]) -> i32;
    }
    let mut attributes = [0u64; 16];
    let (mut address, mut size) = (0usize, 0usize);
    // SAFETY: `attributes` is larger than any target's pthread_attr_t, and the
    // calls only read the calling thread's own attributes.
    let found = unsafe {
        if pthread_getattr_np(pthread_self(), &mut attributes) != 0 {
            return None;
        }
        let found = pthread_attr_getstack(&attributes, &mut address, &mut size) == 0;
        pthread_attr_destroy(&mut attributes);
        found
    };
    (found && address != 0 && size > STACK_MARGIN).then_some(address)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn thread_stack_floor() -> Option<usize> {
    None
}
