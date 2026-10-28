//! Behavioural tests for the heap. These are the coverage the spec calls for in
//! section 18 plus the required cases from the task.

use crate::Heap;
use core::sync::atomic::{AtomicUsize, Ordering};

static DESTROY_CALLS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn counting_destroy(_p: *mut u8) {
    DESTROY_CALLS.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn alloc_counted_then_release_frees() {
    let mut h = Heap::new();
    assert_eq!(h.live_blocks(), 0);
    let p = h.alloc_counted(64, 1, None);
    assert_eq!(h.live_blocks(), 1);
    h.release(p);
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn retain_keeps_alive_until_second_release() {
    let mut h = Heap::new();
    let p = h.alloc_counted(48, 1, None);
    h.retain(p);
    assert_eq!(h.live_blocks(), 1);
    h.release(p);
    assert_eq!(h.live_blocks(), 1, "still one strong reference");
    h.release(p);
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn interior_pointer_resolves() {
    let mut h = Heap::new();
    let p = h.alloc_counted(128, 1, None);
    let interior = unsafe { p.add(40) };
    h.retain(interior);
    assert!(!h.is_unique(interior));
    assert!(!h.is_unique(p));
    h.release(interior);
    assert!(h.is_unique(p));
    h.release(p);
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn delete_raw_quarantines_then_frees() {
    let mut h = Heap::new();
    let p = h.alloc_counted(64, 1, None);
    h.retain(p); // strong = 2
    h.delete_raw(p, None); // strong = 1, poisoned + quarantined, still live
    assert_eq!(h.live_blocks(), 1, "quarantined block stays live");
    let payload = unsafe { core::slice::from_raw_parts(p, 64) };
    assert!(
        payload.iter().all(|&b| b == 0xEF),
        "payload must be poisoned"
    );
    h.release(p); // strong = 0 -> freed
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn cached_access_does_not_bypass_delete_quarantine() {
    let mut heap = Heap::new();
    let pointer = heap.alloc_counted(32, 1, None);
    assert!(heap.write(pointer, 8, 0xA5));
    heap.retain(pointer);
    heap.delete_raw(pointer, None);
    assert_eq!(heap.read(pointer, 8), None);
    heap.release(pointer);
}

#[test]
fn weak_load_tracks_liveness() {
    let mut h = Heap::new();
    let p = h.alloc_counted(32, 1, None);
    let loaded = h.weak_load(p); // bumps strong to 2
    assert_eq!(loaded, p);
    h.release(p); // back to 1
    let again = h.weak_load(p);
    assert_eq!(again, p);
    h.release(p); // back to 1
    h.release(p); // strong 0, freed
    assert_eq!(h.live_blocks(), 0);
    assert!(h.weak_load(p).is_null(), "weak_load must be null once dead");
}

#[test]
fn is_unique_reflects_strong_count() {
    let mut h = Heap::new();
    let p = h.alloc_counted(16, 1, None);
    assert!(h.is_unique(p));
    h.retain(p);
    assert!(!h.is_unique(p));
    h.release(p);
    assert!(h.is_unique(p));
    h.release(p);
}

#[test]
fn destroy_runs_exactly_once_at_zero() {
    DESTROY_CALLS.store(0, Ordering::SeqCst);
    let mut h = Heap::new();
    let p = h.alloc_counted(64, 1, Some(counting_destroy));
    h.retain(p);
    h.release(p);
    assert_eq!(DESTROY_CALLS.load(Ordering::SeqCst), 0, "not yet at zero");
    h.release(p);
    assert_eq!(DESTROY_CALLS.load(Ordering::SeqCst), 1, "destroy ran once");
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn contains_true_for_heap_false_for_stack() {
    let mut h = Heap::new();
    let p = h.alloc_counted(64, 1, None);
    assert!(h.contains(p));
    assert!(h.contains(unsafe { p.add(10) }));
    let on_stack = 0u64;
    assert!(!h.contains(core::ptr::from_ref(&on_stack).cast::<u8>()));
    h.release(p);
}

#[test]
fn distinct_allocations_do_not_overlap() {
    let mut h = Heap::new();
    let a = h.alloc_counted(64, 1, None);
    let b = h.alloc_counted(64, 1, None);
    assert_ne!(a, b);
    unsafe {
        core::ptr::write_bytes(a, 0xAA, 64);
        core::ptr::write_bytes(b, 0xBB, 64);
        assert!((0..64).all(|i| *a.add(i) == 0xAA));
        assert!((0..64).all(|i| *b.add(i) == 0xBB));
    }
    h.release(a);
    h.release(b);
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn the_exit_report_counts_what_was_never_released() {
    let mut heap = Heap::new();
    let buffer = heap.alloc_untracked(4096, 8);
    let owner = heap.alloc_counted(16, 8, None);
    let freed = heap.alloc_untracked(24, 8);
    heap.release(freed);
    let report = heap.exit_report();
    assert_eq!((report.tracked, report.untracked), (1, 1));
    assert!(report.untracked_bytes >= 4096);
    heap.release(owner);
    heap.release(buffer);
    assert_eq!(heap.exit_report(), crate::ExitReport::default());
}

#[test]
fn large_allocation_round_trip() {
    let mut h = Heap::new();
    let p = h.alloc_counted(20_000, 1, None);
    assert!(h.contains(p));
    assert!(h.contains(unsafe { p.add(19_999) }));
    unsafe { core::ptr::write_bytes(p, 0xCD, 20_000) };
    assert!((0..20_000).all(|i| unsafe { *p.add(i) } == 0xCD));
    h.release(p);
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn aligned_allocation_is_aligned() {
    let mut h = Heap::new();
    for &align in &[32usize, 64, 128, 256] {
        let p = h.alloc_counted(40, align, None);
        assert_eq!(p as usize % align, 0, "alignment {align} not satisfied");
        h.release(p);
    }
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn null_and_foreign_pointers_are_ignored() {
    let mut h = Heap::new();
    h.retain(core::ptr::null_mut());
    h.release(core::ptr::null_mut());
    let stack = 0u64;
    let sp = core::ptr::from_ref(&stack).cast::<u8>().cast_mut();
    h.retain(sp);
    h.release(sp);
    assert!(!h.is_unique(sp));
    assert!(h.weak_load(sp).is_null());
}

#[test]
fn heap_bulk_buffers_are_bounded_to_live_allocations() {
    let mut heap = Heap::new();
    let pointer = heap.alloc_counted(16, 1, None);
    assert!(heap.write_buffer(pointer, b"KLF!"));

    let mut actual = [0; 4];
    assert!(heap.read_buffer(pointer, &mut actual));
    assert_eq!(&actual, b"KLF!");
    assert!(!heap.read_buffer(unsafe { pointer.add(14) }, &mut actual));

    heap.release(pointer);
    assert!(!heap.read_buffer(pointer, &mut actual));
}

#[test]
fn heap_copies_and_fills_whole_checked_ranges() {
    let mut heap = Heap::new();
    let pointer = heap.alloc_counted(16, 1, None);
    let other = heap.alloc_counted(16, 1, None);
    assert!(heap.write_buffer(pointer, b"abcdef"));
    assert!(heap.copy(unsafe { pointer.add(2) }, pointer, 4));
    let mut actual = [0; 6];
    assert!(heap.read_buffer(pointer, &mut actual));
    assert_eq!(&actual, b"ababcd");

    assert!(heap.copy(other, pointer, 6));
    assert!(heap.fill(other, b'z', 2));
    assert!(heap.read_buffer(other, &mut actual));
    assert_eq!(&actual, b"zzabcd");

    assert!(!heap.copy(unsafe { other.add(12) }, pointer, 6));
    assert!(!heap.fill(unsafe { other.add(12) }, b'y', 6));
    assert!(heap.read_buffer(other, &mut actual));
    assert_eq!(&actual, b"zzabcd");

    heap.release(pointer);
    assert!(!heap.copy(other, pointer, 1));
}

#[test]
fn heap_scalar_accesses_preserve_little_endian_widths() {
    let mut heap = Heap::new();
    let pointer = heap.alloc_counted(16, 1, None);
    let value = 0x8877_6655_4433_2211;
    for bytes in 0..=8 {
        assert!(heap.write(pointer, bytes, value));
        let mask = if bytes == 8 {
            u64::MAX
        } else {
            (1u64 << (bytes * 8)) - 1
        };
        assert_eq!(heap.read(pointer, bytes), Some(value & mask));
    }
    assert!(!heap.write(pointer, 9, value));
    assert_eq!(heap.read(pointer, 9), None);
    assert!(!heap.write(unsafe { pointer.add(15) }, 2, value));
    assert_eq!(heap.read(unsafe { pointer.add(15) }, 2), None);
    heap.release(pointer);
}

#[test]
fn native_memory_cache_preserves_heap_liveness_checks() {
    let mut heap = Heap::new();
    heap.admit_native_memory();
    let mut native = [0u8; 8];
    let native_ptr = native.as_mut_ptr();
    assert!(heap.write(native_ptr, 4, 0x4433_2211));
    assert_eq!(heap.read(native_ptr, 4), Some(0x4433_2211));

    let pointer = heap.alloc_counted(16, 1, None);
    assert!(heap.write(pointer, 8, 0x8877_6655_4433_2211));
    assert_eq!(heap.read(pointer, 8), Some(0x8877_6655_4433_2211));
    heap.release(pointer);
    assert_eq!(heap.read(pointer, 8), None);
}

#[test]
fn raw_frame_arena_reuses_aligned_space_after_release() {
    let mut heap = Heap::new();
    let mark = heap.arena_mark();
    let first = heap.arena_alloc_raw(24, 16).unwrap();
    let second = heap.arena_alloc_raw(24, 16).unwrap();
    assert_eq!(first as usize % 16, 0);
    assert_ne!(first, second);

    heap.arena_release(mark);
    assert_eq!(heap.arena_alloc_raw(24, 16), Some(first));
}

#[test]
fn slot_reuse_after_free() {
    let mut h = Heap::new();
    let a = h.alloc_counted(64, 1, None);
    h.release(a);
    let b = h.alloc_counted(64, 1, None);
    assert_eq!(a, b, "the freed slot should be reused");
    h.release(b);
    assert_eq!(h.live_blocks(), 0);
}

#[test]
fn alternating_access_cache_preserves_bounds_and_disposal() {
    let mut heap = Heap::new();
    let first = heap.alloc_counted(16, 1, None);
    let second = heap.alloc_counted(16, 1, None);
    assert!(heap.write(first, 8, 17));
    assert!(heap.write(second, 8, 29));
    for _ in 0..4 {
        assert_eq!(heap.read(first, 8), Some(17));
        assert_eq!(heap.read(second, 8), Some(29));
    }
    assert_eq!(heap.read(first.wrapping_add(15), 2), None);
    assert_eq!(heap.read(second.wrapping_add(15), 2), None);
    heap.release(first);
    assert_eq!(heap.read(first, 8), None);
    assert_eq!(heap.read(second, 8), Some(29));
    heap.release(second);
    assert_eq!(heap.read(second, 8), None);
    let raw = heap.alloc_raw(16, 16);
    assert!(heap.write(raw, 8, 37));
    let other = heap.alloc_raw(16, 16);
    assert!(heap.write(other, 8, 41));
    assert!(heap.free_raw(raw));
    assert_eq!(heap.read(raw, 8), None);
    assert_eq!(heap.read(other, 8), Some(41));
    assert!(heap.free_raw(other));
    assert_eq!(heap.live_blocks(), 0);
    assert!(!heap.free_raw(other), "a second free of the same slot is refused");
    let next = heap.alloc_raw(16, 16);
    let after = heap.alloc_raw(16, 16);
    assert_ne!(next, after, "a refused free leaves the free list whole");
}
