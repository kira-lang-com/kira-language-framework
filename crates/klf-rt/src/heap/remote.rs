//! Frees from other threads (section 18.1: "cross-thread frees go to a
//! lock-free list on the span").
//!
//! Every thread of a native program allocates from a heap of its own, and
//! the heaps share one page map, so any thread resolves any pointer. Counts
//! are atomic in the slot metadata, so retaining and releasing need nothing
//! more. Freeing does: a span's free list belongs to the heap that owns it.
//! A thread freeing another heap's slot pushes it onto the span's
//! `remote_free` list instead, threading the list through the slots'
//! metadata words with their strong count at zero, and the push that finds
//! the list empty queues the span on the owner's [`Owner`]. The owner takes
//! the queue whole the next time it allocates and returns each slot to its
//! free list, so its allocation path stays free of atomics read-modify-write.
//!
//! A heap whose thread ends is orphaned rather than dropped: other threads
//! may still hold what it allocated. It moves into its `Owner`, and a free
//! arriving after that collects it there under a lock.

use super::Heap;
use crate::meta::{IMMORTAL, UNTRACKED, strong};
use crate::pagemap::PageMap;
use crate::span::{REMOTE_END, SpanHeader, meta_ref};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

/// The page map every shared heap registers its spans in.
pub(super) static SHARED_PAGES: PageMap = PageMap::new();

/// The next shared heap's number; 0 is a heap of its own.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// What other threads reach of a shared heap, through its spans' `owner`.
/// Never freed: a span names it for as long as the span lives.
pub(crate) struct Owner {
    /// Spans with slots on their `remote_free` list, linked through
    /// `next_remote`, newest first.
    spans: AtomicUsize,
    orphaned: AtomicBool,
    orphan: Mutex<Option<Heap>>,
}

/// Where a remotely freed slot's metadata keeps the next slot of the list:
/// the weak count's bits, which are dead once the slot is free.
const LINK_SHIFT: u32 = 32;
const LINK_MASK: u64 = (REMOTE_END as u64) << LINK_SHIFT;

impl Heap {
    /// A heap that shares the process page map with every other shared
    /// heap, so its allocations can be retained, released and freed from
    /// any thread. A native program gives each thread one.
    #[must_use]
    pub fn shared() -> Self {
        let mut heap = Self::with_pages(&SHARED_PAGES, false);
        heap.id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let owner = Box::leak(Box::new(Owner {
            spans: AtomicUsize::new(0),
            orphaned: AtomicBool::new(false),
            orphan: Mutex::new(None),
        }));
        heap.owner = core::ptr::from_ref(owner) as usize;
        heap
    }

    /// Whether this heap allocated the span `h`.
    #[inline]
    pub(super) fn owns(&self, h: *mut SpanHeader) -> bool {
        unsafe { &*h }.owner == self.owner
    }

    /// Hands slot `slot` of another heap's span back to that heap. `seen` is
    /// the metadata the caller last read; the push fails, answering false,
    /// when another thread freed the slot first.
    pub(super) fn free_remote(h: *mut SpanHeader, slot: u32, seen: u64) -> bool {
        let header = unsafe { &*h };
        let meta = unsafe { meta_ref(h, slot) };
        let mut head = header.remote_free.load(Ordering::Relaxed);
        let mut current = seen;
        loop {
            let link = (current & UNTRACKED) | (u64::from(head) << LINK_SHIFT);
            match meta.compare_exchange(current, link, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break,
                Err(actual) => {
                    if strong(actual) == 0 && actual & IMMORTAL == 0 && strong(seen) != 0 {
                        return false;
                    }
                    current = actual;
                }
            }
        }
        loop {
            match header.remote_free.compare_exchange(
                head,
                slot,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => {
                    head = actual;
                    let kept = meta.load(Ordering::Relaxed) & !LINK_MASK;
                    meta.store(kept | (u64::from(head) << LINK_SHIFT), Ordering::Relaxed);
                }
            }
        }
        let owner = unsafe { &*(header.owner as *const Owner) };
        if head == REMOTE_END {
            let mut top = owner.spans.load(Ordering::Relaxed);
            loop {
                header.next_remote.store(top, Ordering::Relaxed);
                match owner.spans.compare_exchange_weak(top, h as usize, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(actual) => top = actual,
                }
            }
        }
        if owner.orphaned.load(Ordering::Acquire) {
            let mut orphan = owner.orphan.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(heap) = orphan.as_mut() {
                heap.collect_remote();
            }
        }
        true
    }

    /// Returns every slot other threads freed to this heap's free lists.
    /// Each allocation starts here, so the check is one load.
    #[inline]
    pub fn collect_remote(&mut self) {
        if self.owner != 0
            && unsafe { &*(self.owner as *const Owner) }.spans.load(Ordering::Relaxed) != 0
        {
            self.collect_queued();
        }
    }

    #[cold]
    fn collect_queued(&mut self) {
        let owner = unsafe { &*(self.owner as *const Owner) };
        let mut span = owner.spans.swap(0, Ordering::Acquire);
        self.invalidate_access_cache();
        while span != 0 {
            let h = span as *mut SpanHeader;
            // Read before the list is taken: a free after that queues the
            // span again through this link.
            span = unsafe { &*h }.next_remote.load(Ordering::Relaxed);
            let mut slot = unsafe { &*h }.remote_free.swap(REMOTE_END, Ordering::Acquire);
            while slot != REMOTE_END {
                let state = unsafe { meta_ref(h, slot) }.load(Ordering::Relaxed);
                let next = ((state & LINK_MASK) >> LINK_SHIFT) as u32;
                let pa = unsafe { &*h }.payload_addr(slot);
                self.retire(h, slot, pa, state & UNTRACKED == 0);
                slot = next;
            }
        }
    }

    /// Ends this heap's thread's use of it. A shared heap is kept, with
    /// everything it allocated, for the threads that still hold some of it;
    /// a heap of its own is dropped.
    pub fn orphan(mut self) {
        if self.owner == 0 {
            return;
        }
        self.collect_remote();
        let owner = unsafe { &*(self.owner as *const Owner) };
        let mut orphan = owner.orphan.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        owner.orphaned.store(true, Ordering::Release);
        // A free that pushed before the flag was set did not collect.
        self.collect_remote();
        *orphan = Some(self);
    }
}
