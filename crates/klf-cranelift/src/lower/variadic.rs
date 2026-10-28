//! `call.variadic` of a C import (`KLF-RIDL.md` section 29.5): the variadic
//! tail goes where the target's C convention puts it. Cranelift has no
//! variadic signatures, so each call gets a signature of its own that places
//! the tail as C's callee expects to find it.

use super::{FnLowerer, cl_type};
use crate::{EmitError, err, is_void};
use cranelift::codegen::ir::MemFlagsData;
use cranelift::codegen::isa::CallConv;
use cranelift::prelude::{AbiParam, InstBuilder, Signature, Value, types};
use cranelift_module::Module;
use klf_core::Name;
use target_lexicon::Architecture;

/// The argument registers of each class under AAPCS64: x0 to x7, v0 to v7.
const AARCH64_REGISTERS: usize = 8;

impl FnLowerer<'_> {
    pub(crate) fn call_variadic(
        &mut self,
        results: &[Name],
        callee: &Name,
        fixed: &[Name],
        variadic: &[Name],
    ) -> Result<(), EmitError> {
        let function = self
            .index
            .functions
            .get(callee.as_str())
            .copied()
            .filter(|function| function.foreign && function.c_prototype.is_none())
            .ok_or_else(|| {
                err(format!(
                    "`call.variadic` of `@{callee}` uses the platform's variadics, which \
                     Cranelift passes only to a C import without a C prototype"
                ))
            })?;
        let id = self
            .ids
            .get(callee.as_str())
            .map(|(id, _)| *id)
            .ok_or_else(|| err(format!("call to unknown function `@{callee}`")))?;
        let isa = self.module.isa();
        let convention = isa.default_call_conv();
        if convention == CallConv::SystemV && isa.triple().architecture == Architecture::X86_64 {
            // The caller must say in `al` how many vector registers it used,
            // and Cranelift gives no way to set it.
            return Err(err(format!(
                "`call.variadic` of `@{callee}`: a SysV x86_64 variadic call sets `al`, which \
                 Cranelift cannot"
            )));
        }
        let mut signature = self.module.make_signature();
        let mut arguments = Vec::new();
        for (parameter, name) in function.parameters.iter().zip(fixed) {
            signature
                .params
                .push(AbiParam::new(cl_type(&parameter.ty)?));
            arguments.push(self.value(name)?);
        }
        let tail: Vec<Value> = variadic
            .iter()
            .map(|name| self.variadic_argument(callee, name, convention))
            .collect::<Result<_, _>>()?;
        if convention == CallConv::AppleAarch64 && !tail.is_empty() {
            self.fill_registers(callee, &mut signature, &mut arguments)?;
        }
        for value in tail {
            let ty = self.builder.func.dfg.value_type(value);
            signature.params.push(AbiParam::new(ty));
            arguments.push(value);
        }
        if !is_void(&function.result) {
            signature
                .returns
                .push(AbiParam::new(cl_type(&function.result)?));
        }
        let signature = self.builder.import_signature(signature);
        let fref = self.import(id);
        let pointer = self.module.target_config().pointer_type();
        let address = self.builder.ins().func_addr(pointer, fref);
        let values = self.invoke_indirect(signature, address, &arguments);
        if !results.is_empty() {
            self.define(results, values[0])?;
        }
        Ok(())
    }

    /// A variadic argument as it travels: a full 8-byte slot. An `int` is
    /// widened, as its slot's upper bytes are never read; a `double` on
    /// Windows x64 travels in the integer register of its position, which
    /// an integer of the same bits reaches.
    fn variadic_argument(
        &mut self,
        callee: &Name,
        name: &Name,
        convention: CallConv,
    ) -> Result<Value, EmitError> {
        let value = self.value(name)?;
        match self.builder.func.dfg.value_type(value) {
            types::I32 => Ok(self.builder.ins().uextend(types::I64, value)),
            types::F64 if convention == CallConv::WindowsFastcall => Ok(self
                .builder
                .ins()
                .bitcast(types::I64, MemFlagsData::new(), value)),
            types::I64 | types::F64 => Ok(value),
            _ => Err(err(format!(
                "a variadic argument of `@{callee}` is one C promotes before the call, or none \
                 C passes; pass the promoted value"
            ))),
        }
    }

    /// Apple's `AArch64` convention passes every variadic argument on the
    /// stack. Taking up each argument register that the fixed arguments
    /// leave free sends the tail there, each in an 8-byte slot.
    fn fill_registers(
        &mut self,
        callee: &Name,
        signature: &mut Signature,
        arguments: &mut Vec<Value>,
    ) -> Result<(), EmitError> {
        let floats = signature
            .params
            .iter()
            .filter(|parameter| parameter.value_type.is_float())
            .count();
        let integers = signature.params.len() - floats;
        if integers > AARCH64_REGISTERS || floats > AARCH64_REGISTERS {
            return Err(err(format!(
                "`call.variadic` of `@{callee}`: its fixed arguments already reach the stack, \
                 where Apple packs them unlike its variadic slots"
            )));
        }
        for _ in integers..AARCH64_REGISTERS {
            signature.params.push(AbiParam::new(types::I64));
            arguments.push(self.builder.ins().iconst(types::I64, 0));
        }
        for _ in floats..AARCH64_REGISTERS {
            signature.params.push(AbiParam::new(types::F64));
            arguments.push(self.builder.ins().f64const(0.0));
        }
        Ok(())
    }
}
