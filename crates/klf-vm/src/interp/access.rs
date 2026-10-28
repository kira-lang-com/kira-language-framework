//! Dynamic exclusivity checks in the VM (section 11.8).
//!
//! `access.begin [read or modify] [static or dynamic or unchecked] %addr`
//! brackets a memory access. Under `static` the verifier has already proved no
//! two overlapping accesses conflict, and under `unchecked` nothing is
//! enforced, so both emit nothing. Only `dynamic` records a claim and
//! conflicts trap with `KLF-RT-0003`, exactly as `klfrt_access_begin` does on
//! the native backends, so a program behaves the same on either.

use std::collections::HashMap;

/// The claims one frame holds: the first address of the storage it covers, the
/// end of that range, and whether the access may modify it.
#[derive(Default)]
pub(crate) struct Accesses {
    holds: HashMap<usize, (usize, bool)>,
}

impl Accesses {
    /// Claim `[address, address + size)` as a `read` (`modify` false) or a
    /// `modify` (`modify` true). Answers whether it conflicts with a claim the
    /// frame already holds; a conflicting claim is not recorded.
    pub(crate) fn begin(&mut self, address: u64, size: u64, modify: bool) -> bool {
        let start = address as usize;
        let end = start.saturating_add(usize::try_from(size).unwrap_or(1).max(1));
        let conflict = self.holds.iter().any(|(held, (held_end, held_modify))| {
            *held < end && start < *held_end && (modify || *held_modify)
        });
        if !conflict {
            self.holds.insert(start, (end, modify));
        }
        conflict
    }

    /// Release the claim `begin` took for `address`.
    pub(crate) fn end(&mut self, address: u64) {
        self.holds.remove(&(address as usize));
    }
}

#[cfg(test)]
mod tests {
    use super::Accesses;

    #[test]
    fn two_reads_overlapping_do_not_conflict_and_two_modifies_do() {
        let mut accesses = Accesses::default();
        assert!(!accesses.begin(100, 8, false));
        assert!(!accesses.begin(104, 8, false));
        accesses.end(100);
        accesses.end(104);

        assert!(!accesses.begin(100, 8, true));
        assert!(accesses.begin(104, 8, false), "a read conflicts a modify");
    }
}
