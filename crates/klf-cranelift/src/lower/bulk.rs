//! Bulk memory: `mem.copy`, `mem.move` and `mem.set`. A length known at
//! compile time is expanded inline when Cranelift judges it small, and
//! otherwise becomes a call to the compiler intrinsic of the same name.

use super::FnLowerer;
use crate::EmitError;
use cranelift::codegen::ir::{InstructionData, MemFlagsData, Opcode, ValueDef};
use cranelift::prelude::{InstBuilder, Value, types};
use cranelift_module::Module;
use klf_core::Name;

impl FnLowerer<'_> {
    pub(super) fn bulk_copy(
        &mut self,
        dest: &Name,
        source: &Name,
        length: &Name,
        overlapping: bool,
    ) -> Result<(), EmitError> {
        let config = self.module.target_config();
        let (dest, source) = (self.value(dest)?, self.value(source)?);
        let length = self.value(length)?;
        if let Some(bytes) = self.constant(length) {
            self.builder.emit_small_memory_copy(
                config,
                dest,
                source,
                bytes,
                1,
                1,
                !overlapping,
                MemFlagsData::new(),
            );
        } else {
            let length = self.pointer_width(length);
            if overlapping {
                self.builder.call_memmove(config, dest, source, length);
            } else {
                self.builder.call_memcpy(config, dest, source, length);
            }
        }
        Ok(())
    }

    pub(super) fn bulk_set(
        &mut self,
        dest: &Name,
        byte: &Name,
        length: &Name,
    ) -> Result<(), EmitError> {
        let config = self.module.target_config();
        let dest = self.value(dest)?;
        let byte = self.value(byte)?;
        let length = self.value(length)?;
        if let (Some(fill), Some(bytes)) = (self.constant(byte), self.constant(length)) {
            #[allow(clippy::cast_possible_truncation)]
            self.builder
                .emit_small_memset(config, dest, fill as u8, bytes, 1, MemFlagsData::new());
        } else {
            let length = self.pointer_width(length);
            let byte = if self.builder.func.dfg.value_type(byte) == types::I8 {
                byte
            } else {
                self.builder.ins().ireduce(types::I8, byte)
            };
            self.builder.call_memset(config, dest, byte, length);
        }
        Ok(())
    }

    // The value an `iconst` defined, when `value` is one.
    fn constant(&self, value: Value) -> Option<u64> {
        let ValueDef::Result(instruction, 0) = self.builder.func.dfg.value_def(value) else {
            return None;
        };
        match self.builder.func.dfg.insts[instruction] {
            #[allow(clippy::cast_sign_loss)]
            InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } => Some(imm.bits() as u64),
            _ => None,
        }
    }

    fn pointer_width(&mut self, value: Value) -> Value {
        let pointer = self.module.target_config().pointer_type();
        if self.builder.func.dfg.value_type(value) == pointer {
            return value;
        }
        self.builder.ins().uextend(pointer, value)
    }
}
