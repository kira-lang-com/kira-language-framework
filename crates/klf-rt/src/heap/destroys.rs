//! Each span's table of the destroy functions its slots were allocated with
//! (section 18.3), made the first time a slot of the span takes one. A slot's
//! entry lives with its span, so any thread releasing the last count finds it.

use super::DestroyFn;
use crate::span::{MAGIC, SpanHeader};
use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};

/// Records slot `slot`'s destroy function in its span's table, making the
/// table the first time.
pub(super) fn set_destroy(h: *mut SpanHeader, slot: u32, f: DestroyFn) {
    let hdr = unsafe { &mut *h };
    if hdr.destroys == 0 {
        let layout = destroys_layout(hdr);
        let table = unsafe { alloc_zeroed(layout) };
        if table.is_null() {
            handle_alloc_error(layout);
        }
        hdr.destroys = table as usize;
    }
    unsafe { *(hdr.destroys as *mut usize).add(slot as usize) = f as usize };
}

/// Runs slot `slot`'s destroy function, if it was allocated with one.
pub(super) fn run_destroy(h: *mut SpanHeader, slot: u32, pa: usize) {
    let table = unsafe { &*h }.destroys;
    if table == 0 {
        return;
    }
    let f = unsafe { *(table as *const usize).add(slot as usize) };
    if f != 0 {
        // SAFETY: only `set_destroy` writes the table, with a `DestroyFn`.
        let f: DestroyFn = unsafe { core::mem::transmute::<usize, DestroyFn>(f) };
        f(pa as *mut u8);
    }
}

pub(super) fn clear_destroy(h: *mut SpanHeader, slot: u32) {
    let table = unsafe { &*h }.destroys;
    if table != 0 {
        unsafe { *(table as *mut usize).add(slot as usize) = 0 };
    }
}

pub(super) fn free_destroys(h: *mut SpanHeader) {
    let hdr = unsafe { &mut *h };
    if hdr.magic == MAGIC && hdr.destroys != 0 {
        unsafe { dealloc(hdr.destroys as *mut u8, destroys_layout(hdr)) };
        hdr.destroys = 0;
    }
}

fn destroys_layout(hdr: &SpanHeader) -> Layout {
    Layout::array::<usize>(hdr.slot_count.max(1) as usize).expect("destroy table layout")
}
