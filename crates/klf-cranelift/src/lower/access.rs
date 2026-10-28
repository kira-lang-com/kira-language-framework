//! Dynamic exclusivity checks on the Cranelift backend (section 11.8).
//!
//! `access.begin [read or modify] [static or dynamic or unchecked] %addr`
//! brackets a memory access. Under `static` the verifier has already proved no
//! two overlapping accesses conflict, and under `unchecked` nothing is
//! enforced, so both emit nothing. Only `dynamic` calls `klfrt_access_begin`,
//! which traps with `KLF-RT-0003` on a conflict.

use super::FnLowerer;
use crate::EmitError;
use cranelift::codegen::ir::{InstBuilder, Value};
use cranelift::prelude::types;
use klf_core::Name;
use klf_kir::{AccessEnforcement, AccessMode, Operation};

impl FnLowerer<'_> {
    pub(super) fn access_begin(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::AccessBegin {
            mode,
            enforcement,
            address,
        } = operation
        else {
            return Err(crate::err("not an access"));
        };
        if *enforcement != AccessEnforcement::Dynamic {
            return Ok(());
        }
        let operand = self.value(address)?;
        let mode = i64::from(match mode {
            AccessMode::Read => 0,
            AccessMode::Modify => 1,
        });
        let size = self.access_span(address);
        let size = self.builder.ins().uextend(types::I64, size);
        let mode = self.builder.ins().iconst(types::I32, mode);
        self.call_void(self.runtime.access_begin, &[operand, mode, size]);
        Ok(())
    }

    pub(super) fn access_end(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::AccessEnd(address) = operation else {
            return Err(crate::err("not an access end"));
        };
        let operand = self.value(address)?;
        self.call_void(self.runtime.access_end, &[operand]);
        Ok(())
    }

    /// The byte range one `access` claims: the storage's own layout, or one
    /// word for anything that is not a sized record.
    fn access_span(&mut self, address: &Name) -> Value {
        let Some(ty) = self.types.get(address) else {
            return self.builder.ins().iconst(types::I32, 1);
        };
        let size = self.layouts.layout_of(ty).size;
        self.builder
            .ins()
            .iconst(types::I32, i64::try_from(size).unwrap_or(1).max(1))
    }
}
