//! Allocation, load/store through the runtime, counting and calls.

use super::FnEmitter;
use crate::EmitError;
use crate::lltype::{llvm_result_type, llvm_type, scalar_bits, size_align};
use klf_core::Name;
use klf_kir::{FunctionAbi, Type};

impl FnEmitter<'_, '_> {
    pub(super) fn stack_alloc(&mut self, results: &[Name], ty: &Type) -> Result<(), EmitError> {
        let layout = klf_layout::layout_of(ty, &self.module.types);
        let (size, align) = (layout.size, layout.align.max(1));
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = call ptr @klfrt_stack_alloc(i64 {size}, i64 {align})"
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn field_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        field: u32,
    ) -> Result<(), EmitError> {
        let offsets = match pointee(self.types.require(base)?) {
            Some(Type::Nominal(name, _)) => {
                klf_layout::field_offsets(name.as_str(), &self.module.types)
            }
            Some(Type::Tuple(elements)) => klf_layout::aggregate(elements, &self.module.types).1,
            _ => {
                return Err(EmitError(
                    "`field.addr` base is not a struct or tuple address".into(),
                ));
            }
        };
        let offset = *offsets
            .get(field as usize)
            .ok_or_else(|| EmitError("`field.addr` field index out of range".into()))?;
        let base_val = self.value(base)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = getelementptr inbounds i8, ptr {base_val}, i64 {offset}"
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn global_addr(&mut self, results: &[Name], global: &Name) -> Result<(), EmitError> {
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = load ptr, ptr @\"kirg.{}\"",
            global.as_str()
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn elem_addr(
        &mut self,
        results: &[Name],
        base: &Name,
        index: &Name,
    ) -> Result<(), EmitError> {
        let element = match pointee(self.types.require(base)?) {
            Some(Type::Array(_, element) | Type::Vector(_, element)) => (**element).clone(),
            _ => return Err(EmitError("`elem.addr` base is not an array address".into())),
        };
        let layout = klf_layout::layout_of(&element, &self.module.types);
        let stride = layout.size.div_ceil(layout.align.max(1)) * layout.align.max(1);
        let bits = match self.types.require(index)? {
            Type::Integer(bits) => *bits,
            _ => return Err(EmitError("`elem.addr` index must be an integer".into())),
        };
        let index_val = self.value(index)?;
        let index64 = if bits == 64 {
            index_val
        } else {
            let widened = self.fresh();
            self.line(&format!("  {widened} = sext i{bits} {index_val} to i64"));
            widened
        };
        let byte_offset = self.fresh();
        self.line(&format!("  {byte_offset} = mul i64 {index64}, {stride}"));
        let base_val = self.value(base)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = getelementptr inbounds i8, ptr {base_val}, i64 {byte_offset}"
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn heap_alloc(
        &mut self,
        results: &[Name],
        ty: &Type,
        unique: bool,
    ) -> Result<(), EmitError> {
        let layout = klf_layout::layout_of(ty, &self.module.types);
        let (size, align) = (layout.size, layout.align.max(1));
        let callee = if unique {
            "klfrt_heap_alloc_unique"
        } else {
            "klfrt_heap_alloc_counted"
        };
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = call ptr @{callee}(i64 {size}, i64 {align})"
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    /// `heap.alloc.raw`: size and alignment are `i64` operands.
    pub(super) fn heap_alloc_raw(
        &mut self,
        results: &[Name],
        size: &Name,
        align: &Name,
    ) -> Result<(), EmitError> {
        let size = self.value(size)?;
        let align = self.value(align)?;
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = call ptr @klfrt_heap_alloc_untracked(i64 {size}, i64 {align})"
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    /// `mem.copy` and `mem.move`, through LLVM's own intrinsics.
    pub(super) fn bulk_copy(
        &mut self,
        intrinsic: &str,
        dest: &Name,
        source: &Name,
        length: &Name,
    ) -> Result<(), EmitError> {
        let length_type = llvm_type(self.ty_of(length)?)?;
        self.intrinsic(format!(
            "declare void @llvm.{intrinsic}.p0.p0.{length_type}(ptr, ptr, {length_type}, i1)"
        ));
        let (dest, source, length) = (self.value(dest)?, self.value(source)?, self.value(length)?);
        self.line(&format!(
            "  call void @llvm.{intrinsic}.p0.p0.{length_type}(ptr {dest}, ptr {source}, {length_type} {length}, i1 false)"
        ));
        Ok(())
    }

    pub(super) fn bulk_set(
        &mut self,
        dest: &Name,
        byte: &Name,
        length: &Name,
    ) -> Result<(), EmitError> {
        let length_type = llvm_type(self.ty_of(length)?)?;
        let byte_type = llvm_type(self.ty_of(byte)?)?;
        self.intrinsic(format!(
            "declare void @llvm.memset.p0.{length_type}(ptr, i8, {length_type}, i1)"
        ));
        let (dest, byte, length) = (self.value(dest)?, self.value(byte)?, self.value(length)?);
        let narrowed = if byte_type == "i8" {
            byte
        } else {
            let narrowed = self.fresh();
            self.line(&format!("  {narrowed} = trunc {byte_type} {byte} to i8"));
            narrowed
        };
        self.line(&format!(
            "  call void @llvm.memset.p0.{length_type}(ptr {dest}, i8 {narrowed}, {length_type} {length}, i1 false)"
        ));
        Ok(())
    }

    pub(super) fn heap_free_raw(&mut self, pointer: &Name) -> Result<(), EmitError> {
        let operand = self.value(pointer)?;
        self.line(&format!("  call void @klfrt_free_raw(ptr {operand})"));
        Ok(())
    }

    pub(super) fn heap_alloc_array(
        &mut self,
        results: &[Name],
        element: &Type,
        count: &Name,
    ) -> Result<(), EmitError> {
        let (size, align) = size_align(element)?;
        let count_value = self.value(count)?;
        let total = self.fresh();
        self.line(&format!("  {total} = mul i64 {size}, {count_value}"));
        let dest = self.fresh();
        self.line(&format!(
            "  {dest} = call ptr @klfrt_heap_alloc_untracked(i64 {total}, i64 {align})"
        ));
        let result = super::single(results)?.clone();
        self.record(&result, dest);
        Ok(())
    }

    pub(super) fn stack_dealloc(&mut self, address: &Name) -> Result<(), EmitError> {
        let operand = self.value(address)?;
        self.line(&format!("  call void @klfrt_stack_dealloc(ptr {operand})"));
        Ok(())
    }

    pub(super) fn copy(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let ty = self.ty_of(value)?.clone();
        let operand = self.value(value)?;
        if matches!(ty, Type::Reference(_)) {
            let inline = self.countable(&ty);
            self.retain(&operand, inline);
        }
        let result = super::single(results)?.clone();
        self.record(&result, operand);
        Ok(())
    }

    /// `ref.isunique %r`: whether `%r` holds the only strong count.
    pub(super) fn is_unique(&mut self, results: &[Name], value: &Name) -> Result<(), EmitError> {
        let operand = self.value(value)?;
        let byte = self.fresh();
        self.line(&format!(
            "  {byte} = call i8 @klfrt_is_unique(ptr {operand})"
        ));
        let flag = self.fresh();
        self.line(&format!("  {flag} = trunc i8 {byte} to i1"));
        let result = super::single(results)?.clone();
        self.record(&result, flag);
        Ok(())
    }

    pub(super) fn destroy(&mut self, value: &Name) -> Result<(), EmitError> {
        let ty = self.ty_of(value)?.clone();
        let operand = self.value(value)?;
        match ty {
            Type::Reference(_) => {
                let inline = self.countable(&ty);
                self.release(&operand, inline);
            }
            Type::Unique(_) => {
                self.line(&format!("  call void @klfrt_free_raw(ptr {operand})"));
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn load(&mut self, results: &[Name], address: &Name) -> Result<(), EmitError> {
        let result = super::single(results)?.clone();
        let result_ty = self.ty_of(&result)?.clone();
        let bytes = scalar_bits(&result_ty)?.div_ceil(8);
        let addr = self.value(address)?;
        let raw = self.fresh();
        self.line(&format!(
            "  {raw} = call i64 @klfrt_load(ptr {addr}, i32 {bytes})"
        ));
        let operand = self.decode_i64(&raw, &result_ty)?;
        self.record(&result, operand);
        Ok(())
    }

    pub(super) fn store(&mut self, value: &Name, address: &Name) -> Result<(), EmitError> {
        let ty = self.ty_of(value)?.clone();
        let bytes = scalar_bits(&ty)?.div_ceil(8);
        let addr = self.value(address)?;
        let raw = self.encode_i64(value, &ty)?;
        self.line(&format!(
            "  call void @klfrt_store(ptr {addr}, i32 {bytes}, i64 {raw})"
        ));
        Ok(())
    }

    pub(super) fn call(
        &mut self,
        results: &[Name],
        callee: &Name,
        arguments: &[Name],
    ) -> Result<(), EmitError> {
        let call = self.emit_call(results, callee, arguments)?;
        if let Some(slot) = call.error_slot {
            self.propagate_failure(&slot);
        }
        Ok(())
    }

    /// The call itself. `call` propagates a failure out of the function;
    /// `try.call` branches on it instead.
    pub(super) fn emit_call(
        &mut self,
        results: &[Name],
        callee: &Name,
        arguments: &[Name],
    ) -> Result<EmittedCall, EmitError> {
        let function = self
            .callee(callee.as_str())
            .ok_or_else(|| EmitError(format!("call to unknown function `@{callee}`")))?;
        let ret = llvm_result_type(&function.result)?;
        if function.parameters.len() != arguments.len() {
            return Err(EmitError(format!(
                "call to `@{callee}` has {} arguments but the function takes {}",
                arguments.len(),
                function.parameters.len()
            )));
        }
        let mut operands = Vec::new();
        for (index, (argument, parameter)) in arguments.iter().zip(&function.parameters).enumerate()
        {
            let operand = self.value(argument)?;
            let attr = if parameter.context {
                "swiftself "
            } else {
                c_extension(function, Some(index))
            };
            operands.push(format!("{} {attr}{operand}", llvm_type(&parameter.ty)?));
        }

        let error_slot = if function.failure.is_some() {
            let slot = self.fresh();
            self.line(&format!("  {slot} = alloca ptr"));
            self.line(&format!("  store ptr null, ptr {slot}"));
            operands.push(format!("ptr {slot}"));
            Some(slot)
        } else {
            None
        };

        let cc = super::calling_convention(function);
        let symbol = super::symbol(function);
        let extension = c_extension(function, None);
        let call = format!(
            "call {cc}{extension}{ret} {symbol}({})",
            operands.join(", ")
        );
        if ret == "void" {
            self.line(&format!("  {call}"));
            return Ok(EmittedCall {
                value: None,
                error_slot,
            });
        }
        let dest = self.fresh();
        self.line(&format!("  {dest} = {call}"));
        if !results.is_empty() {
            let result = super::single(results)?.clone();
            self.record(&result, dest.clone());
        }
        Ok(EmittedCall {
            value: Some((ret, dest)),
            error_slot,
        })
    }

    /// A `call.variadic` of a C import, whose variadics follow the platform
    /// (section 29.5): LLVM passes the tail as C does when the call names
    /// its variadic type. Calls of functions the module defines were given
    /// argument areas before this backend.
    pub(super) fn call_variadic(
        &mut self,
        results: &[Name],
        callee: &Name,
        fixed: &[Name],
        variadic: &[Name],
    ) -> Result<(), EmitError> {
        let function = self
            .callee(callee.as_str())
            .ok_or_else(|| EmitError(format!("call to unknown function `@{callee}`")))?;
        if !function.foreign || function.c_prototype.is_some() {
            return Err(EmitError(format!(
                "`call.variadic` of `@{callee}` uses the platform's variadics, which the \
                 native backend passes only to a C import without a C prototype"
            )));
        }
        let mut types = Vec::new();
        let mut operands = Vec::new();
        for (index, (argument, parameter)) in fixed.iter().zip(&function.parameters).enumerate() {
            let ty = llvm_type(&parameter.ty)?;
            let extension = c_extension(function, Some(index));
            operands.push(format!("{ty} {extension}{}", self.value(argument)?));
            types.push(ty);
        }
        for argument in variadic {
            let ty = self.ty_of(argument)?.clone();
            // C widens a narrower integer to `int` and a `float` to `double`
            // before a variadic call, and KIR does not say whether an integer
            // is signed, so a frontend passes the promoted value.
            let promoted = match &ty {
                klf_kir::Type::Integer(bits) => *bits >= 32,
                klf_kir::Type::Float(bits) => *bits != 32,
                _ => true,
            };
            if !promoted {
                return Err(EmitError(format!(
                    "a variadic argument of `@{callee}` has type {ty}, which C promotes \
                     before the call; pass the promoted value"
                )));
            }
            operands.push(format!("{} {}", llvm_type(&ty)?, self.value(argument)?));
        }
        types.push("...".to_owned());
        let ret = llvm_result_type(&function.result)?;
        let extension = c_extension(function, None);
        let call = format!(
            "call {extension}{ret} ({}) {}({})",
            types.join(", "),
            super::symbol(function),
            operands.join(", ")
        );
        if ret == "void" {
            self.line(&format!("  {call}"));
        } else {
            let dest = self.fresh();
            self.line(&format!("  {dest} = {call}"));
            if !results.is_empty() {
                self.record(super::single(results)?, dest);
            }
        }
        Ok(())
    }

    pub(super) fn function_reference(
        &mut self,
        results: &[Name],
        function: &Name,
    ) -> Result<(), EmitError> {
        // A function pointer value is the function symbol used directly.
        let result = super::single(results)?.clone();
        if self.callee(function.as_str()).is_some_and(|declared| {
            !declared.foreign
                && declared
                    .c_prototype
                    .as_ref()
                    .is_some_and(klf_kir::CPrototype::needs_shim)
        }) {
            // C enters it through its shim, so the pointer C is handed is
            // the shim's, which the shim object defines.
            let shim = klf_kir::c_shim_symbol(function.as_str());
            self.intrinsic(format!("declare void @\"{shim}\"()"));
            self.record(&result, format!("@\"{shim}\""));
            return Ok(());
        }
        let symbol = self
            .callee(function.as_str())
            .map_or_else(|| format!("@\"kir.{function}\""), super::symbol);
        self.record(&result, symbol);
        Ok(())
    }

    pub(super) fn call_indirect(
        &mut self,
        results: &[Name],
        target: &Name,
        params: &[Type],
        result_ty: &Type,
        arguments: &[Name],
    ) -> Result<(), EmitError> {
        let ret = llvm_result_type(result_ty)?;
        let callee = self.value(target)?;
        let signature = match self.ty_of(target)?.clone() {
            Type::Function(signature) => signature,
            ty => {
                return Err(EmitError(format!(
                    "indirect call target `%{target}` has non-function type `{ty}`"
                )));
            }
        };
        let call_abi = if signature.abi == FunctionAbi::Ridl
            || signature
                .parameters
                .iter()
                .any(|parameter| parameter.context)
        {
            "swiftcc "
        } else {
            ""
        };
        let mut operands = Vec::new();
        if signature.parameters.len() != arguments.len() {
            return Err(EmitError(format!(
                "indirect call target `%{target}` has {} arguments but its type takes {}",
                arguments.len(),
                signature.parameters.len()
            )));
        }
        for ((argument, param), signature_parameter) in
            arguments.iter().zip(params).zip(&signature.parameters)
        {
            let operand = self.value(argument)?;
            let attr = if signature_parameter.context {
                "swiftself "
            } else {
                ""
            };
            operands.push(format!("{} {attr}{operand}", llvm_type(param)?));
        }
        let error_slot = if signature.failure.is_some() {
            let slot = self.fresh();
            self.line(&format!("  {slot} = alloca ptr"));
            self.line(&format!("  store ptr null, ptr {slot}"));
            operands.push(format!("ptr {slot}"));
            Some(slot)
        } else {
            None
        };
        let call = format!("call {call_abi}{ret} {callee}({})", operands.join(", "));
        if results.is_empty() {
            self.line(&format!("  {call}"));
        } else {
            let result = super::single(results)?.clone();
            let dest = self.fresh();
            self.line(&format!("  {dest} = {call}"));
            self.record(&result, dest);
        }
        if let Some(slot) = error_slot {
            self.propagate_failure(&slot);
        }
        Ok(())
    }

    fn decode_i64(&mut self, raw: &str, ty: &Type) -> Result<String, EmitError> {
        match ty {
            Type::Integer(64) => Ok(raw.to_owned()),
            Type::Integer(_) => {
                let dest = self.fresh();
                self.line(&format!("  {dest} = trunc i64 {raw} to {}", llvm_type(ty)?));
                Ok(dest)
            }
            Type::Float(64) => {
                let dest = self.fresh();
                self.line(&format!("  {dest} = bitcast i64 {raw} to double"));
                Ok(dest)
            }
            Type::Float(32) => {
                let narrow = self.fresh();
                self.line(&format!("  {narrow} = trunc i64 {raw} to i32"));
                let dest = self.fresh();
                self.line(&format!("  {dest} = bitcast i32 {narrow} to float"));
                Ok(dest)
            }
            Type::Pointer
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Address(_)
            | Type::Function(_) => {
                let dest = self.fresh();
                self.line(&format!("  {dest} = inttoptr i64 {raw} to ptr"));
                Ok(dest)
            }
            other => Err(EmitError(format!("cannot load `{other}`"))),
        }
    }

    fn encode_i64(&mut self, name: &Name, ty: &Type) -> Result<String, EmitError> {
        let operand = self.value(name)?;
        match ty {
            Type::Integer(64) => Ok(operand),
            Type::Integer(_) => {
                let dest = self.fresh();
                self.line(&format!(
                    "  {dest} = zext {} {operand} to i64",
                    llvm_type(ty)?
                ));
                Ok(dest)
            }
            Type::Float(64) => {
                let dest = self.fresh();
                self.line(&format!("  {dest} = bitcast double {operand} to i64"));
                Ok(dest)
            }
            Type::Float(32) => {
                let narrow = self.fresh();
                self.line(&format!("  {narrow} = bitcast float {operand} to i32"));
                let dest = self.fresh();
                self.line(&format!("  {dest} = zext i32 {narrow} to i64"));
                Ok(dest)
            }
            Type::Pointer
            | Type::Reference(_)
            | Type::Unique(_)
            | Type::Weak(_)
            | Type::Address(_)
            | Type::Function(_) => {
                let dest = self.fresh();
                self.line(&format!("  {dest} = ptrtoint ptr {operand} to i64"));
                Ok(dest)
            }
            other => Err(EmitError(format!("cannot store `{other}`"))),
        }
    }
}

pub(super) fn pointee(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner) | Type::Weak(inner) => {
            Some(inner)
        }
        _ => None,
    }
}

/// What [`FnEmitter::emit_call`] leaves behind: the call's value with its LLVM
/// type, unless it returns `void`, and the error out-pointer slot when the
/// callee declares a failure type.
pub(super) struct EmittedCall {
    pub(super) value: Option<(String, String)>,
    pub(super) error_slot: Option<String>,
}

/// How C widens the narrow integer a function's C prototype gives its
/// parameter `index` (or its result, for `None`): the attribute LLVM passes
/// on to the target's ABI. A record result is the address passed first.
pub(super) fn c_extension(function: &klf_kir::Function, index: Option<usize>) -> &'static str {
    let Some(prototype) = &function.c_prototype else {
        return "";
    };
    let out = usize::from(matches!(prototype.result, klf_kir::CType::Value(_)));
    let ty = match index {
        None => Some(&prototype.result),
        Some(index) => index
            .checked_sub(out)
            .and_then(|index| prototype.parameters.get(index)),
    };
    match ty {
        Some(klf_kir::CType::Bool) => "zeroext ",
        Some(klf_kir::CType::Signed(bits)) if *bits < 32 => "signext ",
        Some(klf_kir::CType::Unsigned(bits)) if *bits < 32 => "zeroext ",
        _ => "",
    }
}
