//! `KLF_HEAP_STRICT=1`: a count taken or dropped on an object that is already
//! dead traps with `KLF-RT-0004`, and names where the object was freed. Freed
//! slots are held back from reuse so a late count still finds them dead.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

const HELD: usize = 1 << 16;
const FRAMES: usize = 24;

/// Where each held-back slot was freed, by address, and the addresses in the
/// order they were freed.
type Freed = (HashMap<usize, Vec<u64>>, VecDeque<usize>);

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static FREED: RefCell<Freed> = RefCell::new((HashMap::new(), VecDeque::new()));
}

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KLF_HEAP_STRICT").is_some_and(|value| value == "1"))
}

pub(crate) fn check(pointer: *mut u8, releasing: bool) {
    let arm = !ARMED.replace(true);
    let count = crate::with(|rt| {
        if arm {
            rt.heap.quarantine_freed(HELD);
        }
        rt.heap.strong_count(pointer)
    });
    match count {
        Some(0) => {
            let action = if releasing { "release" } else { "retain" };
            eprintln!("KLF_HEAP_STRICT: {action} of the dead object at {pointer:p}");
            FREED.with(|freed| match freed.borrow().0.get(&(pointer as usize)) {
                Some(words) => {
                    eprintln!("it was freed at, innermost first:");
                    crate::backtrace::print_words(words);
                }
                None => eprintln!("where it was freed is not known"),
            });
            crate::klfrt_trap(4)
        }
        Some(1) if releasing => {
            let words = crate::backtrace::capture(FRAMES);
            FREED.with(|freed| {
                let (sites, order) = &mut *freed.borrow_mut();
                sites.insert(pointer as usize, words);
                order.push_back(pointer as usize);
                if order.len() > HELD
                    && let Some(oldest) = order.pop_front()
                {
                    sites.remove(&oldest);
                }
            });
        }
        _ => {}
    }
}
