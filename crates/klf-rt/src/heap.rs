//! The heap: allocation, reference counting by address, raw deletion and weak
//! loads (sections 18.1 through 18.5).

use core::cell::Cell;
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering, fence};
use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::collections::{HashMap, VecDeque};

use crate::meta::{
    DELETED, IMMORTAL, INIT, QUARANTINED, STRONG_MASK, UNTRACKED, WEAK_MASK, WEAK_SHIFT, strong,
    weak,
};
use crate::pagemap::PageMap;

mod arena;
mod bulk;
mod access;
mod cycles;
mod destroys;
mod exit;
mod remote;
mod scalar;

use destroys::{clear_destroy, free_destroys, run_destroy, set_destroy};
use crate::sizeclass::{build_classes, class_for, large_layout, layout_for_class};
use crate::span::{LARGE_CLASS, MAGIC, REMOTE_END, SENTINEL, SPAN_SIZE, SpanHeader, meta_ref};
pub use exit::ExitReport;

const SMALL_LOOKUP_STEPS: usize = 64;

/// The frame arena's chunk size, and the mark its chunk headers carry so the
/// free paths leave them alone.
const ARENA_CHUNK: usize = 1 << 20;
const ARENA_MARK: u16 = 0xA7E;

/// A destroy callback run when a counted allocation's strong count hits zero.
/// For M1 it integrates with a bytecode VM not yet wired in, so it is a plain
/// C-ABI function pointer stored per allocation. Trivial types pass `None`.
pub type DestroyFn = extern "C" fn(*mut u8);

/// A hasher for payload addresses: one multiply, where the default SipHash
/// cost more than the allocation it keyed.
#[derive(Default)]
pub(crate) struct AddressHasher(u64);

impl core::hash::Hasher for AddressHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 =
                (self.0.rotate_left(5) ^ u64::from(*byte)).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn write_usize(&mut self, value: usize) {
        self.0 = (value as u64 >> 4).wrapping_mul(0x9e_37_79_b9_7f_4a_7c_15);
    }
}

type AddressMap<V> = HashMap<usize, V, core::hash::BuildHasherDefault<AddressHasher>>;

/// One backing memory block owned by the heap.
struct SpanRecord {
    base: ptr::NonNull<u8>,
    layout: Layout,
}

/// The KLF heap. One per VM instance, or one per thread of a native program
/// ([`Heap::shared`]).
pub struct Heap {
    classes: Vec<u32>,
    /// The class for each small size in 16-byte steps at the default
    /// alignment, so the common allocation skips the class search.
    small_classes: Vec<Option<usize>>,
    /// Per size class, the spans with at least one free slot. A span leaves
    /// the list when it fills and rejoins when a slot is freed into it, so an
    /// allocation never scans full spans.
    partial: Vec<Vec<*mut SpanHeader>>,
    spans: Vec<SpanRecord>,
    /// Large runs by base address, so freeing one finds its layout at once.
    large: AddressMap<Layout>,
    /// The bytes `spans` and `large` hold, kept as they change so an
    /// allocation's limit check does not sum them.
    backing: usize,
    /// The page map: the process's, shared by every thread's heap, or one of
    /// this heap's own (`own_pages`).
    pages: *const PageMap,
    own_pages: bool,
    /// This heap's number in its spans' `owning_thread_cache`, and its
    /// [`Owner`] in their `owner`; both 0 for a heap with pages of its own.
    id: u32,
    owner: usize,
    cycles: cycles::Cycles,
    live: usize,
    /// Disposal clears both entries; alternating input and output accesses can
    /// skip page lookup and allocation metadata without admitting freed slots.
    last_access: Cell<(usize, usize)>,
    previous_access: Cell<(usize, usize)>,
    native_page: Cell<(usize, u64)>,
    /// Stack allocations: chunks used as a stack of frames, with the chunk
    /// and offset of the next allocation.
    arena: Vec<*mut SpanHeader>,
    arena_chunk: usize,
    arena_offset: usize,
    /// Whether an address outside the heap is native memory, not a fault.
    native_memory: bool,
    /// Freed slots held back from reuse, oldest first, and how many to hold.
    quarantine: Option<(VecDeque<(usize, u32)>, usize)>,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

impl Heap {
    /// Bytes of backing storage owned by this heap, including allocator spans.
    #[must_use]
    pub fn backing_bytes(&self) -> usize {
        self.backing
    }

