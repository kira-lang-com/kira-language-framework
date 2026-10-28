//! Span headers and slot metadata access (section 18.1).
//!
//! A span is 64 KiB aligned to 64 KiB. The header occupies the first 64 bytes.
//! The slot metadata table (8 bytes per slot) follows directly, then the payload
//! begins at `data_start`. A large allocation uses a run of contiguous spans with
//! one slot; its header sits at the run base.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

pub(crate) const SPAN_SIZE: usize = 64 * 1024;
pub(crate) const HEADER_SIZE: usize = 64;
pub(crate) const META_BYTES: usize = 8;
pub(crate) const MAGIC: u32 = 0x4B4C_4652; // "KLFR"
pub(crate) const SENTINEL: u32 = u32::MAX;
/// Marks a header that owns a dedicated large run rather than a pooled class.
pub(crate) const LARGE_CLASS: u16 = u16::MAX;

/// The end of a span's cross-thread free list.
pub(crate) const REMOTE_END: u32 = 0x00FF_FFFF;

/// The span header, within the 64 bytes the spec gives it. `data_start`,
/// `run_pages`, `owner`, `destroys` and `next_remote` are implementation
/// fields the spec leaves to the allocator.
///
/// `owning_thread_cache` names the heap that allocates from the span, and
/// `owner` is that heap's [`crate::heap::Owner`]. Only the owning heap pops
/// or pushes the free list; another thread freeing a slot pushes it onto
/// `remote_free` instead, a lock-free list threaded through the slots'
/// metadata words, and the first such push queues the span on the owner
/// through `next_remote` (section 18.1).
#[repr(C)]
pub(crate) struct SpanHeader {
    pub(crate) magic: u32,
    pub(crate) class_index: u16,
    pub(crate) _pad: u16,
    pub(crate) slot_size: u32,
    pub(crate) slot_count: u32,
    pub(crate) free_list_head: u32,
    pub(crate) data_start: u32,
    pub(crate) run_pages: u32,
    pub(crate) owning_thread_cache: u32,
    pub(crate) remote_free: AtomicU32,
    pub(crate) owner: usize,
    /// Each slot's destroy function, or 0: a table of `slot_count` words,
    /// made the first time a slot of the span is allocated with one.
    pub(crate) destroys: usize,
    pub(crate) next_remote: AtomicUsize,
}

const _: () = assert!(core::mem::size_of::<SpanHeader>() <= HEADER_SIZE);

/// Borrow the metadata word for `slot` living just past the header of `h`.
///
/// # Safety
/// `h` must point at a live span header whose metadata table holds `slot`.
pub(crate) unsafe fn meta_ref<'a>(h: *mut SpanHeader, slot: u32) -> &'a AtomicU64 {
    let addr = h as usize + HEADER_SIZE + slot as usize * META_BYTES;
    unsafe { &*(addr as *const AtomicU64) }
}

impl SpanHeader {
    pub(crate) fn base(&self) -> usize {
        core::ptr::from_ref(self) as usize
    }

    pub(crate) fn payload_addr(&self, slot: u32) -> usize {
        self.base() + self.data_start as usize + slot as usize * self.slot_size as usize
    }

    pub(crate) fn is_large(&self) -> bool {
        self.class_index == LARGE_CLASS
    }

    /// Take a free slot from the span's free list, if any.
    pub(crate) fn pop(&mut self) -> Option<u32> {
        let head = self.free_list_head;
        if head == SENTINEL {
            return None;
        }
        let next = unsafe { meta_ref(core::ptr::from_mut(self), head) }.load(Ordering::Relaxed);
        self.free_list_head = (next >> 32) as u32;
        Some(head)
    }

    /// Return `slot` to the free list. The freed metadata word keeps its strong
    /// count at zero (the next-index lives in the high word), so a stale
    /// `weak_load` on a freed slot correctly reads zero and returns null.
    pub(crate) fn push(&mut self, slot: u32) {
        let encoded = u64::from(self.free_list_head) << 32;
        unsafe { meta_ref(core::ptr::from_mut(self), slot) }.store(encoded, Ordering::Relaxed);
        self.free_list_head = slot;
    }

    /// Lay down the initial free list threading every slot in order.
    pub(crate) fn init_free_list(&mut self) {
        let n = self.slot_count;
        let h = core::ptr::from_mut(self);
        for i in 0..n {
            let next = if i + 1 < n { i + 1 } else { SENTINEL };
            unsafe { meta_ref(h, i) }.store(u64::from(next) << 32, Ordering::Relaxed);
        }
        self.free_list_head = if n == 0 { SENTINEL } else { 0 };
    }
}
