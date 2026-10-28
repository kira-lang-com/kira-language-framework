//! Functions and statements: structured control flow with explicit merge
//! and continue blocks (section 19.4 rule 4).

use crate::Writer;
use crate::words::{capability, class, instruction, op};
use klf_shader::{
    AddressSpace, AtomicFunction, BarrierFlags, Block, EntryPoint, Expression, Function, Handle,
    ScalarKind, Span, Statement, SwitchValue, Type, TypeInner,
};
use klf_shader_valid::FunctionInfo;

/// Where `break` and `continue` go.
#[derive(Clone, Copy)]
enum Construct {
    Loop { merge: u32, continuing: u32 },
    Switch { merge: u32 },
}

pub(crate) struct FunctionWriter<'w, 'a> {
    pub w: &'w mut Writer<'a>,
    pub function: &'a Function,
    pub info: &'a FunctionInfo,
    pub entry: Option<&'a EntryPoint>,
    pub values: Vec<u32>,
    pub arguments: Vec<u32>,
    pub locals: Vec<u32>,
    variables: Vec<u32>,
    body: Vec<u32>,
    terminated: bool,
    /// The file and line the last `OpLine` named.
    line: Option<(u32, u32)>,
    constructs: Vec<Construct>,
    pub outputs: Vec<crate::entry::Output>,
}

pub(crate) fn write_function<'a>(
    w: &mut Writer<'a>,
    id: u32,
    function: &'a Function,
    info: &'a FunctionInfo,
    entry: Option<&'a EntryPoint>,
) {
    let result = match (&function.result, entry) {
        (Some(result), None) => w.type_of(result.ty, false),
        _ => w.void(),
    };
    let mut parameters = Vec::new();
    if entry.is_none() {
        for argument in &function.arguments {
            parameters.push(w.type_of(argument.ty, false));
        }
    }
    let function_type = w.function_type(result, &parameters);
    let mut header = instruction(op::FUNCTION, &[result, id, 0, function_type]);
    let mut writer = FunctionWriter {
        w,
        function,
        info,
        entry,
        values: vec![0; function.expressions.len()],
        arguments: Vec::new(),
        locals: Vec::new(),
        variables: Vec::new(),
        body: Vec::new(),
        terminated: false,
        line: None,
        constructs: Vec::new(),
        outputs: Vec::new(),
    };
    for (index, ty) in parameters.iter().enumerate() {
        let parameter = writer.w.id();
        header.extend(instruction(op::FUNCTION_PARAMETER, &[*ty, parameter]));
        if let Some(name) = &function.arguments[index].name {
            writer.w.name(parameter, name);
        }
        writer.arguments.push(parameter);
    }
    let first = writer.w.id();
    header.extend(instruction(op::LABEL, &[first]));
    for (handle, local) in function.locals.iter() {
        let ty = writer.w.type_of(local.ty, false);
        let pointer = writer.w.pointer(class::FUNCTION, ty);
        let variable = writer.w.id();
        let init = match local.init {
            Some(init) => writer.const_expression(init).unwrap_or_else(|| {
                writer.w.refuse(
                    "a local initialized by a runtime value",
                    function.locals.span(handle),
                );
                writer.w.null(ty)
            }),
            None => writer.w.null(ty),
        };
        writer.variables.extend(instruction(
            op::VARIABLE,
            &[pointer, variable, class::FUNCTION, init],
        ));
        if let Some(name) = &local.name {
            writer.w.name(variable, name);
        }
        writer.locals.push(variable);
    }
    if let Some(entry) = entry {
        crate::entry::prologue(&mut writer, entry);
    }
    writer.globals_prelude();
    writer.block(&function.body);
    if !writer.terminated {
        if function.result.is_some() && entry.is_none() {
            writer.terminate(op::UNREACHABLE, &[]);
        } else {
            writer.terminate(op::RETURN, &[]);
        }
    }
    let FunctionWriter {
        w, variables, body, ..
    } = writer;
    w.functions.extend(header);
    w.functions.extend(variables);
    w.functions.extend(body);
    w.functions.extend(instruction(op::FUNCTION_END, &[]));
}

impl FunctionWriter<'_, '_> {
    pub(crate) fn emit(&mut self, opcode: u16, operands: &[u32]) {
        self.body.extend(instruction(opcode, operands));
    }

    /// Emits an instruction with a result: `[type, id, operands...]`.
    pub(crate) fn result(&mut self, opcode: u16, ty: u32, operands: &[u32]) -> u32 {
        let id = self.w.id();
        let mut all = vec![ty, id];
        all.extend_from_slice(operands);
        self.emit(opcode, &all);
        id
    }

    pub(crate) fn terminate(&mut self, opcode: u16, operands: &[u32]) {
        self.emit(opcode, operands);
        self.terminated = true;
    }