    /// Checks a prospective allocation before asking the host allocator for
    /// storage. The limit includes allocator overhead and retained small spans.
    #[must_use]
    pub fn allocation_fits(&self, size: usize, align: usize, limit: usize) -> bool {
        if !align.is_power_of_two() || size > u32::MAX as usize {
            return false;
        }
        let Some(worst_case) = size
            .checked_add(align)
            .and_then(|n| n.checked_add(2 * SPAN_SIZE))
        else {
            return false;
        };
        if worst_case > isize::MAX as usize {
            return false;
        }
        let additional = match class_for(&self.classes, size, align) {
            Some(class) if !self.partial[class].is_empty() => 0,
            Some(_) => SPAN_SIZE,
            None => large_layout(size, align).1,
        };
        self.backing_bytes()
            .checked_add(additional)
            .is_some_and(|total| total <= limit)
    }

    /// A heap with a page map of its own, for one VM instance.
    #[must_use]
    pub fn new() -> Self {
        Self::with_pages(Box::leak(Box::new(PageMap::new())), true)
    }

    fn with_pages(pages: &'static PageMap, own_pages: bool) -> Self {
        let classes = build_classes();
        let partial = (0..classes.len()).map(|_| Vec::new()).collect();
        let small_classes = (0..=SMALL_LOOKUP_STEPS)
            .map(|step| class_for(&classes, step * 16, 16))
            .collect();
        Self {
            classes,
            small_classes,
            partial,
            spans: Vec::new(),
            large: AddressMap::default(),
            backing: 0,
            pages,
            own_pages,
            id: 0,
            owner: 0,
            cycles: cycles::Cycles::default(),
            live: 0,
            last_access: Cell::new((0, 0)),
            previous_access: Cell::new((0, 0)),
            native_page: Cell::new((usize::MAX, u64::MAX)),
            arena: Vec::new(),
            arena_chunk: 0,
            arena_offset: 0,
            native_memory: false,
            quarantine: crate::valgrind::running().then(|| (VecDeque::new(), 1 << 16)),
        }
    }

    #[inline]
    pub(crate) fn pages(&self) -> &PageMap {
        // SAFETY: the shared map is static and an own one lives until drop.
        unsafe { &*self.pages }
    }

    /// Holds the last `limit` freed slots back from reuse, so a count taken or
    /// dropped on a dead object still finds it dead.
    pub fn quarantine_freed(&mut self, limit: usize) {
        self.quarantine = Some((VecDeque::new(), limit));
    }

    /// The strong count of the object `ptr` points into, or nothing when it is
    /// not a heap object or its count is frozen.
    #[must_use]
    pub fn strong_count(&self, ptr: *mut u8) -> Option<u32> {
        let (h, slot, _) = self.resolve(ptr)?;
        let cur = unsafe { meta_ref(h, slot) }.load(Ordering::Relaxed);
        if cur & IMMORTAL != 0 {
            return None;
        }
        Some(strong(cur))
    }

    /// Counted allocation, strong count 1, uninitialized payload.
    pub fn alloc_counted(
        &mut self,
        size: usize,
        align: usize,
        destroy: Option<DestroyFn>,
    ) -> *mut u8 {
        self.alloc_impl(size, align, destroy)
    }

    /// Unique (uniq) allocation, strong count 1.
    pub fn alloc_unique(&mut self, size: usize, align: usize) -> *mut u8 {
        self.alloc_impl(size, align, None)
    }

