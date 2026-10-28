//! Failure on the Cranelift backend (D-118).
//!
//! A function that can `fail` takes an error out-pointer as its last
//! parameter: null is success, non-null is failure with the value in the
//! runtime's error box. `fail` boxes the value, writes the pointer and returns
//! zero. A plain `call` to a failing callee hands a failure on to its own
//! caller, or ends the process through `klfrt_fail` when it cannot fail
//! itself. `try.call` branches on the failure instead (section 12.11).

use super::{FnLowerer, TRAP};
use crate::{EmitError, err};
use cranelift::codegen::ir::{BlockArg, MemFlagsData, StackSlotData, StackSlotKind};
use cranelift::prelude::{InstBuilder, Value, types};
use cranelift_module::Module;
use klf_core::Name;
use klf_kir::{FallibleCall, Type};

impl FnLowerer<'_> {
    pub(super) fn fails(&self, callee: &Name) -> bool {
        self.index
            .functions
            .get(callee.as_str())
            .is_some_and(|function| function.failure.is_some())
    }

    pub(super) fn fail(&mut self, value: &Name) -> Result<(), EmitError> {
        let widened = self.widen_to_i64(value)?;
        let boxed = self.call_one(self.runtime.error_box, &[widened]);
        self.leave_failing(boxed);
        Ok(())
    }

    pub(super) fn call_failing(
        &mut self,
        results: &[Name],
        callee: &Name,
        arguments: &[Name],
    ) -> Result<(), EmitError> {
        let (values, error) = self.call_with_error_slot(callee, arguments)?;
        self.propagate_failure(error);
        if !results.is_empty() {
            let value = *values
                .first()
                .ok_or_else(|| err(format!("`@{callee}` returns no value")))?;
            self.define(results, value)?;
        }
        Ok(())
    }

    pub(super) fn try_call(&mut self, call: &FallibleCall) -> Result<(), EmitError> {
        let failure = self
            .index
            .functions
            .get(call.callee.as_str())
            .and_then(|function| function.failure.clone())
            .ok_or_else(|| {
                err(format!(
                    "`try.call` names `@{}`, which declares no failure type",
                    call.callee
                ))
            })?;
        let (values, error) = self.call_with_error_slot(&call.callee, &call.arguments)?;
        let on_error = self.builder.create_block();
        let on_normal = self.builder.create_block();
        self.builder
            .ins()
            .brif(error, on_error, &[], on_normal, &[]);

        self.builder.switch_to_block(on_error);
        let raw = self.call_one(self.runtime.error_value, &[]);
        let value = match failure {
            Type::Integer(64) => raw,
            Type::Integer(_) => {
                let ty = super::cl_type(&failure)?;
                self.builder.ins().ireduce(ty, raw)
            }
            other => {
                return Err(err(format!(
                    "cannot deliver a `{other}` error to a `try.call` error block"
                )));
            }
        };
        let mut arguments = vec![BlockArg::from(value)];
        arguments.extend(self.edge_args(&call.error)?);
        let block = self.blocks[&call.error.block];
        self.builder.ins().jump(block, &arguments);

        self.builder.switch_to_block(on_normal);
        let mut arguments: Vec<BlockArg> = values.into_iter().map(BlockArg::from).collect();
        arguments.extend(self.edge_args(&call.normal)?);
        let block = self.blocks[&call.normal.block];
        self.builder.ins().jump(block, &arguments);
        Ok(())
    }

    /// After a call to a failing callee: a non-null `error` leaves this
    /// function, and code continues on the success path.
    pub(super) fn propagate_failure(&mut self, error: Value) {
        let propagate = self.builder.create_block();
        let resume = self.builder.create_block();
        self.builder.set_cold_block(propagate);
        self.builder.ins().brif(error, propagate, &[], resume, &[]);
        self.builder.switch_to_block(propagate);
        self.leave_failing(error);
        self.builder.switch_to_block(resume);
    }

    /// A fresh error slot holding null; answers its address.
    pub(super) fn error_slot(&mut self) -> Value {
        let pointer = self.module.target_config().pointer_type();
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        let address = self.builder.ins().stack_addr(pointer, slot, 0);
        let null = self.builder.ins().iconst(pointer, 0);
        self.builder
            .ins()
            .store(MemFlagsData::trusted(), null, address, 0);
        address
    }

    /// The error pointer a callee left in the slot at `address`.
    pub(super) fn read_error(&mut self, address: Value) -> Value {
        let pointer = self.module.target_config().pointer_type();
        self.builder
            .ins()
            .load(pointer, MemFlagsData::trusted(), address, 0)
    }

    /// Calls `callee` with a fresh, null error slot appended. Answers the
    /// call's results and the error pointer the callee left in the slot.
    fn call_with_error_slot(
        &mut self,
        callee: &Name,
        arguments: &[Name],
    ) -> Result<(Vec<Value>, Value), EmitError> {
        let id = self
            .ids
            .get(callee.as_str())
            .map(|(id, _)| *id)
            .ok_or_else(|| err(format!("call to unknown function `@{callee}`")))?;
        let address = self.error_slot();
        let mut args: Vec<Value> = arguments
            .iter()
            .map(|argument| self.value(argument))
            .collect::<Result<_, _>>()?;
        args.push(address);
        let fref = self.import(id);
        let values = self.invoke(fref, &args);
        Ok((values, self.read_error(address)))
    }

    /// Hands `error` to this function's caller through its out-pointer, or
    /// ends the process when this function cannot fail.
    fn leave_failing(&mut self, error: Value) {
        let Some((out, zero)) = self.error_out else {
            let value = self.call_one(self.runtime.error_value, &[]);
            self.call_void(self.runtime.fail, &[value]);
            self.builder.ins().trap(TRAP);
            return;
        };
        self.builder
            .ins()
            .store(MemFlagsData::trusted(), error, out, 0);
        self.exit_frame();
        let zero = zero.map(|ty| match ty {
            types::F32 => self.builder.ins().f32const(0.0),
            types::F64 => self.builder.ins().f64const(0.0),
            ty => self.builder.ins().iconst(ty, 0),
        });
        self.builder.ins().return_(zero.as_slice());
    }
}
