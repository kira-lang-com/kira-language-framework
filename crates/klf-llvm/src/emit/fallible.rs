//! `try.call` on the LLVM backend (section 12.11).
//!
//! `try.call callee(args) normal bbN, error bbE` calls a `fails` callee and
//! splits on the error it hands back: `bbN` receives the result and `bbE` the
//! owned error, each as the leading parameters of the block. The callee takes
//! the error as an out-pointer, exactly as a plain `call` to a failing
//! function does, but a `try.call` has somewhere to branch to, so the failure
//! is not propagated out of this function.

use super::FnEmitter;
use crate::EmitError;
use crate::lltype::llvm_type;
use klf_kir::{BlockTarget, Operation, Type};

impl FnEmitter<'_, '_> {
    pub(super) fn try_call(&mut self, operation: &Operation) -> Result<(), EmitError> {
        let Operation::TryCall(call) = operation else {
            return Err(EmitError("not a try.call".to_owned()));
        };
        let failure = self
            .callee(call.callee.as_str())
            .and_then(|function| function.failure.clone())
            .ok_or_else(|| {
                EmitError(format!(
                    "`try.call` names `@{}`, which declares no failure type",
                    call.callee
                ))
            })?;
        let emitted = self.emit_call(&[], &call.callee, &call.arguments)?;
        let slot = emitted
            .error_slot
            .ok_or_else(|| EmitError("a failing call has no error slot".to_owned()))?;

        let error = self.fresh();
        self.line(&format!("  {error} = load ptr, ptr {slot}"));
        let failed = self.fresh();
        self.line(&format!("  {failed} = icmp ne ptr {error}, null"));
        let on_error = self.fresh_label("try.error");
        let on_normal = self.fresh_label("try.normal");
        self.line(&format!(
            "  br i1 {failed}, label %{on_error}, label %{on_normal}"
        ));

        self.line(&format!("{on_error}:"));
        let value = self.error_value(&failure)?;
        self.store_delivering_edge(&call.error, &value)?;

        self.line(&format!("{on_normal}:"));
        let delivered: Vec<(String, String)> = emitted.value.into_iter().collect();
        self.store_delivering_edge(&call.normal, &delivered)
    }

    /// The error the callee recorded, narrowed back from the runtime's `i64`
    /// error register to the declared failure type.
    fn error_value(&mut self, failure: &Type) -> Result<Vec<(String, String)>, EmitError> {
        let raw = self.fresh();
        self.line(&format!("  {raw} = call i64 @klfrt_error_value()"));
        let ty = llvm_type(failure)?;
        match failure {
            Type::Integer(64) => Ok(vec![(ty, raw)]),
            Type::Integer(_) => {
                let narrowed = self.fresh();
                self.line(&format!("  {narrowed} = trunc i64 {raw} to {ty}"));
                Ok(vec![(ty, narrowed)])
            }
            other => Err(EmitError(format!(
                "cannot deliver a `{other}` error to a `try.call` error block"
            ))),
        }
    }

    /// Stores `delivered` into the target's leading block parameters, then the
    /// target's own arguments into the rest, and branches.
    fn store_delivering_edge(
        &mut self,
        target: &BlockTarget,
        delivered: &[(String, String)],
    ) -> Result<(), EmitError> {
        let params = self
            .block_params
            .get(&target.block)
            .ok_or_else(|| EmitError(format!("branch to unknown block `{}`", target.block)))?
            .clone();
        if params.len() != delivered.len() + target.arguments.len() {
            return Err(EmitError("branch argument count mismatch".to_owned()));
        }
        let (received, passed) = params.split_at(delivered.len());
        for ((ty, operand), (param, _)) in delivered.iter().zip(received) {
            self.line(&format!(
                "  store {ty} {operand}, ptr %\"ps.{}\"",
                param.as_str()
            ));
        }
        for (argument, (param, param_ty)) in target.arguments.iter().zip(passed) {
            let ty = llvm_type(param_ty)?;
            let source = self.value(argument)?;
            self.line(&format!(
                "  store {ty} {source}, ptr %\"ps.{}\"",
                param.as_str()
            ));
        }
        self.line(&format!("  br label %\"{}\"", target.block.as_str()));
        Ok(())
    }
}
