//! Dynamic exclusivity checks on the native backends (section 11.8).
//!
//! `access.begin [read or modify] [static or dynamic or unchecked] %addr`
//! brackets a memory access. Under `static` the verifier has already proved no
//! two overlapping accesses conflict, and under `unchecked` nothing is
//! enforced, so both emit nothing. Only `dynamic` calls `klfrt_access_begin`,
//! which traps with `KLF-RT-0003` on a conflict.

use super::FnEmitter;
use crate::EmitError;
use klf_core::Name;
use klf_kir::{AccessMode, Operation};

impl FnEmitter<'_, '_> {
    pub(super) fn access_begin(
        &mut self,
        _results: &[Name],
        operation: &Operation,
    ) -> Result<(), EmitError> {
        let Operation::AccessBegin {
            mode,
            enforcement,
            address,
        } = operation
        else {
            return Err(EmitError("not an access".to_owned()));
        };
        if *enforcement != klf_kir::AccessEnforcement::Dynamic {
            return Ok(());
        }
        let operand = self.value(address)?;
        let mode = match mode {
            AccessMode::Read => 0,
            AccessMode::Modify => 1,
        };
        let size = self.access_span(address);
        self.line(&format!(
            "  call void @klfrt_access_begin(ptr {operand}, i32 {mode}, i64 {size})"
        ));
        Ok(())
    }

    pub(super) fn access_end(
        &mut self,
        _results: &[Name],
        operation: &Operation,
    ) -> Result<(), EmitError> {
        let Operation::AccessEnd(address) = operation else {
            return Err(EmitError("not an access end".to_owned()));
        };
        let operand = self.value(address)?;
        self.line(&format!("  call void @klfrt_access_end(ptr {operand})"));
        Ok(())
    }

    /// The byte range one `access` claims: the storage's own layout, or one
    /// word for anything that is not a sized record.
    fn access_span(&mut self, address: &Name) -> u64 {
        let Ok(ty) = self.types.require(address) else {
            return 1;
        };
        let layout = klf_layout::layout_of(ty, &self.module.types);
        layout.size.max(1)
    }
}