    fn label(&mut self, id: u32) {
        self.emit(op::LABEL, &[id]);
        self.terminated = false;
    }

    fn branch(&mut self, target: u32) {
        if !self.terminated {
            self.terminate(op::BRANCH, &[target]);
        }
    }

    /// A `Function` variable declared in the first block.
    pub(crate) fn temporary(&mut self, ty: u32) -> u32 {
        let pointer = self.w.pointer(class::FUNCTION, ty);
        let variable = self.w.id();
        self.variables.extend(instruction(
            op::VARIABLE,
            &[pointer, variable, class::FUNCTION],
        ));
        variable
    }

    /// The pointer value of each global the function names: buffer blocks
    /// reach their member, handles are loaded once.
    fn globals_prelude(&mut self) {
        let module = self.w.module;
        for (handle, expression) in self.function.expressions.iter() {
            let Expression::GlobalVariable(global) = expression else {
                continue;
            };
            let Some(id) = self.w.globals.get(global.index()).copied().flatten() else {
                continue;
            };
            let variable = &module.globals[*global];
            let value = match variable.space {
                AddressSpace::Handle => {
                    let ty = self.w.type_of(variable.ty, false);
                    self.result(op::LOAD, ty, &[id.variable])
                }
                space if id.wrapped => {
                    let member_type = self.w.type_of(variable.ty, true);
                    let pointer = self.w.pointer(crate::storage_class(space), member_type);
                    let zero = self.w.literal(klf_shader::Literal::U32(0));
                    self.result(op::ACCESS_CHAIN, pointer, &[id.variable, zero])
                }
                _ => id.variable,
            };
            self.values[handle.index()] = value;
        }
    }

    fn block(&mut self, block: &Block) {
        for (statement, span) in block.iter() {
            if self.terminated {
                return;
            }
            if self.w.debug_info && span.is_known() && self.line != Some((span.file, span.line)) {
                let file = self.w.file_string(span.file);
                self.emit(op::LINE, &[file, span.line, span.column]);
                self.line = Some((span.file, span.line));
            }
            self.statement(statement, span);
        }
    }

