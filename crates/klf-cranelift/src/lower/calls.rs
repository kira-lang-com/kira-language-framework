//! Calls, and in definition code the cleanup pads and polls that let a
//! language server unwind it (`KLF-EDITOR.md` section 8.6).
//!
//! A call that may raise, made while the frame owns counted values, is a
//! `try_call` whose exceptional edge lands in a pad: the pad releases those
//! values and resumes the unwind. Without owned values a call stays a plain
//! call, which the unwinder passes through. The normal path runs exactly the
//! code it ran before; the pads are out of line.

use super::FnLowerer;
use super::owned::Release;
use crate::{EmitError, err, is_void};
use cranelift::codegen::ir::{
    BlockArg, BlockCall, ExceptionTableData, ExceptionTableItem, FuncRef, MemFlagsData, SigRef,
    TrapCode,
};
use cranelift::prelude::{AbiParam, Block, InstBuilder, Value, types};
use cranelift_module::{DataId, FuncId, Linkage, Module};
use klf_core::Name;
use klf_kir::Type;

/// The data symbol every poll loads.
pub(crate) const PENDING: &str = "klfrt_interrupt_pending";

const UNREACHABLE: TrapCode = TrapCode::unwrap_user(1);

impl FnLowerer<'_> {
    pub(crate) fn call_void(&mut self, id: FuncId, args: &[Value]) {
        let fref = self.import(id);
        if self.runtime.raises(id) {
            self.invoke(fref, args);
        } else {
            self.builder.ins().call(fref, args);
        }
    }

    pub(crate) fn call_one(&mut self, id: FuncId, args: &[Value]) -> Value {
        let fref = self.import(id);
        if self.runtime.raises(id) {
            self.invoke(fref, args)[0]
        } else {
            let call = self.builder.ins().call(fref, args);
            self.builder.inst_results(call)[0]
        }
    }

    /// A call that may raise: a `try_call` into a pad when the frame owns
    /// something, else a plain call. Answers the call's results.
    pub(super) fn invoke(&mut self, fref: FuncRef, args: &[Value]) -> Vec<Value> {
        let signature = self.builder.func.dfg.ext_funcs[fref].signature;
        let Some((normal, table)) = self.exception_table(signature) else {
            let call = self.builder.ins().call(fref, args);
            return self.builder.inst_results(call).to_vec();
        };
        self.builder.ins().try_call(fref, args, table);
        self.builder.switch_to_block(normal);
        self.builder.block_params(normal).to_vec()
    }

    pub(super) fn invoke_indirect(
        &mut self,
        signature: SigRef,
        callee: Value,
        args: &[Value],
    ) -> Vec<Value> {
        let Some((normal, table)) = self.exception_table(signature) else {
            let call = self.builder.ins().call_indirect(signature, callee, args);
            return self.builder.inst_results(call).to_vec();
        };
        self.builder.ins().try_call_indirect(callee, args, table);
        self.builder.switch_to_block(normal);
        self.builder.block_params(normal).to_vec()
    }

    /// The block a `try_call` of `signature` continues in and its exception
    /// table, or `None` when the frame owns nothing a pad would release.
    fn exception_table(
        &mut self,
        signature: SigRef,
    ) -> Option<(Block, cranelift::codegen::ir::ExceptionTable)> {
        if self.unwind.is_none() || self.live.is_empty() {
            return None;
        }
        let pad = self.pad();
        let normal = self.builder.create_block();
        let returns: Vec<types::Type> = self.builder.func.dfg.signatures[signature]
            .returns
            .iter()
            .map(|parameter| parameter.value_type)
            .collect();
        for ty in &returns {
            self.builder.append_block_param(normal, *ty);
        }
        let pool = &mut self.builder.func.dfg.value_lists;
        let results = (0..returns.len())
            .map(|index| BlockArg::TryCallRet(u32::try_from(index).unwrap_or(u32::MAX)));
        let normal_call = BlockCall::new(normal, results, pool);
        let pad_call = BlockCall::new(pad, [BlockArg::TryCallExn(0)], pool);
        let table = self
            .builder
            .func
            .dfg
            .exception_tables
            .push(ExceptionTableData::new(
                signature,
                normal_call,
                [ExceptionTableItem::Default(pad_call)],
            ));
        Some((normal, table))
    }

    /// The pad for what the frame owns now, shared by every call of the same
    /// instruction. Its body is written by [`Self::finish_pads`] once every
    /// block of the function is filled.
    fn pad(&mut self) -> Block {
        if let Some(pad) = self.pad_of_instruction {
            return pad;
        }
        let pad = self.builder.create_block();
        self.builder.set_cold_block(pad);
        let pointer = self.module.target_config().pointer_type();
        self.builder.append_block_param(pad, pointer);
        let owned = self
            .live
            .iter()
            .filter_map(|(name, release)| Some((*self.values.get(name)?, *release)))
            .collect();
        self.pads.push((pad, owned));
        self.pad_of_instruction = Some(pad);
        pad
    }

    /// Writes each pad: release what the frame owned, then resume the unwind.
    pub(crate) fn finish_pads(&mut self) {
        for (pad, owned) in std::mem::take(&mut self.pads) {
            self.builder.switch_to_block(pad);
            let exception = self.builder.block_params(pad)[0];
            for (value, release) in owned {
                let id = match release {
                    Release::Counted => self.runtime.release,
                    Release::Unique => self.runtime.free_raw,
                };
                let fref = self.import(id);
                self.builder.ins().call(fref, &[value]);
            }
            if let Some((_, resume)) = self.runtime.unwinding {
                let resume = self.import(resume);
                self.builder.ins().call(resume, &[exception]);
            }
            self.builder.ins().trap(UNREACHABLE);
        }
    }

    /// The poll at a function's entry and before a back edge: one load of
    /// the pending count and a branch, with the call to `klfrt_poll` out of
    /// line.
    pub(crate) fn poll(&mut self) {
        let (Some(pending), Some((poll, _))) = (self.unwind, self.runtime.unwinding) else {
            return;
        };
        let pointer = self.module.target_config().pointer_type();
        let symbol = self.module.declare_data_in_func(pending, self.builder.func);
        let address = self.builder.ins().symbol_value(pointer, symbol);
        let count = self
            .builder
            .ins()
            .load(types::I32, MemFlagsData::trusted(), address, 0);
        let slow = self.builder.create_block();
        let resume = self.builder.create_block();
        self.builder.set_cold_block(slow);
        self.builder.ins().brif(count, slow, &[], resume, &[]);
        self.builder.switch_to_block(slow);
        self.call_void(poll, &[]);
        self.builder.ins().jump(resume, &[]);
        self.builder.switch_to_block(resume);
    }

    pub(crate) fn call(
        &mut self,
        results: &[Name],
        callee: &Name,
        arguments: &[Name],
    ) -> Result<(), EmitError> {
        let id = self
            .ids
            .get(callee.as_str())
            .map(|(id, _)| *id)
            .ok_or_else(|| err(format!("call to unknown function `@{callee}`")))?;
        if self.fails(callee) {
            return self.call_failing(results, callee, arguments);
        }
        let args: Vec<Value> = arguments
            .iter()
            .map(|argument| self.value(argument))
            .collect::<Result<_, _>>()?;
        let fref = self.import(id);
        let values = self.invoke(fref, &args);
        if !results.is_empty() {
            self.define(results, values[0])?;
        }
        Ok(())
    }

    pub(crate) fn function_reference(
        &mut self,
        results: &[Name],
        function: &Name,
    ) -> Result<(), EmitError> {
        let (mut id, _) = self
            .ids
            .get(function.as_str())
            .cloned()
            .ok_or_else(|| err(format!("`func.ref` names unknown function `@{function}`")))?;
        if let Some(&shim) = self.shims.get(function.as_str()) {
            id = shim;
        }
        let fref = self.import(id);
        let addr = self.builder.ins().func_addr(types::I64, fref);
        self.define(results, addr)
    }

    pub(crate) fn call_indirect_op(
        &mut self,
        results: &[Name],
        target: &Name,
        params: &[Type],
        result: &Type,
        arguments: &[Name],
    ) -> Result<(), EmitError> {
        let mut sig = self.module.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(super::cl_type(param)?));
        }
        if !is_void(result) {
            sig.returns.push(AbiParam::new(super::cl_type(result)?));
        }
        let fails = matches!(
            self.ty_of(target)?,
            Type::Function(signature) if signature.failure.is_some()
        );
        if fails {
            let pointer = self.module.target_config().pointer_type();
            sig.params.push(AbiParam::new(pointer));
        }
        let sigref = self.builder.import_signature(sig);
        let callee = self.value(target)?;
        let mut args: Vec<Value> = arguments
            .iter()
            .map(|argument| self.value(argument))
            .collect::<Result<_, _>>()?;
        let slot = fails.then(|| self.error_slot());
        args.extend(slot);
        let values = self.invoke_indirect(sigref, callee, &args);
        if let Some(slot) = slot {
            let error = self.read_error(slot);
            self.propagate_failure(error);
        }
        if !results.is_empty() {
            self.define(results, values[0])?;
        }
        Ok(())
    }
}

/// Declares the pending count polls load, for an object with unwind tables.
pub(crate) fn declare_pending(
    object: &mut cranelift_object::ObjectModule,
) -> Result<DataId, EmitError> {
    object
        .declare_data(PENDING, Linkage::Import, true, false)
        .map_err(|e| err(format!("declare `{PENDING}`: {e}")))
}
