//! Slot metadata layout (section 18.2). Eight bytes per slot, held as a `u64`.
//!
//! | Bits    | Field                                              |
//! | ------- | -------------------------------------------------- |
//! | 0 - 31  | strong count, starts at 1                           |
//! | 32 - 55 | weak count, starts at 1 (held collectively)         |
//! | 56      | `deleted`: a raw-mode delete ran the destructor     |
//! | 57      | `quarantined`                                       |
//! | 58      | `immortal`: counts are frozen                        |
//! | 59      | `untracked`: outside the heap-balance check          |
//! | 60      | `collected`: freed by a collection that cannot reach it |
//! | 61      | `marked`: reached by the collection under way         |
//! | 62 - 63 | reserved, zero                                       |

pub(crate) const STRONG_MASK: u64 = 0xFFFF_FFFF;
pub(crate) const WEAK_SHIFT: u32 = 32;
pub(crate) const WEAK_ONE: u64 = 1 << WEAK_SHIFT;
pub(crate) const WEAK_MASK: u64 = 0x00FF_FFFF << WEAK_SHIFT;

pub(crate) const DELETED: u64 = 1 << 56;
pub(crate) const QUARANTINED: u64 = 1 << 57;
pub(crate) const IMMORTAL: u64 = 1 << 58;
pub(crate) const UNTRACKED: u64 = 1 << 59;

/// A freshly allocated slot: strong 1, weak 1, no flags.
pub(crate) const INIT: u64 = 1 | WEAK_ONE;

pub(crate) fn strong(m: u64) -> u32 {
    (m & STRONG_MASK) as u32
}

pub(crate) fn weak(m: u64) -> u32 {
    ((m & WEAK_MASK) >> WEAK_SHIFT) as u32
}
