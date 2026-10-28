//! Size classes and per-span geometry (section 18.1).
//!
//! Classes are multiples of 16 up to 256, then eight classes per power of two up
//! to 16,384. Anything larger gets a dedicated run of spans with one slot.

use crate::span::{HEADER_SIZE, META_BYTES, SPAN_SIZE};

/// Round `x` up to a multiple of `align` (a power of two).
pub(crate) const fn align_up(x: usize, align: usize) -> usize {
    (x + align - 1) & !(align - 1)
}

/// Build the ordered table of slot sizes.
pub(crate) fn build_classes() -> Vec<u32> {
    let mut v: Vec<u32> = Vec::new();

    let mut s = 16u32;
    while s <= 256 {
        v.push(s);
        s += 16;
    }

    let mut base = 256u32;
    while base < 16_384 {
        let step = base / 8;
        let mut x = base + step;
        while x <= base * 2 {
            v.push(x);
            x += step;
        }
        base *= 2;
    }

    v
}

/// The natural alignment a span for `slot_size` provides: the largest power of
/// two dividing the slot size, at least 16. Every slot start is a multiple of it.
pub(crate) fn class_align(slot_size: u32) -> usize {
    let a = 1usize << slot_size.trailing_zeros();
    a.max(16)
}

/// Pick the class index for a request, or `None` for a large allocation.
///
/// Alignments above 16 need a class whose slot size is a multiple of the
/// alignment; because slot sizes are multiples of 16, alignments up to 16 fit
/// every class.
pub(crate) fn class_for(classes: &[u32], size: usize, align: usize) -> Option<usize> {
    let need_align = align.max(16);
    classes.iter().position(|&ss| {
        let ss = ss as usize;
        ss >= size && ss.is_multiple_of(need_align)
    })
}

/// Slots that fit in one span for a fixed class, and the payload start offset.
pub(crate) fn layout_for_class(slot_size: u32) -> (u32, u32) {
    let a = class_align(slot_size);
    let ss = slot_size as usize;

    let mut n = (SPAN_SIZE - HEADER_SIZE) / (META_BYTES + ss);
    loop {
        let data_start = align_up(HEADER_SIZE + META_BYTES * n, a);
        if n >= 1 && data_start + n * ss <= SPAN_SIZE {
            return (
                u32::try_from(data_start).expect("offset fits in u32"),
                u32::try_from(n).expect("slot count fits in u32"),
            );
        }
        n -= 1;
    }
}

/// Geometry for a large run: payload offset, total bytes, and run alignment.
pub(crate) fn large_layout(size: usize, align: usize) -> (usize, usize, usize) {
    let a = align.max(16);
    let data_start = align_up(HEADER_SIZE + META_BYTES, a);
    let total = align_up(data_start + size, SPAN_SIZE);
    let run_align = SPAN_SIZE.max(align);
    (data_start, total, run_align)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_matches_spec() {
        let c = build_classes();
        let idx = |sz, al| class_for(&c, sz, al).map(|i| c[i]);
        assert_eq!(idx(16, 1), Some(16));
        assert_eq!(idx(17, 1), Some(32));
        assert_eq!(idx(256, 1), Some(256));
        assert_eq!(idx(257, 1), Some(288));
        assert_eq!(idx(4096, 1), Some(4096));
        assert_eq!(idx(20_000, 1), None); // large
    }

    #[test]
    fn alignment_selects_multiple_class() {
        let c = build_classes();
        let ss = c[class_for(&c, 20, 32).unwrap()];
        assert!(ss >= 32 && ss.is_multiple_of(32));
        assert!(class_align(ss) >= 32);
    }

    #[test]
    fn class_geometry_fits_the_span() {
        for &ss in &build_classes() {
            let (ds, n) = layout_for_class(ss);
            let end = ds as usize + n as usize * ss as usize;
            assert!(end <= SPAN_SIZE, "class {ss} overruns span");
            assert!(n >= 1, "class {ss} must hold at least one slot");
            assert_eq!(ds as usize % class_align(ss), 0);
        }
    }
}
