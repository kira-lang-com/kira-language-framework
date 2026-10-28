//! Ownership on storage rather than in registers (section 11.6 pass 5 and
//! section 12.8).
//!
//! `copy.addr %src to [init or assign] %dst` copies the value whose storage is
//! at `%src` into the storage at `%dst`, through the type's policy.
//! `move.addr` relocates it, leaving `%src` uninitialized. `destroy.addr %a`
//! destroys the value in place. `assign` destroys what `%dst` held first.
//!
//! A register holds an address as readily as a value, so the payload move is
//! the bulk copy an address pair stands for, and the policy is the retain,
//! release or free that the value form applies to a register.

use super::{LowerError, Lowerer};
use crate::bytecode::{Instr, Opcode};
use klf_core::Name;
use klf_kir::{Operation, StoreMode, Type};

impl Lowerer<'_> {
    pub(super) fn lower_ownership(
        &mut self,
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        if !matches!(
            operation,
            Operation::CopyAddr { .. } | Operation::MoveAddr { .. } | Operation::DestroyAddr(_)
        ) {
            return None;
        }
        Some(self.ownership(operation))
    }

    fn ownership(&mut self, operation: &Operation) -> Result<(), LowerError> {
        match operation {
            Operation::CopyAddr { source, dest, mode } => {
                self.assign_first(dest, *mode)?;
                self.move_payload(source, dest)?;
                // A copy leaves a second owner, so it retains like `copy` does.
                if matches!(self.pointee_of(source), Some(Type::Reference(_))) {
                    let held = self.load_pointer(source)?;
                    self.emit(Instr::abc(Opcode::Retain, held, 0, 0));
                }
                Ok(())
            }
            Operation::MoveAddr { source, dest, mode } => {
                self.assign_first(dest, *mode)?;
                // A relocation moves the one value there is, so nothing is retained.
                self.move_payload(source, dest)
            }
            Operation::DestroyAddr(address) => {
                let opcode = match self.pointee_of(address) {
                    Some(Type::Reference(_)) => Some(Opcode::Release),
                    Some(Type::Unique(_)) => Some(Opcode::HfreeR),
                    _ => None,
                };
                if let Some(opcode) = opcode {
                    let held = self.load_pointer(address)?;
                    self.emit(Instr::abc(opcode, held, 0, 0));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// `assign` gives the destination an old value, which has to go first.
    fn assign_first(&mut self, dest: &Name, mode: StoreMode) -> Result<(), LowerError> {
        if mode != StoreMode::Assign {
            return Ok(());
        }
        self.ownership(&Operation::DestroyAddr(dest.clone()))
    }

    /// Copy the storage `%source` names onto the storage `%dest` names. The
    /// length is the storage's own layout, so a pointer-sized owner moves one
    /// word and an aggregate moves whole.
    fn move_payload(&mut self, source: &Name, dest: &Name) -> Result<(), LowerError> {
        let size = self.storage_bytes(source);
        let (source, dest) = (self.register(source)?, self.register(dest)?);
        let length = self.fresh()?;
        self.emit(Instr::from_bx(
            Opcode::LoadI,
            length,
            i32::try_from(size).unwrap_or(i32::MAX),
        ));
        self.emit(Instr::abc(Opcode::MemCpy, dest, source, length));
        Ok(())
    }

    /// Read the pointer a `Reference` or `Unique` slot holds, for the policy to
    /// act on.
    fn load_pointer(&mut self, address: &Name) -> Result<u16, LowerError> {
        let slot = self.register(address)?;
        let held = self.fresh()?;
        let width = super::scalar_width(&Type::Pointer)?;
        self.emit(Instr::abc(Opcode::Load, held, slot, 0).with_flags(width.code()));
        Ok(held)
    }

    /// The bytes of the storage an address names: its pointee's layout, so
    /// an aggregate moves whole, or one word for a name with no known pointee.
    fn storage_bytes(&self, address: &Name) -> u64 {
        let Some(ty) = self.pointee_of(address) else {
            return 8;
        };
        klf_layout::layout_of(&ty, self.decls).size.max(1)
    }
}
