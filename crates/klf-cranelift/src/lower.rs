//! Per-function lowering of KIR to Cranelift IR.
mod access;
mod arith;
mod atomic;
mod bulk;
mod calls;
mod entry;
#[path = "enums.rs"]
mod enums;
mod fallible;
mod frame;
mod memory;
mod module_data;
mod owned;
mod ownership;
mod registry;
mod signature;
mod unmanaged;
mod variadic;

use crate::runtime::Runtime;
use crate::{EmitError, err, is_void};
use cranelift::codegen::ir::{
    BlockArg, ExtFuncData, ExternalName, FuncRef, Function as ClFunction, StackSlot, TrapCode,
    UserExternalName,
};
use cranelift::prelude::{
    Block, FunctionBuilder, FunctionBuilderContext, InstBuilder, Signature, Value, types,
};
use cranelift_module::{DataId, FuncId, Module};
use cranelift_object::ObjectModule;
use klf_core::Name;
use klf_kir::{BlockTarget, Function, Operation, Type};
use rustc_hash::FxHashMap as HashMap;

pub(crate) use calls::declare_pending;
pub(crate) use entry::{ProgramEntry, emit_frontend_entry, emit_library_init, emit_main};
pub(crate) use module_data::{declare_globals, declare_strings};
pub(crate) use registry::emit_registry;

pub(crate) type IdMap<'a> = HashMap<&'a str, (FuncId, Signature)>;
pub(crate) type ShimMap<'a> = HashMap<&'a str, FuncId>;
pub(crate) type GlobalMap<'a> = HashMap<&'a str, DataId>;
pub(crate) type StringMap = HashMap<String, DataId>;

const TRAP: TrapCode = TrapCode::unwrap_user(1);

/// The Cranelift type for a scalar KIR type.
pub(crate) fn cl_type(ty: &Type) -> Result<types::Type, EmitError> {
    Ok(match ty {
        Type::Integer(1 | 8) => types::I8,
        Type::Integer(16) => types::I16,
        Type::Integer(32) => types::I32,
        Type::Integer(64)
        | Type::Pointer
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address(_)
        | Type::Function(_) => types::I64,
        Type::Integer(bits) => return Err(err(format!("unsupported integer width {bits}"))),
        Type::Float(32) => types::F32,
        Type::Float(64) => types::F64,
        Type::Float(bits) => return Err(err(format!("unsupported float width {bits}"))),
        other => return Err(err(format!("type not lowerable: {other}"))),
    })
}

pub(crate) use signature::signature;

pub(crate) struct FnLowerer<'a> {
    pub(crate) module: &'a ObjectModule,
    pub(crate) builder: FunctionBuilder<'a>,
    runtime: &'a Runtime,
    ids: &'a IdMap<'a>,
    shims: &'a ShimMap<'a>,
    globals: &'a GlobalMap<'a>,
    strings: &'a StringMap,
    types: klf_kir::ValueTypes,
    values: HashMap<Name, Value>,
    blocks: HashMap<Name, Block>,
    slots: HashMap<Name, StackSlot>,
    runtime_frame: bool,
    /// Whether the function can be part of a recursion, and so counts the
    /// call depth on entry and exit (`Module::recursive_functions`).
    counts_depth: bool,
    frame_alloc: bool,
    index: &'a klf_kir::ModuleIndex<'a>,
    layouts: &'a klf_layout::Layouts<'a>,
    /// The pending count polls load, when the function gets pads and polls.
    unwind: Option<DataId>,
    /// What the frame owns during the instruction being lowered.
    live: Vec<(Name, owned::Release)>,
    pad_of_instruction: Option<Block>,
    /// The pads made so far, with the values each releases.
    pads: Vec<(Block, Vec<(Value, owned::Release)>)>,
    /// The error out-pointer of a function that can `fail` (D-118), with the
    /// type its zero return carries.
    error_out: Option<(Value, Option<types::Type>)>,
    /// The source places the function's instructions name, which its
    /// Cranelift source locations index (`debug_info.rs`).
    pub(crate) places: Vec<klf_kir::DebugLocation>,
}

pub(crate) struct ModuleTables<'a> {
    pub(crate) runtime: &'a Runtime,
    pub(crate) ids: &'a IdMap<'a>,
    /// The C entry of each function `func.ref` takes through a shim.
    pub(crate) shims: &'a ShimMap<'a>,
    pub(crate) globals: &'a GlobalMap<'a>,
    pub(crate) strings: &'a StringMap,
    pub(crate) index: &'a klf_kir::ModuleIndex<'a>,
    pub(crate) layouts: &'a klf_layout::Layouts<'a>,
    pub(crate) unwind: Option<DataId>,
}