    /// Unmanaged raw allocation, strong count 1 held by the manual owner.
    pub fn alloc_raw(&mut self, size: usize, align: usize) -> *mut u8 {
        self.alloc_impl(size, align, None)
    }

    /// Allocation that does not count toward the heap-balance check and is
    /// reclaimed only when the heap is dropped. Growable-array and string
    /// buffers use this: they may outlive the frame that created them, and
    /// typed reference-counting destructors for them are not implemented yet.
    pub fn alloc_untracked(&mut self, size: usize, align: usize) -> *mut u8 {
        let pointer = self.alloc_impl(size, align, None);
        self.live -= 1;
        if let Some((h, slot, _)) = self.resolve(pointer) {
            unsafe { meta_ref(h, slot) }.fetch_or(UNTRACKED, Ordering::Relaxed);
        }
        pointer
    }

    fn alloc_impl(&mut self, size: usize, align: usize, destroy: Option<DestroyFn>) -> *mut u8 {
        self.collect_remote();
        let class = if align <= 16 && size <= SMALL_LOOKUP_STEPS * 16 {
            self.small_classes[size.div_ceil(16)]
        } else {
            class_for(&self.classes, size, align)
        };
        match class {
            Some(ci) => self.alloc_small(ci, destroy),
            None => self.alloc_large(size, align, destroy),
        }
    }

    fn alloc_small(&mut self, ci: usize, destroy: Option<DestroyFn>) -> *mut u8 {
        let h = match self.partial[ci].last() {
            Some(&h) => h,
            None => self.create_span(ci),
        };
        let hdr = unsafe { &mut *h };
        let slot = hdr.pop().expect("a partial span has a free slot");
        if hdr.free_list_head == SENTINEL {
            self.partial[ci].pop();
        }
        unsafe { meta_ref(h, slot) }.store(INIT, Ordering::Relaxed);
        let pa = hdr.payload_addr(slot);
        crate::valgrind::allocated(pa, hdr.slot_size as usize);
        if let Some(f) = destroy {
            set_destroy(h, slot, f);
        }
        self.live += 1;
        pa as *mut u8
    }

    fn create_span(&mut self, ci: usize) -> *mut SpanHeader {
        let slot_size = self.classes[ci];
        let (data_start, slot_count) = layout_for_class(slot_size);
        let layout = Layout::from_size_align(SPAN_SIZE, SPAN_SIZE).expect("valid span layout");
        let base = unsafe { alloc(layout) };
        let base = ptr::NonNull::new(base).unwrap_or_else(|| handle_alloc_error(layout));

        let h = base.as_ptr().cast::<SpanHeader>();
        unsafe {
            ptr::write(
                h,
                SpanHeader {
                    magic: MAGIC,
                    class_index: u16::try_from(ci).expect("class index fits in u16"),
                    _pad: 0,
                    slot_size,
                    slot_count,
                    free_list_head: SENTINEL,
                    data_start,
                    run_pages: 1,
                    owning_thread_cache: self.id,
                    remote_free: AtomicU32::new(REMOTE_END),
                    owner: self.owner,
                    destroys: 0,
                    next_remote: AtomicUsize::new(0),
                },
            );
            (*h).init_free_list();
        }
        crate::valgrind::no_access(
            base.as_ptr() as usize + data_start as usize,
            slot_count as usize * slot_size as usize,
        );

        self.pages()
            .insert(base.as_ptr() as usize >> 16, h as usize);
        self.backing += layout.size();
        self.spans.push(SpanRecord { base, layout });
        self.partial[ci].push(h);
        h
    }

