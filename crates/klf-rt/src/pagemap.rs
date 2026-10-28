//! The global page map (section 18.1): a two-level radix tree keyed by the
//! address shifted right by 16, so one entry per 64 KiB span page.
//!
//! Each entry holds the address of the owning span header, or 0 for empty. A
//! large run registers every one of its pages against the same header, so an
//! interior pointer anywhere in the run resolves to the allocation.
//!
//! The key is split into a high part naming a leaf and a low `L2_BITS` part
//! indexing it. A process maps its heap into a handful of 4 GiB windows, so
//! the root is a short list of the leaves in use, searched in order: indexing
//! a root by the high part would size it by the highest window, which on a
//! 64-bit target sits near the top of the address space. On a 32-bit target
//! (wasm) the key is 16 bits, the high part is empty, and the single leaf is
//! the flat bitmap the spec describes.

use core::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

const L2_BITS: u32 = 16;
const LEAF_LEN: usize = 1 << L2_BITS;
const L2_MASK: usize = LEAF_LEN - 1;

/// How many 4 GiB windows the map can name. A process maps its heap into a
/// handful; running out is a host allocator spreading spans across the whole
/// address space, which the heap reports as an allocation failure.
const WINDOWS: usize = 64;

/// The map is shared by every thread's heap in a native program (section
/// 18.1: one global page map), so a pointer one thread allocated resolves on
/// any other. Lookups take no lock: a leaf is published by a release store
/// after it is zeroed, and an entry by a release store after the header it
/// names is written. Growing the root takes `grow`, which only span creation
/// does.
pub(crate) struct PageMap {
    windows: [AtomicUsize; WINDOWS],
    leaves: [AtomicPtr<AtomicUsize>; WINDOWS],
    used: AtomicUsize,
    grow: Mutex<()>,
    generation: AtomicU64,
}

impl PageMap {
    pub(crate) const fn new() -> Self {
        Self {
            windows: [const { AtomicUsize::new(0) }; WINDOWS],
            leaves: [const { AtomicPtr::new(core::ptr::null_mut()) }; WINDOWS],
            used: AtomicUsize::new(0),
            grow: Mutex::new(()),
            generation: AtomicU64::new(0),
        }
    }

    #[inline]
    fn leaf(&self, hi: usize) -> Option<&[AtomicUsize]> {
        let used = self.used.load(Ordering::Acquire);
        for at in 0..used {
            if self.windows[at].load(Ordering::Relaxed) == hi {
                let leaf = self.leaves[at].load(Ordering::Acquire);
                // SAFETY: a published leaf is `LEAF_LEN` entries that live
                // as long as the map.
                return Some(unsafe { core::slice::from_raw_parts(leaf, LEAF_LEN) });
            }
        }
        None
    }

    fn split(key: usize) -> (usize, usize) {
        (key >> L2_BITS, key & L2_MASK)
    }

    /// The header address registered for `key`, if any.
    #[inline]
    pub(crate) fn get(&self, key: usize) -> Option<usize> {
        let (hi, lo) = Self::split(key);
        let v = self.leaf(hi)?[lo].load(Ordering::Acquire);
        (v != 0).then_some(v)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Register `header` as the owner of `key`.
    pub(crate) fn insert(&self, key: usize, header: usize) {
        let (hi, lo) = Self::split(key);
        let leaf = match self.leaf(hi) {
            Some(leaf) => leaf,
            None => self.add_window(hi),
        };
        leaf[lo].store(header, Ordering::Release);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// The leaf of window `hi`, made under the lock unless another thread
    /// made it first.
    #[cold]
    fn add_window(&self, hi: usize) -> &[AtomicUsize] {
        let _held = self.grow.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(leaf) = self.leaf(hi) {
            return leaf;
        }
        let at = self.used.load(Ordering::Relaxed);
        assert!(at < WINDOWS, "the heap spans more address windows than the page map names");
        let leaf: Box<[AtomicUsize]> = (0..LEAF_LEN).map(|_| AtomicUsize::new(0)).collect();
        let leaf = Box::into_raw(leaf).cast::<AtomicUsize>();
        self.windows[at].store(hi, Ordering::Relaxed);
        self.leaves[at].store(leaf, Ordering::Release);
        self.used.store(at + 1, Ordering::Release);
        // SAFETY: just published, `LEAF_LEN` entries.
        unsafe { core::slice::from_raw_parts(leaf, LEAF_LEN) }
    }

    /// Clear the owner of `key`.
    pub(crate) fn remove(&self, key: usize) {
        let (hi, lo) = Self::split(key);
        if let Some(leaf) = self.leaf(hi) {
            leaf[lo].store(0, Ordering::Release);
            self.generation.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for PageMap {
    fn drop(&mut self) {
        for at in 0..*self.used.get_mut() {
            let leaf = *self.leaves[at].get_mut();
            // SAFETY: made by `add_window` from a boxed slice of `LEAF_LEN`.
            drop(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(leaf, LEAF_LEN)) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove() {
        let m = PageMap::new();
        assert_eq!(m.get(42), None);
        m.insert(42, 0xDEAD_BEEF);
        assert_eq!(m.get(42), Some(0xDEAD_BEEF));
        m.remove(42);
        assert_eq!(m.get(42), None);
    }

    #[test]
    fn distinct_leaves_do_not_alias() {
        let m = PageMap::new();
        let a = 1usize;
        let b = (3usize << L2_BITS) + 7;
        m.insert(a, 100);
        m.insert(b, 200);
        assert_eq!(m.get(a), Some(100));
        assert_eq!(m.get(b), Some(200));
        assert_eq!(m.get(0), None);
    }

    #[test]
    fn mutations_advance_generation() {
        let map = PageMap::new();
        let initial = map.generation();
        map.insert(42, 0xDEAD_BEEF);
        let inserted = map.generation();
        map.remove(42);
        assert!(inserted > initial);
        assert!(map.generation() > inserted);
    }
}