impl<'a> FnLowerer<'a> {
    pub(crate) fn new(
        module: &'a ObjectModule,
        func: &'a mut ClFunction,
        fbctx: &'a mut FunctionBuilderContext,
        tables: &ModuleTables<'a>,
        counts_depth: bool,
    ) -> Self {
        Self {
            module,
            builder: FunctionBuilder::new(func, fbctx),
            runtime: tables.runtime,
            ids: tables.ids,
            shims: tables.shims,
            globals: tables.globals,
            strings: tables.strings,
            types: klf_kir::ValueTypes::default(),
            values: HashMap::default(),
            blocks: HashMap::default(),
            slots: HashMap::default(),
            runtime_frame: false,
            counts_depth,
            frame_alloc: false,
            index: tables.index,
            layouts: tables.layouts,
            unwind: tables.unwind,
            live: Vec::new(),
            pad_of_instruction: None,
            pads: Vec::new(),
            error_out: None,
            places: Vec::new(),
        }
    }

    /// Marks the code built next as coming from `place`.
    fn place_at(&mut self, place: Option<&klf_kir::DebugLocation>) {
        let Some(place) = place else {
            return;
        };
        if self.places.last() != Some(place) {
            self.places.push(place.clone());
        }
        let index = u32::try_from(self.places.len() - 1).unwrap_or(0);
        self.builder.set_srcloc(cranelift::codegen::ir::SourceLoc::new(index));
    }

    pub(crate) fn lower(&mut self, function: &Function) -> Result<(), EmitError> {
        self.types = klf_kir::infer_value_types_in(function, self.index);
        for (position, block) in function.blocks.iter().enumerate() {
            let created = self.builder.create_block();
            // A block that only stops the program is laid out away from the
            // code that runs.
            if position > 0
                && matches!(
                    block.instructions.as_slice(),
                    [only] if matches!(only.operation, Operation::Trap(_) | Operation::Unreachable)
                )
            {
                self.builder.set_cold_block(created);
            }
            self.blocks.insert(block.name.clone(), created);
        }
        let entry = self.blocks[&function.blocks[0].name];
        self.builder.append_block_params_for_function_params(entry);
        for other in function.blocks.iter().skip(1) {
            let block = self.blocks[&other.name];
            for parameter in &other.parameters {
                let ty = cl_type(&parameter.ty)?;
                self.builder.append_block_param(block, ty);
            }
        }

        self.builder.switch_to_block(entry);
        let entry_values: Vec<Value> = self.builder.block_params(entry).to_vec();
        if function.failure.is_some() {
            let zero = (!is_void(&function.result)).then(|| cl_type(&function.result));
            self.error_out = entry_values.last().copied().zip(Some(zero.transpose()?));
        }
        for (parameter, value) in function.blocks[0].parameters.iter().zip(entry_values) {
            self.values.insert(parameter.name.clone(), value);
        }

        let entries = if self.unwind.is_some() {
            owned::block_entries(function, &self.types)
        } else {
            Vec::new()
        };
        // A terminator that may jump to a block at or before its own gets a
        // poll. Every cycle has such an edge, so each loop iteration polls.
        let positions: HashMap<&Name, usize> = function
            .blocks
            .iter()
            .enumerate()
            .map(|(position, block)| (&block.name, position))
            .collect();
        let loops_back = |position: usize, instruction: &klf_kir::Instruction| {
            instruction.operation.targets().iter().any(|target| {
                positions
                    .get(&target.block)
                    .is_some_and(|&to| to <= position)
            })
        };
        let frame_allocs = frame::frame_allocs(function, self.index);
        self.runtime_frame = !frame_allocs.is_empty();
        self.set_live(entries.first());
        if self.runtime_frame {
            self.call_void(self.runtime.frame_enter, &[]);
        } else if self.counts_depth {
            self.call_void(self.runtime.depth_enter, &[]);
        }
        self.poll();

        for (position, kir_block) in function.blocks.iter().enumerate() {
            let block = self.blocks[&kir_block.name];
            if block != entry {
                self.builder.switch_to_block(block);
                let params: Vec<Value> = self.builder.block_params(block).to_vec();
                for (parameter, value) in kir_block.parameters.iter().zip(params) {
                    self.values.insert(parameter.name.clone(), value);
                }
            }
            let mut owned = entries.get(position).cloned().unwrap_or_default();
            let last = kir_block.instructions.len().saturating_sub(1);
            for (index, instruction) in kir_block.instructions.iter().enumerate() {
                self.frame_alloc = instruction
                    .results
                    .iter()
                    .any(|result| frame_allocs.contains(result));
                if self.unwind.is_some() {
                    if index == last && loops_back(position, instruction) {
                        self.set_live(Some(&owned));
                        self.poll();
                    }
                    let consumed = owned::consumed(&instruction.operation);
                    let mut live = owned.clone();
                    for name in consumed {
                        live.remove(name);
                    }
                    self.set_live(Some(&live));
                }
                self.place_at(instruction.debug_location.as_deref());
                self.operation(&instruction.results, &instruction.operation)?;
                if self.unwind.is_some() {
                    owned::step(&mut owned, instruction, &self.types);
                }
            }
        }

        self.finish_pads();
        self.builder.seal_all_blocks();
        Ok(())
    }

