//! Ownership on storage rather than in registers (section 11.6 pass 5 and
//! section 12.8).
//!
//! `copy.addr %src to [init or assign] %dst` copies the value whose storage is
//! at `%src` into the storage at `%dst`, through the type's policy.
//! `move.addr` relocates it, leaving `%src` uninitialized. `destroy.addr %a`
//! destroys the value in place. `assign` destroys what `%dst` held first.
//!
//! An address is a pointer on this backend, so the payload move is a `memcpy` of
//! the pointee's layout and the policy is the retain, release or free that the
//! value form (`copy` and `destroy` in `mem.rs`) applies to a register.

use super::FnEmitter;
use super::mem::pointee;
use crate::EmitError;
use klf_core::Name;
use klf_kir::{Operation, StoreMode, Type};

impl FnEmitter<'_, '_> {
    pub(super) fn copy_addr(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::CopyAddr { source, dest, mode } = operation else {
            return Err(EmitError("not a copy.addr".to_owned()));
        };
        self.assign_first(dest, *mode)?;
        self.relocate(source, dest)?;
        self.retain_the_copy(source)
    }

    pub(super) fn move_addr(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::MoveAddr { source, dest, mode } = operation else {
            return Err(EmitError("not a move.addr".to_owned()));
        };
        self.assign_first(dest, *mode)?;
        // A relocation moves the one value there is, so nothing is retained.
        self.relocate(source, dest)
    }

    pub(super) fn destroy_addr(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::DestroyAddr(address) = operation else {
            return Err(EmitError("not a destroy.addr".to_owned()));
        };
        let Some(ty) = self.pointee_of(address)?.cloned() else {
            return Ok(());
        };
        let address = self.value(address)?;
        let pointer = self.fresh();
        self.line(&format!("  {pointer} = load ptr, ptr {address}"));
        match &ty {
            Type::Reference(_) => {
                let inline = self.countable(&ty);
                self.release(&pointer, inline);
            }
            Type::Unique(_) => {
                self.line(&format!("  call void @klfrt_free_raw(ptr {pointer})"));
            }
            _ => {}
        }
        Ok(())
    }

    /// `assign` gives the destination an old value, which has to go first.
    fn assign_first(&mut self, dest: &Name, mode: StoreMode) -> Result<(), EmitError> {
        if mode != StoreMode::Assign {
            return Ok(());
        }
        self.destroy_addr(&Operation::DestroyAddr(dest.clone()))
    }

    /// Move the payload from one storage to another. The length is the
    /// pointee's own layout, so an aggregate goes across whole and a scalar
    /// goes across as itself.
    fn relocate(&mut self, source: &Name, dest: &Name) -> Result<(), EmitError> {
        let size = self.pointee_bytes(source)?;
        let (source, dest) = (self.value(source)?, self.value(dest)?);
        self.intrinsic("declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)".to_owned());
        self.line(&format!(
            "  call void @llvm.memcpy.p0.p0.i64(ptr {dest}, ptr {source}, i64 {size}, i1 false)"
        ));
        Ok(())
    }

    /// The pointee's size, or one word for anything without a layout.
    fn pointee_bytes(&mut self, address: &Name) -> Result<u64, EmitError> {
        let Some(ty) = self.pointee_of(address)? else {
            return Ok(1);
        };
        let layout = klf_layout::layout_of(ty, &self.module.types);
        Ok(layout.size.max(1))
    }

    /// A `copy.addr` leaves a second owner behind, so a counted reference is
    /// retained once for the copy.
    fn retain_the_copy(&mut self, source: &Name) -> Result<(), EmitError> {
        let Some(ty @ Type::Reference(_)) = self.pointee_of(source)?.cloned() else {
            return Ok(());
        };
        if !self.countable(&ty) {
            return Ok(());
        }
        let address = self.value(source)?;
        let pointer = self.fresh();
        self.line(&format!("  {pointer} = load ptr, ptr {address}"));
        self.retain(&pointer, true);
        Ok(())
    }

    /// The type of the storage an address names, or `None` when the name is not
    /// an address at all.
    fn pointee_of(&self, address: &Name) -> Result<Option<&Type>, EmitError> {
        Ok(pointee(self.types.require(address)?))
    }
}
