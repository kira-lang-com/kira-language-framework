//! What a program leaves allocated when it ends.

use super::{ARENA_MARK, Heap};
use crate::meta::{UNTRACKED, strong};
use crate::span::{SpanHeader, meta_ref};
use core::sync::atomic::Ordering;

/// What is still allocated when a program ends: memory the program never
/// released.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExitReport {
    /// Counted, unique and raw allocations still holding a strong count.
    pub tracked: usize,
    pub tracked_bytes: usize,
    /// Untracked allocations still holding a strong count: growable-array
    /// and string buffers, and blocks a language frees itself.
    pub untracked: usize,
    pub untracked_bytes: usize,
}

impl Heap {
    /// Counts every allocation still holding a strong count. For a program
    /// whose frames have all returned.
    #[must_use]
    pub fn exit_report(&self) -> ExitReport {
        let mut report = ExitReport::default();
        self.each_slot(|h, slot| {
            let state = unsafe { meta_ref(h, slot) }.load(Ordering::Relaxed);
            if strong(state) == 0 {
                return;
            }
            let size = unsafe { &*h }.slot_size as usize;
            if state & UNTRACKED != 0 {
                report.untracked += 1;
                report.untracked_bytes += size;
            } else {
                report.tracked += 1;
                report.tracked_bytes += size;
            }
        });
        report
    }

    /// Calls `visit` with every slot of every span and large run, the frame
    /// arena aside.
    fn each_slot(&self, mut visit: impl FnMut(*mut SpanHeader, u32)) {
        for span in &self.spans {
            let h = span.base.as_ptr().cast::<SpanHeader>();
            for slot in 0..unsafe { &*h }.slot_count {
                visit(h, slot);
            }
        }
        for &base in self.large.keys() {
            let h = base as *mut SpanHeader;
            if unsafe { &*h }._pad != ARENA_MARK {
                visit(h, 0);
            }
        }
    }
}
