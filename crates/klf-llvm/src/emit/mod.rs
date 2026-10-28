//! Per-function lowering of KIR to textual LLVM IR.

mod access;
mod arith;
mod atomic;
mod count;
mod enums;
mod fallible;
mod mem;
mod ownership;
mod unmanaged;

use crate::EmitError;
use crate::debug::DebugInfo;
use crate::lltype::{ValueTypes, llvm_result_type, llvm_type};
use klf_core::Name;
use klf_kir::{
    Block, BlockTarget, DebugLocation, Function, FunctionAbi, Linkage, Module, Operation, Type,
};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

/// The LLVM symbol of a function: a foreign import's C symbol (section
/// 26.3), or the C shim that makes its crossing when it has a C prototype,
/// otherwise its name under the `kir.` prefix.
pub(crate) fn symbol(function: &Function) -> String {
    if function.foreign && function.c_prototype.is_some() {
        format!("@\"{}\"", klf_kir::c_shim_symbol(function.name.as_str()))
    } else if function.foreign {
        format!("@\"{}\"", klf_kir::c_symbol(function.name.as_str()))
    } else {
        format!("@\"kir.{}\"", function.name)
    }
}

/// The LLVM calling-convention keyword for a function. `ridlcc` functions use
/// `swiftcc` (D-111); foreign imports always use the platform C convention.
pub(crate) fn calling_convention(function: &Function) -> &'static str {
    if function.foreign {
        return "";
    }
    let has_context = function
        .parameters
        .iter()
        .any(|parameter| parameter.context);
    if function.abi == FunctionAbi::Ridl
        || matches!(function.linkage, Linkage::Exported)
        || has_context
        || function.failure.is_some()
    {
        "swiftcc "
    } else {
        ""
    }
}

pub(crate) struct FnEmitter<'a, 'o> {
    module: &'a Module,
    function: &'a Function,
    strings: &'a HashMap<String, String>,
    types: ValueTypes,
    intrinsics: &'o mut BTreeSet<String>,
    debug: &'o mut DebugInfo<'a>,
    subprogram: Option<usize>,
    location: Option<usize>,
    refs: HashMap<Name, String>,
    block_params: HashMap<Name, Vec<(Name, Type)>>,
    text: String,
    temp: usize,
    label: usize,
}

impl<'a, 'o> FnEmitter<'a, 'o> {
    pub(crate) fn new(
        module: &'a Module,
        function: &'a Function,
        strings: &'a HashMap<String, String>,
        intrinsics: &'o mut BTreeSet<String>,
        debug: &'o mut DebugInfo<'a>,
    ) -> Self {
        let mut block_params = HashMap::new();
        for block in &function.blocks {
            let params = block
                .parameters
                .iter()
                .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
                .collect();
            block_params.insert(block.name.clone(), params);
        }
        let subprogram = debug.subprogram(function);
        Self {
            module,
            function,
            strings,
            types: ValueTypes::infer(function, module),
            intrinsics,
            debug,
            subprogram,
            location: None,
            refs: HashMap::new(),
            block_params,
            text: String::new(),
            temp: 0,
            label: 0,
        }
    }

    pub(crate) fn finish(self) -> String {
        self.text
    }

