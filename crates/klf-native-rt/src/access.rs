//! Dynamic exclusivity checks (section 11.8).
//!
//! `access.begin [read or modify] [static or dynamic or unchecked] %addr`
//! brackets a memory access. Under `static` the verifier proves that no two
//! overlapping accesses conflict, and under `unchecked` nothing is enforced, so
//! only `dynamic` reaches the runtime. Two overlapping accesses conflict when
//! either is a `modify`; a conflict traps with `KLF-RT-0003`.

use std::cell::RefCell;
use std::collections::HashMap;

/// The claims of one frame. A frame's are dropped when it exits, so an
/// `access.end` that never arrives cannot make a later frame conflict.
#[derive(Default)]
struct Frame {
    holds: HashMap<usize, (usize, bool)>,
}

// The claim frames of this thread. Index 0 is the thread's own base, created on
// first use; `enter` pushes and `leave` pops past it, so claims made by code
// that never opened a frame still conflict with each other.
thread_local! {
    static ACCESS: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

/// Record the claim and answer whether it conflicts with one the frame already
/// holds. A conflicting claim is not recorded, so ending the address releases
/// whatever claim did take it.
fn claim(address: *mut u8, modify: bool, size: u64) -> bool {
    let start = address as usize;
    let end = start.saturating_add(usize::try_from(size).unwrap_or(1).max(1));
    ACCESS.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.is_empty() {
            stack.push(Frame::default());
        }
        let frame = stack.last_mut().expect("just pushed");
        let conflict = frame.holds.iter().any(|(held, (held_end, held_modify))| {
            *held < end && start < *held_end && (modify || *held_modify)
        });
        if !conflict {
            frame.holds.insert(start, (end, modify));
        }
        conflict
    })
}

/// `klfrt_access_begin(address, mode, size)`: claim `[address, address + size)`
/// as a `read` (`mode` 0) or a `modify` (`mode` 1). A conflict traps with
/// `KLF-RT-0003`.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_access_begin(address: *mut u8, mode: i32, size: u64) {
    if claim(address, mode != 0, size) {
        let access = if mode != 0 { "modify" } else { "read" };
        crate::trap::trap_with(
            3,
            vec![format!("a {access} of {size} bytes at {address:p} overlaps an access still open")],
        );
    }
}

/// `klfrt_access_end(address)`: release the claim `klfrt_access_begin` took.
/// The innermost frame holding it releases it, so a claim taken in a callee
/// can be ended by its caller.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn klfrt_access_end(address: *mut u8) {
    let address = address as usize;
    ACCESS.with(|stack| {
        for frame in stack.borrow_mut().iter_mut().rev() {
            if frame.holds.remove(&address).is_some() {
                break;
            }
        }
    });
}

/// A frame entering starts holding nothing of its own, so claims its
/// predecessor never released cannot make it conflict.
pub(crate) fn enter() {
    ACCESS.with(|stack| stack.borrow_mut().push(Frame::default()));
}

/// A frame leaving drops the claims it held. The base frame stays.
pub(crate) fn leave() {
    ACCESS.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.len() > 1 {
            stack.pop();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{claim, enter, klfrt_access_end, leave};

    fn at(value: u64) -> *mut u8 {
        value as usize as *mut u8
    }

    #[test]
    fn two_reads_overlapping_do_not_conflict_and_two_modifies_do() {
        assert!(!claim(at(100), false, 8));
        assert!(!claim(at(104), false, 8));
        klfrt_access_end(at(100));
        klfrt_access_end(at(104));

        assert!(!claim(at(100), true, 8));
        assert!(claim(at(104), false, 8), "a read conflicts a modify");
        klfrt_access_end(at(100));
    }

    #[test]
    fn a_claim_conflicts_with_no_frame_entered() {
        // The entry sequence does not always open a frame, so the thread's own
        // base frame has to hold claims.
        assert!(!claim(at(200), true, 8));
        assert!(claim(at(200), true, 8));
        klfrt_access_end(at(200));
    }

    #[test]
    fn storage_that_does_not_overlap_does_not_conflict() {
        assert!(!claim(at(100), true, 8));
        assert!(!claim(at(108), true, 8));
        klfrt_access_end(at(100));
        klfrt_access_end(at(108));
    }

    #[test]
    fn a_frame_leaving_drops_the_claims_it_never_released() {
        enter();
        claim(at(300), true, 8);
        leave();
        enter();
        assert!(!claim(at(300), true, 8));
        klfrt_access_end(at(300));
        leave();
    }

    #[test]
    fn a_caller_can_end_a_claim_its_callee_took() {
        enter();
        assert!(!claim(at(400), true, 8));
        enter();
        klfrt_access_end(at(400));
        leave();
        assert!(!claim(at(400), true, 8));
        klfrt_access_end(at(400));
        leave();
    }
}
