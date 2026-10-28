//! Checked reads and writes: whether an address lies in a live slot, with the
//! last two slots reached remembered so a run of accesses to one value skips
//! the page map.

use super::Heap;
use crate::meta::{DELETED, QUARANTINED, strong};
use crate::service_memory::ServiceMemory;
use crate::span::meta_ref;
use core::ptr;
use core::sync::atomic::Ordering;

impl Heap {
    /// Bounds check: the complete range at `ptr` stays inside one live slot.
    #[inline(always)]
    pub(super) fn access_ok(&self, ptr: *mut u8, bytes: usize) -> Option<usize> {
        let addr = ptr as usize;
        let end = addr.checked_add(bytes)?;
        let (start, stop) = self.last_access.get();
        if addr >= start && addr < stop {
            return (end <= stop).then_some(addr);
        }
        let (start, stop) = self.previous_access.get();
        if addr >= start && addr < stop {
            return (end <= stop).then_some(addr);
        }
        self.access_slow(ptr, end)
    }

    pub(super) fn invalidate_access_cache(&self) {
        self.last_access.set((0, 0));
        self.previous_access.set((0, 0));
    }

    #[inline(never)]
    fn access_slow(&self, ptr: *mut u8, end: usize) -> Option<usize> {
        let addr = ptr as usize;
        let (slot_start, slot_end, meta) = {
            let page = addr >> 16;
            let generation = self.pages().generation();
            if self.native_memory && addr != 0 && self.native_page.get() == (page, generation) {
                return Some(addr);
            }
            let Some(header) = self.pages().get(page) else {
                if self.native_memory && addr != 0 {
                    self.native_page.set((page, generation));
                    return Some(addr);
                }
                return None;
            };
            let (h, slot, pa) = self.resolve_from_header(ptr, header)?;
            let slot_end = pa.checked_add(unsafe { &*h }.slot_size as usize)?;
            let meta = ptr::from_ref(unsafe { meta_ref(h, slot) }) as usize;
            (pa, slot_end, meta)
        };
        let state =
            unsafe { &*(meta as *const core::sync::atomic::AtomicU64) }.load(Ordering::Relaxed);
        if strong(state) == 0 || state & (DELETED | QUARANTINED) != 0 {
            return None;
        }
        self.previous_access.set(self.last_access.get());
        self.last_access.set((slot_start, slot_end));
        (end <= slot_end).then_some(addr)
    }
}

impl ServiceMemory for Heap {
    fn read_buffer(&self, address: u64, destination: &mut [u8]) -> bool {
        let Ok(address) = usize::try_from(address) else {
            return false;
        };
        Heap::read_buffer(self, address as *const u8, destination)
    }

    fn write_buffer(&mut self, address: u64, source: &[u8]) -> bool {
        let Ok(address) = usize::try_from(address) else {
            return false;
        };
        Heap::write_buffer(self, address as *mut u8, source)
    }
}