    /// Makes `owned` what the pads of the next calls release.
    fn set_live(&mut self, owned: Option<&std::collections::BTreeSet<Name>>) {
        self.pad_of_instruction = None;
        self.live.clear();
        for name in owned.into_iter().flatten() {
            if let Some(release) = owned::release_of(self.types.get(name)) {
                self.live.push((name.clone(), release));
            }
        }
    }

    fn operation(&mut self, results: &[Name], operation: &Operation) -> Result<(), EmitError> {
        if let Some(result) = self.value_operation(results, operation) {
            return result;
        }
        self.memory_and_control(results, operation)
    }

    /// Constants, arithmetic, conversions and calls. Returns `None` for an
    /// operation this step does not handle.
    fn value_operation(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Option<Result<(), EmitError>> {
        let result = match operation {
            Operation::ConstInt { ty, value } => self.const_int(results, ty, *value),
            Operation::ConstBool(value) => {
                self.const_int(results, &Type::Integer(1), i128::from(*value))
            }
            Operation::ConstNull => self.const_int(results, &Type::Pointer, 0),
            Operation::ConstZero(ty) => self.const_zero(results, ty),
            Operation::ConstFloat { ty, literal } => self.const_float(results, ty, literal),
            Operation::ConstString(value) => self.const_string(results, value),
            Operation::Binary {
                operator,
                overflow,
                signed,
                lhs,
                rhs,
            } => self.binary(results, *operator, *overflow, *signed, lhs, rhs),
            Operation::BinaryOverflow {
                operator,
                signed,
                lhs,
                rhs,
            } => self.binary_overflow(results, *operator, *signed, lhs, rhs),
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
            | Operation::AddrCast { address: value, .. } => {
                self.value(value).and_then(|v| self.define(results, v))
            }
            Operation::Copy(value) => self.copy(results, value),
            Operation::Destroy(value) => self.destroy(value),
            Operation::RefIsUnique(value) => self.is_unique(results, value),
            Operation::CopyAddr { .. } => self.copy_addr(operation),
            Operation::MoveAddr { .. } => self.move_addr(operation),
            Operation::DestroyAddr(_) => self.destroy_addr(operation),
            Operation::AccessBegin { .. } => self.access_begin(operation),
            Operation::AccessEnd(_) => self.access_end(operation),
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
            Operation::CallIndirect(call) => self.call_indirect_op(
                results,
                &call.target,
                &call.params,
                &call.result,
                &call.arguments,
            ),
            _ => return None,
        };
        Some(result)
    }

    /// Addressing, memory, aggregates and the terminators.
    fn memory_and_control(
        &mut self,
        results: &[Name],
        operation: &Operation,
    ) -> Result<(), EmitError> {
        match operation {
            Operation::FieldAddr { base, field } => self.field_addr(results, base, *field),
            Operation::ElemAddr { base, index } => self.elem_addr(results, base, index),
            Operation::GlobalAddress { global, .. } => self.global_addr(results, global),
            Operation::EnumTag { value } => self.enum_tag(results, value),
            Operation::EnumPayloadAddr { base, variant } => {
                self.enum_payload_addr(results, base, *variant)
            }
            Operation::EnumInject { address, variant } => self.enum_inject(address, *variant),
            Operation::StackAlloc(ty) if !self.frame_alloc => self.stack_slot(results, ty),
            Operation::StackAlloc(ty) => self.alloc(results, ty, self.runtime.stack_alloc),
            Operation::HeapAllocCounted(ty) => {
                self.alloc(results, ty, self.runtime.heap_alloc_counted)
            }
            Operation::HeapAllocUnique(ty) => {
                self.alloc(results, ty, self.runtime.heap_alloc_unique)
            }
            Operation::HeapAllocArray { element, count } => {
                self.alloc_array(results, element, count)
            }
            Operation::HeapAllocRaw { size, align } => {
                let arguments = [self.value(size)?, self.value(align)?];
                let pointer = self.call_one(self.runtime.heap_alloc_untracked, &arguments);
                self.define(results, pointer)
            }
            Operation::MemCopy {
                dest,
                source,
                length,
            } => self.bulk_copy(dest, source, length, false),
            Operation::MemMove {
                dest,
                source,
                length,
            } => self.bulk_copy(dest, source, length, true),
            Operation::MemSet { dest, byte, length } => self.bulk_set(dest, byte, length),
            Operation::HeapFreeRaw(pointer) => {
                let pointer = self.value(pointer)?;
                self.call_void(self.runtime.free_raw, &[pointer]);
                Ok(())
            }
            Operation::StackDealloc(address) if self.slots.contains_key(address) => Ok(()),
            Operation::StackDealloc(address) => {
                let a = self.value(address)?;
                self.call_void(self.runtime.stack_dealloc, &[a]);
                Ok(())
            }
            Operation::AtomicLoad { .. }
            | Operation::AtomicStore { .. }
            | Operation::AtomicRmw { .. }
            | Operation::AtomicCompareExchange { .. }
            | Operation::Fence(_) => self.atomic(results, operation),
            Operation::Select { .. }
            | Operation::FloatFma { .. }
            | Operation::RawLoad { .. }
            | Operation::RawStore { .. }
            | Operation::LoadBorrow(_)
            | Operation::AddrToPointer(_)
            | Operation::RefUnowned(_)
            | Operation::PointerOffset { .. }
            | Operation::PointerDiff { .. }
            | Operation::TypeSize(_)
            | Operation::TypeAlign(_)
            | Operation::TypeStride(_) => self.unmanaged(results, operation),
            Operation::Load { address, .. } => self.load(results, address),
            Operation::Store { value, address, .. } => self.store(value, address),
            Operation::BorrowBegin(_) | Operation::BorrowEnd(_) => Ok(()),
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
            Operation::TryCall(call) => self.try_call(call),
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
            Operation::SetJmp(_) | Operation::LongJmp { .. } => Err(err(
                "`setjmp` returns twice, and Cranelift cannot keep a value the second return \
                 needs out of a register it hands to another value; the VM and LLVM run it",
            )),
            other => Err(err(format!(
                "operation not lowered by cranelift yet: {other:?}"
            ))),
        }
    }

    fn exit_frame(&mut self) {
        if self.runtime_frame {
            self.call_void(self.runtime.frame_exit, &[]);
        } else if self.counts_depth {
            self.call_void(self.runtime.depth_exit, &[]);
        }
    }

    fn ret(&mut self, value: Option<&Name>) -> Result<(), EmitError> {
        self.exit_frame();
        match value {
            Some(name) => {
                let v = self.value(name)?;
                self.builder.ins().return_(&[v]);
            }
            None => {
                self.builder.ins().return_(&[]);
            }
        }
        Ok(())
    }

    fn branch(&mut self, target: &BlockTarget) -> Result<(), EmitError> {
        let block = self.blocks[&target.block];
        let args = self.edge_args(target)?;
        self.builder.ins().jump(block, &args);
        Ok(())
    }

    fn cond_branch(
        &mut self,
        condition: &Name,
        then_target: &BlockTarget,
        else_target: &BlockTarget,
    ) -> Result<(), EmitError> {
        let cond = self.value(condition)?;
        let then_block = self.blocks[&then_target.block];
        let else_block = self.blocks[&else_target.block];
        let then_args = self.edge_args(then_target)?;
        let else_args = self.edge_args(else_target)?;
        self.builder
            .ins()
            .brif(cond, then_block, &then_args, else_block, &else_args);
        Ok(())
    }

    fn switch(
        &mut self,
        value: &Name,
        cases: &[(i128, BlockTarget)],
        default: &BlockTarget,
    ) -> Result<(), EmitError> {
        let selector = self.value(value)?;
        let ty = cl_type(self.ty_of(value)?)?;
        for (key, target) in cases {
            let key_value = self.builder.ins().iconst(ty, arith::imm64(*key));
            let equal =
                self.builder
                    .ins()
                    .icmp(cranelift::prelude::IntCC::Equal, selector, key_value);
            let hit = self.blocks[&target.block];
            let miss = self.builder.create_block();
            let args = self.edge_args(target)?;
            self.builder.ins().brif(equal, hit, &args, miss, &[]);
            self.builder.switch_to_block(miss);
        }
        let block = self.blocks[&default.block];
        let args = self.edge_args(default)?;
        self.builder.ins().jump(block, &args);
        Ok(())
    }

    pub(super) fn edge_args(&mut self, target: &BlockTarget) -> Result<Vec<BlockArg>, EmitError> {
        target
            .arguments
            .iter()
            .map(|argument| self.value(argument).map(BlockArg::from))
            .collect()
    }

    fn trap(&mut self, code: i32) {
        let code_value = self.builder.ins().iconst(types::I32, i64::from(code));
        self.call_void(self.runtime.trap, &[code_value]);
        self.builder.ins().trap(TRAP);
    }

    /// A trap whose code is a language check's message rather than a
    /// `KLF-RT` code: the runtime reports the message.
    fn trap_message(&mut self, message: &str) -> Result<(), EmitError> {
        let data_id = *self
            .strings
            .get(message)
            .ok_or_else(|| err(format!("missing trap message data for `{message}`")))?;
        let data = self.module.declare_data_in_func(data_id, self.builder.func);
        let pointer = self.module.target_config().pointer_type();
        let address = self.builder.ins().symbol_value(pointer, data);
        let length = i64::try_from(message.len()).unwrap_or(i64::MAX);
        let length = self.builder.ins().iconst(pointer, length);
        self.call_void(self.runtime.trap_message, &[address, length]);
        self.builder.ins().trap(TRAP);
        Ok(())
    }

    // Builder helpers shared with the arith submodule.

    /// A reference to the declared function `id`, as `Module::declare_func_in_func`
    /// makes one, but reading the module only: functions are lowered on
    /// several threads sharing it.
    pub(crate) fn import(&mut self, id: FuncId) -> FuncRef {
        let declaration = self.module.declarations().get_function_decl(id);
        let func = &mut *self.builder.func;
        let signature = func.import_signature(declaration.signature.clone());
        let name = func.declare_imported_user_function(UserExternalName {
            namespace: 0,
            index: id.as_u32(),
        });
        func.import_function(ExtFuncData {
            name: ExternalName::user(name),
            signature,
            colocated: declaration.linkage.is_final(),
            patchable: false,
        })
    }

    pub(crate) fn value(&mut self, name: &Name) -> Result<Value, EmitError> {
        if let Some(&slot) = self.slots.get(name) {
            let pointer = self.module.target_config().pointer_type();
            return Ok(self.builder.ins().stack_addr(pointer, slot, 0));
        }
        self.values
            .get(name)
            .copied()
            .ok_or_else(|| err(format!("value `%{name}` used before definition")))
    }

    pub(crate) fn define(&mut self, results: &[Name], value: Value) -> Result<(), EmitError> {
        let result = single(results)?;
        self.values.insert(result.clone(), value);
        Ok(())
    }

    pub(crate) fn define_pair(
        &mut self,
        results: &[Name],
        first: Value,
        second: Value,
    ) -> Result<(), EmitError> {
        let [first_name, second_name] = results else {
            return Err(err("expected two results"));
        };
        self.values.insert(first_name.clone(), first);
        self.values.insert(second_name.clone(), second);
        Ok(())
    }

    pub(crate) fn ty_of(&self, name: &Name) -> Result<&Type, EmitError> {
        self.types
            .get(name)
            .ok_or_else(|| err(format!("cannot type `%{name}`")))
    }
}

pub(crate) fn pointee(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Address(inner) | Type::Reference(inner) | Type::Unique(inner) | Type::Weak(inner) => {
            Some(inner)
        }
        _ => None,
    }
}

pub(crate) fn single(results: &[Name]) -> Result<&Name, EmitError> {
    match results {
        [one] => Ok(one),
        _ => Err(err("expected exactly one result")),
    }
}

/// The number of a `KLF-RT-0005`-style trap code, or none when the code is
/// a language check's message instead.
pub(crate) fn trap_number(code: &str) -> Option<i32> {
    let (prefix, digits) = code.rsplit_once('-')?;
    let named = prefix.split('-').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_uppercase()));
    if !named || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i32>().ok()
}