    fn statement(&mut self, statement: &Statement, span: Span) {
        match statement {
            Statement::Emit(range) => {
                for handle in range.iter() {
                    let value = self.expression(handle);
                    self.values[handle.index()] = value;
                    if let Some(name) = self.function.named_expressions.get(&handle.raw()) {
                        self.w.name(value, name);
                    }
                }
            }
            Statement::Block(block) => self.block(block),
            Statement::If {
                condition,
                accept,
                reject,
            } => {
                let condition = self.value(*condition);
                let merge = self.w.id();
                let accept_label = self.w.id();
                let reject_label = if reject.is_empty() {
                    merge
                } else {
                    self.w.id()
                };
                self.emit(op::SELECTION_MERGE, &[merge, 0]);
                self.terminate(
                    op::BRANCH_CONDITIONAL,
                    &[condition, accept_label, reject_label],
                );
                self.label(accept_label);
                self.block(accept);
                self.branch(merge);
                if !reject.is_empty() {
                    self.label(reject_label);
                    self.block(reject);
                    self.branch(merge);
                }
                self.label(merge);
            }
            Statement::Switch { selector, cases } => {
                let selector_id = self.value(*selector);
                let merge = self.w.id();
                let mut labels = vec![0; cases.len()];
                for (index, case) in cases.iter().enumerate().rev() {
                    labels[index] = match labels.get(index + 1) {
                        Some(next) if case.body.is_empty() && case.fall_through => *next,
                        _ => self.w.id(),
                    };
                }
                let mut default = merge;
                let mut operands = vec![selector_id, 0];
                for (case, label) in cases.iter().zip(&labels) {
                    match case.value {
                        SwitchValue::I32(value) => operands.extend([value.cast_unsigned(), *label]),
                        SwitchValue::U32(value) => operands.extend([value, *label]),
                        SwitchValue::Default => default = *label,
                    }
                }
                operands[1] = default;
                self.emit(op::SELECTION_MERGE, &[merge, 0]);
                self.terminate(op::SWITCH, &operands);
                self.constructs.push(Construct::Switch { merge });
                for (index, case) in cases.iter().enumerate() {
                    let next = labels.get(index + 1).copied();
                    if next == Some(labels[index]) {
                        continue;
                    }
                    self.label(labels[index]);
                    self.block(&case.body);
                    match next {
                        Some(next) if case.fall_through => self.branch(next),
                        _ => self.branch(merge),
                    }
                }
                self.constructs.pop();
                self.label(merge);
            }
            Statement::Loop {
                body,
                continuing,
                break_if,
            } => {
                let header = self.w.id();
                let body_label = self.w.id();
                let continuing_label = self.w.id();
                let merge = self.w.id();
                self.branch(header);
                self.label(header);
                self.emit(op::LOOP_MERGE, &[merge, continuing_label, 0]);
                self.terminate(op::BRANCH, &[body_label]);
                self.label(body_label);
                self.constructs.push(Construct::Loop {
                    merge,
                    continuing: continuing_label,
                });
                self.block(body);
                self.constructs.pop();
                self.branch(continuing_label);
                self.label(continuing_label);
                self.block(continuing);
                if !self.terminated {
                    match break_if {
                        Some(condition) => {
                            let condition = self.value(*condition);
                            self.terminate(op::BRANCH_CONDITIONAL, &[condition, merge, header]);
                        }
                        None => self.terminate(op::BRANCH, &[header]),
                    }
                }
                self.label(merge);
            }
            Statement::Break => match self.constructs.last() {
                Some(Construct::Loop { merge, .. } | Construct::Switch { merge }) => {
                    let merge = *merge;
                    self.terminate(op::BRANCH, &[merge]);
                }
                None => self.w.refuse("a break outside a loop or switch", span),
            },
            Statement::Continue => {
                let target = self
                    .constructs
                    .iter()
                    .rev()
                    .find_map(|construct| match construct {
                        Construct::Loop { continuing, .. } => Some(*continuing),
                        Construct::Switch { .. } => None,
                    });
                match target {
                    Some(target) => self.terminate(op::BRANCH, &[target]),
                    None => self.w.refuse("a continue outside a loop", span),
                }
            }
            Statement::Return { value } => {
                if self.entry.is_some() {
                    if let Some(value) = value {
                        let value = self.value(*value);
                        crate::entry::store_result(self, value);
                    }
                    self.terminate(op::RETURN, &[]);
                } else if let Some(value) = value {
                    let value = self.value(*value);
                    self.terminate(op::RETURN_VALUE, &[value]);
                } else {
                    self.terminate(op::RETURN, &[]);
                }
            }
            Statement::Kill => self.terminate(op::KILL, &[]),
            Statement::Store { pointer, value } => {
                let target = self.value(*pointer);
                let mut value = self.value(*value);
                match self.pointee(*pointer) {
                    Some((base, space))
                        if matches!(self.w.module.types[base].inner, TypeInner::Atomic(_)) =>
                    {
                        let scope = if space == AddressSpace::Workgroup {
                            2
                        } else {
                            1
                        };
                        let scope = self.w.literal(klf_shader::Literal::U32(scope));
                        let relaxed = self.w.literal(klf_shader::Literal::U32(0));
                        self.emit(op::ATOMIC_STORE, &[target, scope, relaxed, value]);
                        return;
                    }
                    Some((base, space)) if space.is_host_shared() => {
                        value = self.convert(value, base, true);
                    }
                    _ => {}
                }
                self.emit(op::STORE, &[target, value]);
            }
            Statement::ImageStore {
                image,
                coordinate,
                array_index,
                value,
            } => self.image_store(*image, *coordinate, *array_index, *value),
            Statement::Atomic {
                pointer,
                fun,
                value,
                compare,
                result,
            } => self.atomic(*pointer, *fun, *value, *compare, *result, span),
            Statement::Call {
                function,
                arguments,
                result,
            } => {
                let module = self.w.module;
                let callee = &module.functions[*function];
                let ty = match &callee.result {
                    Some(result) => self.w.type_of(result.ty, false),
                    None => self.w.void(),
                };
                let mut operands = vec![self.w.function_ids[function.index()]];
                for argument in arguments {
                    operands.push(self.value(*argument));
                }
                let id = self.result(op::FUNCTION_CALL, ty, &operands);
                if let Some(result) = result {
                    self.values[result.index()] = id;
                }
            }
            Statement::ControlBarrier(flags) => self.barrier(*flags, true),
            Statement::MemoryBarrier(flags) => self.barrier(*flags, false),
            Statement::WorkgroupUniformLoad { pointer, result } => {
                let flags = BarrierFlags {
                    workgroup: true,
                    ..BarrierFlags::default()
                };
                self.barrier(flags, true);
                let source = self.value(*pointer);
                let ty = self.resolved_type(*result);
                let id = self.result(op::LOAD, ty, &[source]);
                self.values[result.index()] = id;
                self.barrier(flags, true);
            }
        }
    }