    fn alloc_large(&mut self, size: usize, align: usize, destroy: Option<DestroyFn>) -> *mut u8 {
        let (data_start, total, run_align) = large_layout(size, align);
        let layout = Layout::from_size_align(total, run_align).expect("valid run layout");
        let base = unsafe { alloc(layout) };
        let base = ptr::NonNull::new(base).unwrap_or_else(|| handle_alloc_error(layout));

        let h = base.as_ptr().cast::<SpanHeader>();
        let run_pages = u32::try_from(total / SPAN_SIZE).expect("run pages fit in u32");
        unsafe {
            ptr::write(
                h,
                SpanHeader {
                    magic: MAGIC,
                    class_index: LARGE_CLASS,
                    _pad: 0,
                    slot_size: u32::try_from(size).expect("large size fits in u32"),
                    slot_count: 1,
                    free_list_head: SENTINEL,
                    data_start: u32::try_from(data_start).expect("offset fits in u32"),
                    run_pages,
                    owning_thread_cache: self.id,
                    remote_free: AtomicU32::new(REMOTE_END),
                    owner: self.owner,
                    destroys: 0,
                    next_remote: AtomicUsize::new(0),
                },
            );
            meta_ref(h, 0).store(INIT, Ordering::Relaxed);
        }

        let first = base.as_ptr() as usize >> 16;
        for i in 0..run_pages as usize {
            self.pages().insert(first + i, h as usize);
        }
        self.backing += layout.size();
        self.large.insert(base.as_ptr() as usize, layout);
        let pa = unsafe { &*h }.payload_addr(0);
        crate::valgrind::allocated(pa, size);
        if let Some(f) = destroy {
            set_destroy(h, 0, f);
        }
        self.live += 1;
        pa as *mut u8
    }

    /// Resolve any pointer inside an allocation to its span, slot and canonical
    /// payload address (section 18.3, step 2).
    fn resolve(&self, ptr: *mut u8) -> Option<(*mut SpanHeader, u32, usize)> {
        let addr = ptr as usize;
        if addr == 0 {
            return None;
        }
        let hv = self.pages().get(addr >> 16)?;
        self.resolve_from_header(ptr, hv)
    }

    fn resolve_from_header(
        &self,
        ptr: *mut u8,
        header: usize,
    ) -> Option<(*mut SpanHeader, u32, usize)> {
        let addr = ptr as usize;
        let h = header as *mut SpanHeader;
        let hdr = unsafe { &*h };
        if hdr.magic != MAGIC {
            return None;
        }
        let payload_start = hdr.base() + hdr.data_start as usize;
        if addr < payload_start {
            return None;
        }
        let off = addr - payload_start;
        let slot = off / hdr.slot_size as usize;
        if slot >= hdr.slot_count as usize {
            return None;
        }
        let pa = payload_start + slot * hdr.slot_size as usize;
        Some((h, u32::try_from(slot).expect("slot fits in u32"), pa))
    }