    pub(crate) fn emit(&mut self) -> Result<(), EmitError> {
        let ret = llvm_result_type(&self.function.result)?;
        let mut params: Vec<String> = self
            .function
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                let attr = if parameter.context {
                    "swiftself "
                } else {
                    mem::c_extension(self.function, Some(index))
                };
                Ok(format!("{} {attr}%arg{index}", llvm_type(&parameter.ty)?))
            })
            .collect::<Result<_, EmitError>>()?;
        if self.function.failure.is_some() {
            params.push("ptr %arg.err".to_owned());
        }
        let name = self.function.name.as_str();
        let cc = calling_convention(self.function);
        let symbol = symbol(self.function);

        if self.function.blocks.is_empty() {
            let extension = mem::c_extension(self.function, None);
            self.line(&format!(
                "declare {cc}{extension}{ret} {symbol}({})",
                params.join(", ")
            ));
            return Ok(());
        }

        self.line(&format!(
            "define {cc}{ret} @\"kir.{name}\"({}) {dbg}{{",
            params.join(", "),
            dbg = self.subprogram_dbg()
        ));
        self.line("entry:");
        self.line("  call void @klfrt_frame_enter()");
        for block in &self.function.blocks {
            for parameter in &block.parameters {
                let ty = llvm_type(&parameter.ty)?;
                self.line(&format!(
                    "  %\"ps.{}\" = alloca {ty}",
                    parameter.name.as_str()
                ));
            }
        }
        let entry = &self.function.blocks[0];
        for (index, parameter) in entry.parameters.iter().enumerate() {
            let ty = llvm_type(&parameter.ty)?;
            self.line(&format!(
                "  store {ty} %arg{index}, ptr %\"ps.{}\"",
                parameter.name.as_str()
            ));
            self.name_parameter(u32::try_from(index).unwrap_or(u32::MAX), parameter);
        }
        self.line(&format!("  br label %\"{}\"", entry.name.as_str()));

        for block in &self.function.blocks {
            self.emit_block(block)?;
        }
        self.line("}");
        Ok(())
    }

    fn emit_block(&mut self, block: &Block) -> Result<(), EmitError> {
        self.line(&format!("\"{}\":", block.name.as_str()));
        for parameter in &block.parameters {
            let ty = llvm_type(&parameter.ty)?;
            let dest = format!("%\"p.{}\"", parameter.name.as_str());
            self.line(&format!(
                "  {dest} = load {ty}, ptr %\"ps.{}\"",
                parameter.name.as_str()
            ));
            self.refs.insert(parameter.name.clone(), dest);
        }
        for instruction in &block.instructions {
            // An instruction carries a location only where it changes; the
            // ones after it come from the same place.
            if instruction.debug_location.is_some() {
                self.at(instruction.debug_location.as_deref());
            }
            self.emit_operation(&instruction.results, &instruction.operation)?;
        }
        self.at(None);
        Ok(())
    }

    /// The `!dbg` a definition, an instruction or a label carries. A line
    /// indented by two spaces is an instruction inside a block and takes the
    /// location of the instruction being lowered; anything else is structure.
    fn dbg(&self) -> String {
        self.location
            .map_or_else(String::new, |node| format!(", !dbg !{node}"))
    }

    /// The `!dbg` a definition carries, which is its subprogram.
    fn subprogram_dbg(&self) -> String {
        self.subprogram
            .map_or_else(String::new, |node| format!("!dbg !{node} "))
    }

    fn at(&mut self, location: Option<&DebugLocation>) {
        self.location = match (self.subprogram, location) {
            (Some(_), Some(location)) => {
                Some(self.debug.location(self.function.name.as_str(), location))
            }
            _ => None,
        };
    }

    /// Places the following records at the function's declaration site.
    fn at_declaration(&mut self, owner: &str) {
        self.location = Some(self.debug.declaration(owner));
    }

    /// Names `parameter` so that a debugger prints it (section 16.3). The
    /// slot the entry block spilled it into is the stable home a `#dbg_declare`
    /// wants, where an argument register does not survive the first call.
    fn name_parameter(&mut self, index: u32, parameter: &klf_kir::Parameter) {
        let name = self.function.name.as_str().to_owned();
        let Some(variable) =
            self.debug
                .parameter(&name, parameter.name.as_str(), &parameter.ty, index)
        else {
            return;
        };
        let slot = format!("ptr %\"ps.{}\"", parameter.name.as_str());
        self.at_declaration(&name);
        self.debug_record("dbg_declare", &slot, variable);
        self.at(None);
    }

    fn debug_value(
        &mut self,
        value: &Name,
        variable: &klf_kir::DebugVariable,
    ) -> Result<(), EmitError> {
        let owner = self.function.name.as_str().to_owned();
        let Some(node) =
            self.debug
                .value_variable(&owner, &variable.name, &variable.ty, &variable.scope)
        else {
            return Ok(());
        };
        let ty = llvm_type(&self.ty_of(value)?.clone())?;
        let operand = self.value(value)?;
        self.debug_record("dbg_value", &format!("{ty} {operand}"), node);
        Ok(())
    }

    fn debug_address(
        &mut self,
        address: &Name,
        variable: &klf_kir::DebugVariable,
    ) -> Result<(), EmitError> {
        let owner = self.function.name.as_str().to_owned();
        let Some(node) =
            self.debug
                .address_variable(&owner, &variable.name, &variable.ty, &variable.scope)
        else {
            return Ok(());
        };
        let operand = self.value(address)?;
        self.debug_record("dbg_declare", &format!("ptr {operand}"), node);
        Ok(())
    }

    /// The exclusivity brackets of section 11.8.
    fn access(&mut self, results: &[Name], operation: &Operation) -> Option<Result<(), EmitError>> {
        Some(match operation {
            Operation::AccessBegin { .. } => self.access_begin(results, operation),
            Operation::AccessEnd(_) => self.access_end(results, operation),
            _ => return None,
        })
    }

    /// The debug and hint operations of section 12.15, which name a variable
    /// for a debugger and carry no code of their own.
    fn hint(&mut self, operation: &Operation) -> Option<Result<(), EmitError>> {
        Some(match operation {
            Operation::DebugValue { value, variable } => self.debug_value(value, variable),
            Operation::DebugAddr { address, variable } => self.debug_address(address, variable),
            _ => return None,
        })
    }

    /// The memory operations of section 12.5, from the first stack slot to the
    /// last store.
    fn memory(&mut self, results: &[Name], operation: &Operation) -> Option<Result<(), EmitError>> {
        if let Some(unmanaged) = self.unmanaged(results, operation) {
            return Some(unmanaged);
        }
        Some(match operation {
            Operation::StackAlloc(ty) => self.stack_alloc(results, ty),
            Operation::FieldAddr { base, field } => self.field_addr(results, base, *field),
            Operation::ElemAddr { base, index } => self.elem_addr(results, base, index),
            Operation::GlobalAddress { global, .. } => self.global_addr(results, global),
            Operation::EnumTag { value } => self.enum_tag(results, value),
            Operation::EnumPayloadAddr { base, variant } => {
                self.enum_payload_addr(results, base, *variant)
            }
            Operation::EnumInject { address, variant } => self.enum_inject(address, *variant),
            Operation::HeapAllocCounted(ty) => self.heap_alloc(results, ty, false),
            Operation::HeapAllocUnique(ty) => self.heap_alloc(results, ty, true),
            Operation::HeapAllocArray { element, count } => {
                self.heap_alloc_array(results, element, count)
            }
            Operation::HeapAllocRaw { size, align } => self.heap_alloc_raw(results, size, align),
            Operation::HeapFreeRaw(pointer) => self.heap_free_raw(pointer),
            Operation::MemCopy {
                dest,
                source,
                length,
            } => self.bulk_copy("memcpy", dest, source, length),
            Operation::MemMove {
                dest,
                source,
                length,
            } => self.bulk_copy("memmove", dest, source, length),
            Operation::MemSet { dest, byte, length } => self.bulk_set(dest, byte, length),
            Operation::StackDealloc(address) => self.stack_dealloc(address),
            Operation::Load { address, .. } => self.load(results, address),
            Operation::Store { value, address, .. } => self.store(value, address),
            _ => return None,
        })
    }

    fn emit_operation(&mut self, results: &[Name], operation: &Operation) -> Result<(), EmitError> {
        if let Some(access) = self.access(results, operation) {
            return access;
        }
        if let Some(hint) = self.hint(operation) {
            return hint;
        }
        if let Some(memory) = self.memory(results, operation) {
            return memory;
        }
        if let Some(atomic) = self.atomic(results, operation) {
            return atomic;
        }
        if let Some(scalar) = self.scalar(results, operation) {
            return scalar;
        }
        match operation {
            Operation::Binary {
                operator,
                overflow,
                signed,
                lhs,
                rhs,
            } => self.binary(results, *operator, *overflow, *signed, lhs, rhs),
            Operation::IntegerCompare {
                predicate,
                lhs,
                rhs,
            } => self.icmp(results, *predicate, lhs, rhs),
            Operation::FloatBinary { operator, lhs, rhs } => {
                self.float_binary(results, *operator, lhs, rhs)
            }
            Operation::FloatCompare {
                predicate,
                lhs,
                rhs,
            } => self.fcmp(results, *predicate, lhs, rhs),
            Operation::FloatUnary { operator, value } => {
                self.float_unary(results, *operator, value)
            }
            Operation::Convert { kind, value, ty } => self.convert(results, *kind, value, ty),
            Operation::Move(value)
            | Operation::IntoRaw(value)
            | Operation::FromRaw { pointer: value, .. }
            | Operation::RefAddr(value)
            | Operation::UniqAddr(value)
            | Operation::AddrCast { address: value, .. } => self.alias(results, value),
            Operation::TryCall(_) => self.try_call(operation),
            Operation::Copy(value) => self.copy(results, value),
            Operation::Destroy(value) => self.destroy(value),
            Operation::RefIsUnique(value) => self.is_unique(results, value),
            Operation::CopyAddr { .. } => self.copy_addr(operation),
            Operation::MoveAddr { .. } => self.move_addr(operation),
            Operation::DestroyAddr(_) => self.destroy_addr(operation),
            Operation::Call {
                callee, arguments, ..
            } => self.call(results, callee, arguments),
            Operation::CallForeign { symbol, arguments } => self.call(results, symbol, arguments),
            Operation::CallVariadic {
                callee,
                fixed,
                variadic,
            } => self.call_variadic(results, callee, fixed, variadic),
            Operation::FunctionReference(function) => self.function_reference(results, function),
            Operation::CallIndirect(call) => self.call_indirect(
                results,
                &call.target,
                &call.params,
                &call.result,
                &call.arguments,
            ),
            Operation::BorrowBegin(_)
            | Operation::BorrowEnd(_)
            | Operation::Assume(_)
            | Operation::Expect { .. }
            | Operation::Nop => Ok(()),
            Operation::Branch(target) => self.branch(target),
            Operation::ConditionalBranch(branch) => {
                self.cond_branch(&branch.condition, &branch.then_target, &branch.else_target)
            }
            Operation::SwitchInt(switch) => {
                self.switch(&switch.value, &switch.cases, &switch.default)
            }
            Operation::SwitchEnum(switch) => {
                self.enum_switch(&switch.value, &switch.cases, switch.default.as_ref())
            }
            Operation::Return(value) => self.ret(value.as_ref()),
            Operation::Fail(value) => self.fail(value),
            Operation::Trap(code) => match trap_number(code) {
                Some(number) => {
                    self.trap(number);
                    Ok(())
                }
                None => self.trap_message(code),
            },
            Operation::Unreachable => {
                self.trap(1);
                Ok(())
            }
            other => Err(EmitError(format!(
                "operation not lowered by the native backend yet: {other:?}"
            ))),
        }
    }

    fn branch(&mut self, target: &BlockTarget) -> Result<(), EmitError> {
        self.store_edge(target)?;
        self.line(&format!("  br label %\"{}\"", target.block.as_str()));
        Ok(())
    }

    fn cond_branch(
        &mut self,
        condition: &Name,
        then_target: &BlockTarget,
        else_target: &BlockTarget,
    ) -> Result<(), EmitError> {
        let cond = self.value(condition)?;
        let then_prep = self.fresh_label("prep.then");
        let else_prep = self.fresh_label("prep.else");
        self.line(&format!(
            "  br i1 {cond}, label %{then_prep}, label %{else_prep}"
        ));
        self.line(&format!("{then_prep}:"));
        self.store_edge(then_target)?;
        self.line(&format!("  br label %\"{}\"", then_target.block.as_str()));
        self.line(&format!("{else_prep}:"));
        self.store_edge(else_target)?;
        self.line(&format!("  br label %\"{}\"", else_target.block.as_str()));
        Ok(())
    }

    fn switch(
        &mut self,
        value: &Name,
        cases: &[(i128, BlockTarget)],
        default: &BlockTarget,
    ) -> Result<(), EmitError> {
        let selector = self.value(value)?;
        let ty = llvm_type(self.types.require(value)?)?;
        let default_prep = self.fresh_label("prep.def");
        let mut arms = Vec::new();
        let mut preps = Vec::new();
        for (key, target) in cases {
            let prep = self.fresh_label("prep.case");
            arms.push(format!("{ty} {key}, label %{prep}"));
            preps.push((prep, target));
        }
        self.line(&format!(
            "  switch {ty} {selector}, label %{default_prep} [ {} ]",
            arms.join(" ")
        ));
        for (prep, target) in preps {
            self.line(&format!("{prep}:"));
            self.store_edge(target)?;
            self.line(&format!("  br label %\"{}\"", target.block.as_str()));
        }
        self.line(&format!("{default_prep}:"));
        self.store_edge(default)?;
        self.line(&format!("  br label %\"{}\"", default.block.as_str()));
        Ok(())
    }

    fn store_edge(&mut self, target: &BlockTarget) -> Result<(), EmitError> {
        let params = self
            .block_params
            .get(&target.block)
            .ok_or_else(|| EmitError(format!("branch to unknown block `{}`", target.block)))?
            .clone();
        if params.len() != target.arguments.len() {
            return Err(EmitError("branch argument count mismatch".to_owned()));
        }
        for (argument, (param, param_ty)) in target.arguments.iter().zip(params) {
            let ty = llvm_type(&param_ty)?;
            let source = self.value(argument)?;
            self.line(&format!(
                "  store {ty} {source}, ptr %\"ps.{}\"",
                param.as_str()
            ));
        }
        Ok(())
    }

    fn ret(&mut self, value: Option<&Name>) -> Result<(), EmitError> {
        self.line("  call void @klfrt_frame_exit()");
        match value {
            Some(name) => {
                let ty = llvm_type(self.types.require(name)?)?;
                let operand = self.value(name)?;
                self.line(&format!("  ret {ty} {operand}"));
            }
            None => self.line("  ret void"),
        }
        Ok(())
    }

    fn fail(&mut self, value: &Name) -> Result<(), EmitError> {
        let widened = self.widen_operand_to_i64(value)?;
        let boxed = self.fresh();
        self.line(&format!(
            "  {boxed} = call ptr @klfrt_error_box(i64 {widened})"
        ));
        self.line(&format!("  store ptr {boxed}, ptr %arg.err"));
        self.line("  call void @klfrt_frame_exit()");
        let zero = self.zero_return()?;
        self.line(&zero);
        Ok(())
    }

    fn propagate_failure(&mut self, slot: &str) {
        let error = self.fresh();
        self.line(&format!("  {error} = load ptr, ptr {slot}"));
        let failed = self.fresh();
        self.line(&format!("  {failed} = icmp ne ptr {error}, null"));
        let prop = self.fresh_label("fail.prop");
        let cont = self.fresh_label("fail.cont");
        self.line(&format!("  br i1 {failed}, label %{prop}, label %{cont}"));
        self.line(&format!("{prop}:"));
        if self.function.failure.is_some() {
            self.line(&format!("  store ptr {error}, ptr %arg.err"));
            self.line("  call void @klfrt_frame_exit()");
            let zero = self
                .zero_return()
                .unwrap_or_else(|_| "  ret void".to_owned());
            self.line(&zero);
        } else {
            let value = self.fresh();
            self.line(&format!("  {value} = call i64 @klfrt_error_value()"));
            self.line(&format!("  call void @klfrt_fail(i64 {value})"));
            self.line("  unreachable");
        }
        self.line(&format!("{cont}:"));
    }

    fn zero_return(&self) -> Result<String, EmitError> {
        Ok(match &self.function.result {
            Type::Unit | Type::Never => "  ret void".to_owned(),
            Type::Integer(_) => format!("  ret {} 0", llvm_type(&self.function.result)?),
            Type::Float(_) => format!("  ret {} 0.0", llvm_type(&self.function.result)?),
            _ => "  ret ptr null".to_owned(),
        })
    }

    pub(crate) fn trap(&mut self, code: i32) {
        self.line(&format!("  call void @klfrt_trap(i32 {code})"));
        self.line("  unreachable");
    }

    /// A trap whose code is a language check's message rather than a
    /// `KLF-RT` code: the runtime reports the message.
    fn trap_message(&mut self, message: &str) -> Result<(), EmitError> {
        let symbol = self
            .strings
            .get(message)
            .ok_or_else(|| EmitError(format!("missing trap message declaration for `{message}`")))?;
        self.line(&format!(
            "  call void @klfrt_trap_message(ptr @\"{symbol}\", i64 {})",
            message.len()
        ));
        self.line("  unreachable");
        Ok(())
    }

    fn widen_operand_to_i64(&mut self, name: &Name) -> Result<String, EmitError> {
        let ty = self.types.require(name)?.clone();
        let operand = self.value(name)?;
        match ty {
            Type::Integer(64) => Ok(operand),
            Type::Integer(_) => {
                let dest = self.fresh();
                self.line(&format!(
                    "  {dest} = zext {} {operand} to i64",
                    llvm_type(&ty)?
                ));
                Ok(dest)
            }
            other => Err(EmitError(format!("cannot widen `{other}` to i64"))),
        }
    }

    // Builder helpers shared with the arith and mem submodules.

    pub(crate) fn line(&mut self, text: &str) {
        if text.starts_with("  ") {
            let dbg = self.dbg();
            self.text.push_str(text);
            self.text.push_str(&dbg);
        } else {
            self.text.push_str(text);
        }
        self.text.push('\n');
    }

    /// A debug record (`#dbg_declare`, `#dbg_value`), which stands where an
    /// instruction would and carries the location of the code around it as its
    /// last operand.
    pub(crate) fn debug_record(&mut self, kind: &str, address: &str, variable: usize) {
        let at = self
            .location
            .map_or_else(String::new, |node| format!(", !{node}"));
        let _ = writeln!(
            self.text,
            "  #{kind}({address}, !{variable}, !DIExpression(){at})"
        );
    }

    pub(crate) fn fresh(&mut self) -> String {
        let name = format!("%t{}", self.temp);
        self.temp += 1;
        name
    }

    pub(crate) fn fresh_label(&mut self, prefix: &str) -> String {
        let name = format!("{prefix}.{}", self.label);
        self.label += 1;
        name
    }

    pub(crate) fn value(&self, name: &Name) -> Result<String, EmitError> {
        self.refs
            .get(name)
            .cloned()
            .ok_or_else(|| EmitError(format!("value `%{name}` used before definition")))
    }

    pub(crate) fn record(&mut self, name: &Name, operand: String) {
        self.refs.insert(name.clone(), operand);
    }

    pub(crate) fn record_pair(
        &mut self,
        results: &[Name],
        first: String,
        second: String,
    ) -> Result<(), EmitError> {
        let [first_name, second_name] = results else {
            return Err(EmitError("expected two results".to_owned()));
        };
        self.record(first_name, first);
        self.record(second_name, second);
        Ok(())
    }

    pub(crate) fn ty_of(&self, name: &Name) -> Result<&Type, EmitError> {
        self.types.require(name)
    }

    pub(crate) fn intrinsic(&mut self, declaration: String) {
        self.intrinsics.insert(declaration);
    }

    pub(crate) fn callee<'m>(&self, name: &str) -> Option<&'a Function>
    where
        'a: 'm,
    {
        self.module
            .functions
            .iter()
            .find(|function| function.name.as_str() == name)
    }
}

pub(super) fn single(results: &[Name]) -> Result<&Name, EmitError> {
    match results {
        [one] => Ok(one),
        _ => Err(EmitError("expected exactly one result".to_owned())),
    }
}

/// The number of a `KLF-RT-0005`-style trap code, or none when the code is
/// a language check's message instead.
pub(crate) fn trap_number(code: &str) -> Option<i32> {
    let (prefix, digits) = code.rsplit_once('-')?;
    let named = prefix
        .split('-')
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_uppercase()));
    if !named || digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i32>().ok()
}