    fn barrier(&mut self, flags: BarrierFlags, control: bool) {
        const WORKGROUP: u32 = 2;
        const DEVICE: u32 = 1;
        let mut semantics = 0x8;
        if flags.workgroup {
            semantics |= 0x100;
        }
        if flags.storage {
            semantics |= 0x40;
        }
        if flags.image {
            semantics |= 0x800;
        }
        let memory = if flags.storage || flags.image {
            DEVICE
        } else {
            WORKGROUP
        };
        let memory = self.w.literal(klf_shader::Literal::U32(memory));
        let semantics = self.w.literal(klf_shader::Literal::U32(semantics));
        if control {
            let execution = self.w.literal(klf_shader::Literal::U32(WORKGROUP));
            self.emit(op::CONTROL_BARRIER, &[execution, memory, semantics]);
        } else {
            self.emit(op::MEMORY_BARRIER, &[memory, semantics]);
        }
    }

    fn atomic(
        &mut self,
        pointer: Handle<Expression>,
        fun: AtomicFunction,
        value: Handle<Expression>,
        compare: Option<Handle<Expression>>,
        result: Option<Handle<Expression>>,
        span: Span,
    ) {
        let module = self.w.module;
        let Some((base, space)) = self.pointee(pointer) else {
            self.w.refuse("an atomic through a non-pointer", span);
            return;
        };
        let TypeInner::Atomic(scalar) = module.types[base].inner else {
            self.w.refuse("an atomic on a non-atomic value", span);
            return;
        };
        if scalar.width == 8 {
            self.w.capabilities.insert(capability::INT64_ATOMICS);
        }
        let scope = if space == AddressSpace::Workgroup {
            2
        } else {
            1
        };
        let scope = self.w.literal(klf_shader::Literal::U32(scope));
        let relaxed = self.w.literal(klf_shader::Literal::U32(0));
        let ty = self.w.scalar(scalar);
        let target = self.value(pointer);
        let value = self.value(value);
        let signed = scalar.kind == ScalarKind::Sint;
        let opcode = match fun {
            AtomicFunction::Add => op::ATOMIC_I_ADD,
            AtomicFunction::Subtract => op::ATOMIC_I_SUB,
            AtomicFunction::And => op::ATOMIC_AND,
            AtomicFunction::Or => op::ATOMIC_OR,
            AtomicFunction::Xor => op::ATOMIC_XOR,
            AtomicFunction::Min if signed => op::ATOMIC_S_MIN,
            AtomicFunction::Min => op::ATOMIC_U_MIN,
            AtomicFunction::Max if signed => op::ATOMIC_S_MAX,
            AtomicFunction::Max => op::ATOMIC_U_MAX,
            AtomicFunction::Exchange => op::ATOMIC_EXCHANGE,
            AtomicFunction::CompareExchange => op::ATOMIC_COMPARE_EXCHANGE,
        };
        let id = if fun == AtomicFunction::CompareExchange {
            let Some(compare) = compare else {
                self.w.refuse("compare_exchange without a comparand", span);
                return;
            };
            let compare = self.value(compare);
            self.result(
                opcode,
                ty,
                &[target, scope, relaxed, relaxed, value, compare],
            )
        } else {
            self.result(opcode, ty, &[target, scope, relaxed, value])
        };
        if let Some(result) = result {
            self.values[result.index()] = id;
        }
    }

    /// The pointee type and address space of a pointer expression.
    pub(crate) fn pointee(
        &self,
        pointer: Handle<Expression>,
    ) -> Option<(Handle<Type>, AddressSpace)> {
        let module = self.w.module;
        match self.info.expressions[pointer.index()]
            .ty
            .inner(&module.types)
        {
            TypeInner::Pointer { base, space } => Some((*base, *space)),
            _ => None,
        }
    }

    /// Converts an aggregate between its plain and layout forms.
    pub(crate) fn convert(&mut self, value: u32, ty: Handle<Type>, to_layout: bool) -> u32 {
        let module = self.w.module;
        let source = self.w.type_of(ty, !to_layout);
        let target = self.w.type_of(ty, to_layout);
        if source == target {
            return value;
        }
        let parts: Vec<(Handle<Type>, u32)> = match &module.types[ty].inner {
            TypeInner::Struct { members, .. } => members
                .iter()
                .enumerate()
                .map(|(index, member)| (member.ty, index as u32))
                .collect(),
            TypeInner::Array {
                base,
                size: klf_shader::ArraySize::Constant(count),
                ..
            } => (0..*count).map(|index| (*base, index)).collect(),
            _ => {
                self.w
                    .refuse("a buffer value that cannot be copied", Span::default());
                return value;
            }
        };
        let mut converted = Vec::new();
        for (part, index) in parts {
            let part_type = self.w.type_of(part, !to_layout);
            let extracted = self.result(op::COMPOSITE_EXTRACT, part_type, &[value, index]);
            converted.push(self.convert(extracted, part, to_layout));
        }
        self.result(op::COMPOSITE_CONSTRUCT, target, &converted)
    }
}
