//! Lowering of direct, foreign and indirect calls.

use super::{LowerError, Lowerer, err};
use crate::bytecode::Instr;
use crate::program::CallSite;
use klf_core::Name;
use klf_kir::Operation;

impl Lowerer<'_> {
    pub(super) fn lower_call_operation(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        let result = match operation {
            Operation::Call {
                callee, arguments, ..
            } => self.lower_call(results, callee, arguments),
            Operation::CallForeign { symbol, arguments } => {
                let import = self.foreign_import(symbol, arguments, results);
                import.and_then(|import| self.lower_call_foreign(results, arguments, import))
            }
            Operation::CallVariadic {
                callee,
                fixed,
                variadic,
            } => self
                .variadic_import(callee, fixed, variadic, results)
                .and_then(|import| {
                    let arguments: Vec<Name> = fixed.iter().chain(variadic).cloned().collect();
                    self.lower_call_foreign(results, &arguments, import)
                }),
            Operation::FunctionReference(function) => {
                self.lower_function_reference(results, function)
            }
            Operation::CallIndirect(call) => {
                self.lower_call_indirect(results, &call.target, &call.arguments)
            }
            Operation::TryCall(call) => self.lower_try_call(call),
            _ => return None,
        };
        Some(result)
    }

    /// Copies the arguments into consecutive fresh registers, the layout a
    /// call site reads, and returns the first of them.
    pub(super) fn stage_arguments(&mut self, arguments: &[Name]) -> Result<u16, LowerError> {
        let mut first_arg = 0u16;
        for (position, argument) in arguments.iter().enumerate() {
            let source = self.register(argument)?;
            let temporary = self.fresh()?;
            if position == 0 {
                first_arg = temporary;
            }
            self.emit(Instr::abc(
                crate::bytecode::Opcode::Mov,
                temporary,
                source,
                0,
            ));
        }
        Ok(first_arg)
    }

    fn lower_call(
        &mut self,
        results: &[Name],
        callee: &Name,
        arguments: &[Name],
    ) -> Result<(), LowerError> {
        let index = *self
            .indices
            .get(callee.as_str())
            .ok_or_else(|| err(format!("call to unknown function `@{callee}`")))?;
        let first_arg = self.stage_arguments(arguments)?;
        let has_result = !results.is_empty();
        let result = if has_result { self.result(results)? } else { 0 };
        let arg_count =
            u16::try_from(arguments.len()).map_err(|_| err("call has too many arguments"))?;
        let site = u16::try_from(self.call_sites.len()).map_err(|_| err("too many call sites"))?;
        self.call_sites.push(CallSite {
            callee: index,
            first_arg,
            arg_count,
            result,
            has_result,
            foreign: None,
            target: None,
            fallible: None,
        });
        self.emit(Instr::from_bx(
            crate::bytecode::Opcode::Call,
            result,
            i32::from(site),
        ));
        Ok(())
    }

    /// A call site of the native `import`, with `arguments` in its order.
    fn lower_call_foreign(
        &mut self,
        results: &[Name],
        arguments: &[Name],
        import: u32,
    ) -> Result<(), LowerError> {
        let first_arg = self.stage_arguments(arguments)?;
        let has_result = !results.is_empty();
        let result = if has_result { self.result(results)? } else { 0 };
        let arg_count = u16::try_from(arguments.len())
            .map_err(|_| err("call.foreign has too many arguments"))?;
        let site = u16::try_from(self.call_sites.len()).map_err(|_| err("too many call sites"))?;
        self.call_sites.push(CallSite {
            callee: 0,
            first_arg,
            arg_count,
            result,
            has_result,
            foreign: Some(import),
            target: None,
            fallible: None,
        });
        self.emit(Instr::from_bx(
            crate::bytecode::Opcode::Call,
            result,
            i32::from(site),
        ));
        Ok(())
    }

    fn lower_function_reference(
        &mut self,
        results: &[Name],
        function: &Name,
    ) -> Result<(), LowerError> {
        let index = *self
            .indices
            .get(function.as_str())
            .ok_or_else(|| err(format!("`func.ref` names unknown function `@{function}`")))?;
        let dest = self.result(results)?;
        if let Some(declared) = self.functions.get(function.as_str()).copied()
            && super::foreign::entered_from_c(declared)
        {
            return self.lower_entry_reference(dest, declared);
        }
        let index = i32::try_from(index).map_err(|_| err("too many functions"))?;
        self.emit(Instr::from_bx(crate::bytecode::Opcode::LeaFn, dest, index));
        Ok(())
    }

    fn lower_call_indirect(
        &mut self,
        results: &[Name],
        target: &Name,
        arguments: &[Name],
    ) -> Result<(), LowerError> {
        let target_reg = self.register(target)?;
        let mut first_arg = 0u16;
        for (position, argument) in arguments.iter().enumerate() {
            let source = self.register(argument)?;
            let temporary = self.fresh()?;
            if position == 0 {
                first_arg = temporary;
            }
            self.emit(Instr::abc(
                crate::bytecode::Opcode::Mov,
                temporary,
                source,
                0,
            ));
        }
        let has_result = !results.is_empty();
        let result = if has_result { self.result(results)? } else { 0 };
        let arg_count = u16::try_from(arguments.len())
            .map_err(|_| err("call.indirect has too many arguments"))?;
        let foreign = self.pointer_import(target, arguments, results)?;
        let site = u16::try_from(self.call_sites.len()).map_err(|_| err("too many call sites"))?;
        self.call_sites.push(CallSite {
            callee: 0,
            first_arg,
            arg_count,
            result,
            has_result,
            foreign,
            target: Some(target_reg),
            fallible: None,
        });
        self.emit(Instr::from_bx(
            crate::bytecode::Opcode::Call,
            result,
            i32::from(site),
        ));
        Ok(())
    }
}
