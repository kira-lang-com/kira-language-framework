//! Dynamic exclusivity checks in the VM (section 11.8).
//!
//! `access.begin [read or modify] [static or dynamic or unchecked] %addr`
//! brackets a memory access. Under `static` the verifier has already proved no
//! two overlapping accesses conflict, and under `unchecked` nothing is
//! enforced, so both emit nothing. Only `dynamic` emits the claim and its
//! release, exactly as `klfrt_access_begin` and `klfrt_access_end` do on the
//! native backends, so a program behaves the same on either.

use super::{LowerError, Lowerer};
use crate::bytecode::{Instr, Opcode};
use klf_kir::{AccessEnforcement, AccessMode, Operation};

impl Lowerer<'_> {
    pub(super) fn lower_access(&mut self, operation: &Operation) -> Option<Result<(), LowerError>> {
        Some(match operation {
            Operation::AccessBegin {
                mode,
                enforcement,
                address,
            } => {
                if *enforcement != AccessEnforcement::Dynamic {
                    return Some(Ok(()));
                }
                let register = match self.register(address) {
                    Ok(register) => register,
                    Err(error) => return Some(Err(error)),
                };
                let size = u16::try_from(self.access_size(address)).unwrap_or(1);
                let mode: u16 = match mode {
                    AccessMode::Read => 0,
                    AccessMode::Modify => 1,
                };
                self.emit(Instr::abc(Opcode::AccBegin, size, register, mode));
                Ok(())
            }
            Operation::AccessEnd(address) => {
                let register = match self.register(address) {
                    Ok(register) => register,
                    Err(error) => return Some(Err(error)),
                };
                self.emit(Instr::abc(Opcode::AccEnd, 0, register, 0));
                Ok(())
            }
            _ => return None,
        })
    }

    /// The byte range one dynamic access claims: the storage's own layout, or
    /// one word for anything that is not a sized record.
    fn access_size(&self, address: &klf_core::Name) -> u64 {
        self.types
            .get(address)
            .map_or(1, |ty| klf_layout::layout_of(ty, self.decls).size.max(1))
    }
}