    /// Reference counting by address; interior pointers resolve to the slot.
    pub fn retain(&mut self, ptr: *mut u8) {
        let Some((h, slot, _)) = self.resolve(ptr) else {
            return;
        };
        let m = unsafe { meta_ref(h, slot) };
        loop {
            let cur = m.load(Ordering::Relaxed);
            if cur & IMMORTAL != 0 {
                return;
            }
            let s = strong(cur);
            if s == 0 {
                return;
            }
            if s == u32::MAX {
                let _ =
                    m.compare_exchange(cur, cur | IMMORTAL, Ordering::Relaxed, Ordering::Relaxed);
                return;
            }
            if m.compare_exchange(cur, cur + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Release one strong reference; run destroy and free at zero (section 18.3).
    pub fn release(&mut self, ptr: *mut u8) {
        let Some((h, slot, pa)) = self.resolve(ptr) else {
            return;
        };
        let m = unsafe { meta_ref(h, slot) };
        let after = loop {
            let cur = m.load(Ordering::Relaxed);
            if cur & IMMORTAL != 0 {
                return;
            }
            let s = strong(cur);
            if s == 0 {
                return;
            }
            let next = (cur & !STRONG_MASK) | u64::from(s - 1);
            // Release, so what this thread wrote to the object happens
            // before the destroy whichever thread drops the last count.
            if m.compare_exchange(cur, next, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                break next;
            }
        };
        if strong(after) == 0 {
            fence(Ordering::Acquire);
            self.cycles.forget(pa);
            self.invalidate_access_cache();
            if after & DELETED == 0 {
                run_destroy(h, slot, pa);
            }
            self.drop_weak_and_maybe_free(h, slot, pa);
        } else if self.owns(h) {
            self.cycles.released(pa);
        }
    }

    /// Drop the collective weak reference; free the slot when weak reaches zero.
    fn drop_weak_and_maybe_free(&mut self, h: *mut SpanHeader, slot: u32, pa: usize) {
        let m = unsafe { meta_ref(h, slot) };
        let after = loop {
            let cur = m.load(Ordering::Relaxed);
            let w = weak(cur);
            if w == 0 {
                return;
            }
            let next = (cur & !WEAK_MASK) | (u64::from(w - 1) << WEAK_SHIFT);
            if m.compare_exchange(cur, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break next;
            }
        };
        if weak(after) == 0 {
            self.free_slot(h, slot, pa);
        }
    }

    fn free_slot(&mut self, h: *mut SpanHeader, slot: u32, pa: usize) {
        if unsafe { &*h }._pad == ARENA_MARK {
            return;
        }
        let state = unsafe { meta_ref(h, slot) }.load(Ordering::Relaxed);
        if !self.owns(h) {
            let _ = Self::free_remote(h, slot, state);
            return;
        }
        self.retire(h, slot, pa, state & UNTRACKED == 0);
    }

    /// Returns a slot of this heap's to its free list.
    fn retire(&mut self, h: *mut SpanHeader, slot: u32, pa: usize, tracked: bool) {
        self.invalidate_access_cache();
        self.cycles.forget(pa);
        clear_destroy(h, slot);
        crate::valgrind::freed(pa);
        if tracked {
            self.live -= 1;
        }
        let (h, slot) = match self.quarantine.as_mut() {
            Some((held, limit)) => {
                held.push_back((h as usize, slot));
                if held.len() <= *limit {
                    return;
                }
                let (oldest, oldest_slot) = held.pop_front().expect("held is not empty");
                (oldest as *mut SpanHeader, oldest_slot)
            }
            None => (h, slot),
        };
        self.reuse_slot(h, slot);
    }

    fn reuse_slot(&mut self, h: *mut SpanHeader, slot: u32) {
        if unsafe { &*h }.is_large() {
            self.free_large(h);
        } else {
            let hdr = unsafe { &mut *h };
            if hdr.free_list_head == SENTINEL {
                self.partial[usize::from(hdr.class_index)].push(h);
            }
            hdr.push(slot);
        }
    }

    fn free_large(&mut self, h: *mut SpanHeader) {
        self.invalidate_access_cache();
        free_destroys(h);
        let hdr = unsafe { &*h };
        let base = hdr.base();
        let pages = hdr.run_pages as usize;
        let first = base >> 16;
        for i in 0..pages {
            self.pages().remove(first + i);
        }
        if let Some(layout) = self.large.remove(&base) {
            self.backing -= layout.size();
            unsafe { dealloc(base as *mut u8, layout) };
        }
    }

    /// Unmanaged free: return the slot regardless of counts. A slot whose
    /// strong count is already zero is free, or waiting on weak references,
    /// so freeing it again would thread it onto the free list twice and hand
    /// it out to two owners; that is refused and reported as `false`, which
    /// callers turn into `KLF-RT-0006`.
    #[must_use]
    pub fn free_raw(&mut self, ptr: *mut u8) -> bool {
        let Some((h, slot, pa)) = self.resolve(ptr) else {
            return true;
        };
        if unsafe { &*h }._pad == ARENA_MARK {
            return true;
        }
        let meta = unsafe { meta_ref(h, slot) }.load(Ordering::Relaxed);
        if meta & IMMORTAL == 0 && strong(meta) == 0 {
            return false;
        }
        if !self.owns(h) {
            return Self::free_remote(h, slot, meta);
        }
        self.free_slot(h, slot, pa);
        true
    }

    /// Raw-mode delete with poison and quarantine (section 18.4).
    pub fn delete_raw(&mut self, ptr: *mut u8, destroy: Option<DestroyFn>) {
        let Some((h, slot, pa)) = self.resolve(ptr) else {
            return;
        };
        self.invalidate_access_cache();
        if let Some(f) = destroy {
            f(pa as *mut u8);
        }
        let m = unsafe { meta_ref(h, slot) };
        m.fetch_or(DELETED, Ordering::Relaxed);
        let after = loop {
            let cur = m.load(Ordering::Relaxed);
            let s = strong(cur);
            if s == 0 {
                return;
            }
            let next = (cur & !STRONG_MASK) | u64::from(s - 1);
            if m.compare_exchange(cur, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                break next;
            }
        };
        if strong(after) == 0 {
            self.cycles.forget(pa);
            self.drop_weak_and_maybe_free(h, slot, pa);
        } else {
            let len = unsafe { &*h }.slot_size as usize;
            unsafe { ptr::write_bytes(pa as *mut u8, 0xEF, len) };
            m.fetch_or(QUARANTINED, Ordering::Relaxed);
        }
    }

    /// Weak load (section 18.5): bump the strong count if the object is alive.
    pub fn weak_load(&mut self, ptr: *mut u8) -> *mut u8 {
        let Some((h, slot, _)) = self.resolve(ptr) else {
            return ptr::null_mut();
        };
        let m = unsafe { meta_ref(h, slot) };
        loop {
            let cur = m.load(Ordering::Relaxed);
            if cur & DELETED != 0 {
                return ptr::null_mut();
            }
            let s = strong(cur);
            if s == 0 {
                return ptr::null_mut();
            }
            if cur & IMMORTAL != 0 {
                return ptr;
            }
            if s == u32::MAX {
                let _ =
                    m.compare_exchange(cur, cur | IMMORTAL, Ordering::Relaxed, Ordering::Relaxed);
                return ptr;
            }
            if m.compare_exchange(cur, cur + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return ptr;
            }
        }
    }

    /// True when the strong count is exactly 1 (copy-on-write, `ref.isunique`).
    #[must_use]
    pub fn is_unique(&self, ptr: *mut u8) -> bool {
        match self.resolve(ptr) {
            Some((h, slot, _)) => {
                let m = unsafe { meta_ref(h, slot) };
                strong(m.load(Ordering::Relaxed)) == 1
            }
            None => false,
        }
    }

    /// Is this address inside the KLF heap (page map lookup)?
    #[must_use]
    pub fn contains(&self, ptr: *const u8) -> bool {
        self.pages().get(ptr as usize >> 16).is_some()
    }

    /// Number of live blocks not yet freed (KLF-RT-0100 heap-balance check).
    #[must_use]
    pub fn live_blocks(&self) -> usize {
        self.live
    }

}

// SAFETY: a heap is used by one thread at a time; one moves between threads
// only whole, as an orphaned heap does into its `Owner`.
unsafe impl Send for Heap {}

impl Drop for Heap {
    fn drop(&mut self) {
        for rec in &self.spans {
            free_destroys(rec.base.as_ptr().cast());
            unsafe { dealloc(rec.base.as_ptr(), rec.layout) };
        }
        for (&base, &layout) in &self.large {
            free_destroys(base as *mut SpanHeader);
            unsafe { dealloc(base as *mut u8, layout) };
        }
        if self.own_pages {
            // SAFETY: made by `Heap::new` from a box and used only by this
            // heap.
            drop(unsafe { Box::from_raw(self.pages.cast_mut()) });
        }
    }
}
