//! Ownership on storage rather than in registers (section 11.6 pass 5 and
//! section 12.8).
//!
//! `copy.addr %src to [init or assign] %dst` copies the value whose storage is
//! at `%src` into the storage at `%dst`, through the type's policy.
//! `move.addr` relocates it, leaving `%src` uninitialized. `destroy.addr %a`
//! destroys the value in place. `assign` destroys what `%dst` held first.
//!
//! An address is a pointer here too, so the payload move is a memory copy of
//! the pointee's layout and the policy is the retain, release or free that the
//! value form (`copy` and `destroy` in `memory.rs`) applies to a register.

use super::FnLowerer;
use crate::EmitError;
use cranelift::codegen::ir::{InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;
use klf_core::Name;
use klf_kir::{Operation, StoreMode, Type};

impl FnLowerer<'_> {
    pub(super) fn copy_addr(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::CopyAddr { source, dest, mode } = operation else {
            return Err(crate::err("not a copy.addr"));
        };
        self.assign_first(dest, *mode)?;
        self.relocate(source, dest)?;
        self.retain_the_copy(source)
    }

    pub(super) fn move_addr(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::MoveAddr { source, dest, mode } = operation else {
            return Err(crate::err("not a move.addr"));
        };
        self.assign_first(dest, *mode)?;
        // A relocation moves the one value there is, so nothing is retained.
        self.relocate(source, dest)
    }

    pub(super) fn destroy_addr(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::DestroyAddr(address) = operation else {
            return Err(crate::err("not a destroy.addr"));
        };
        let Some(ty) = self.pointee_of(address)?.cloned() else {
            return Ok(());
        };
        let slot = self.value(address)?;
        let pointer = self.load_pointer(slot);
        match ty {
            Type::Reference(_) => self.call_void(self.runtime.release, &[pointer]),
            Type::Unique(_) => self.call_void(self.runtime.free_raw, &[pointer]),
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
        let bytes = self.pointee_bytes(source)?;
        let (dest, source) = (self.value(dest)?, self.value(source)?);
        let config = self.module.target_config();
        self.builder.emit_small_memory_copy(
            config,
            dest,
            source,
            bytes,
            1,
            1,
            true,
            MemFlagsData::new(),
        );
        Ok(())
    }

    /// The pointee's size, or one word for anything without a layout.
    fn pointee_bytes(&mut self, address: &Name) -> Result<u64, EmitError> {
        let Some(ty) = self.pointee_of(address)?.cloned() else {
            return Ok(1);
        };
        let layout = self.layouts.layout_of(&ty);
        Ok(layout.size.max(1))
    }

    /// A `copy.addr` leaves a second owner behind, so a counted reference is
    /// retained once for the copy.
    fn retain_the_copy(&mut self, source: &Name) -> Result<(), EmitError> {
        let Some(Type::Reference(_)) = self.pointee_of(source)?.cloned() else {
            return Ok(());
        };
        let slot = self.value(source)?;
        let pointer = self.load_pointer(slot);
        self.call_void(self.runtime.retain, &[pointer]);
        Ok(())
    }

    /// The pointer a slot of owned storage holds.
    fn load_pointer(&mut self, slot: Value) -> Value {
        let pointer = self.module.target_config().pointer_type();
        self.builder
            .ins()
            .load(pointer, MemFlagsData::trusted(), slot, 0)
    }

    /// The type of the storage an address names, or `None` when the name is not
    /// an address at all.
    fn pointee_of(&self, address: &Name) -> Result<Option<&Type>, EmitError> {
        Ok(super::pointee(self.ty_of(address)?))
    }
}
