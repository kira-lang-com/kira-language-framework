//! One function's aggregate values moved into storage. A first walk gives
//! every aggregate value the name of its address; a second emits the
//! instructions that define those addresses and rewrites every use. Slots
//! are allocated once, at the top of the entry block, so a loop reuses its
//! slots: a value carried around a loop arrives as a block parameter, which
//! moves it into the block's own slot before anything can overwrite it.

use super::{Fresh, Signatures};
use crate::{
    Block, Function, Instruction, LoadMode, Operation, Parameter, StoreMode, Type, ValueTypes,
};
use klf_core::Name;
use klf_source::Span;
use std::collections::{HashMap, HashSet};

pub(super) fn rewrite(function: &Function, signatures: &Signatures<'_, '_>) -> Option<Function> {
    let types = crate::infer_value_types_in(function, signatures.index);
    let mut rewriter = Rewriter {
        signatures,
        types: &types,
        fresh: Fresh::new(function),
        address: HashMap::new(),
        slots: Vec::new(),
        emitted: Vec::new(),
        span: function.span,
    };
    let mut result = function.clone();
    let converted = signatures.is_converted(function.name.as_str());
    let outgoing = if converted {
        rewriter.signature(&mut result)
    } else if rewriter.signature_holds_aggregates(function) {
        return None;
    } else {
        None
    };
    rewriter.assign(function);
    for (index, block) in function.blocks.iter().enumerate() {
        result.blocks[index] = rewriter.block(block, &result.blocks[index], outgoing.as_ref())?;
    }
    let slots = std::mem::take(&mut rewriter.slots);
    let entry = result.blocks.first_mut()?;
    entry.instructions.splice(0..0, slots);
    Some(result)
}

struct Rewriter<'a, 'm> {
    signatures: &'a Signatures<'a, 'm>,
    types: &'a ValueTypes,
    fresh: Fresh,
    /// The name of the address of each aggregate value. A value that maps to
    /// itself already is an address; a `move` maps to its operand.
    address: HashMap<Name, Name>,
    /// The entry block's `stack.alloc`s.
    slots: Vec<Instruction>,
    emitted: Vec<Instruction>,
    span: Span,
}

impl Rewriter<'_, '_> {
    fn aggregate(&self, name: &Name) -> bool {
        self.types
            .get(name)
            .is_some_and(|ty| self.signatures.is_aggregate(ty))
    }

    fn ty(&self, name: &Name) -> Option<Type> {
        self.types.get(name).cloned()
    }

    /// Whether an aggregate crosses `function`'s signature, which an
    /// unconverted function cannot rewrite.
    fn signature_holds_aggregates(&self, function: &Function) -> bool {
        self.signatures.is_aggregate(&function.result)
            || function
                .parameters
                .iter()
                .any(|parameter| self.signatures.is_aggregate(&parameter.ty))
    }

    /// Gives a converted function its address parameters, and returns the
    /// parameter its result goes out through.
    fn signature(&mut self, result: &mut Function) -> Option<Name> {
        let aggregate_result = self.signatures.is_aggregate(&result.result);
        let mirrored = result.blocks.first().is_some_and(|entry| {
            entry.parameters.len() == result.parameters.len()
                && entry
                    .parameters
                    .iter()
                    .zip(&result.parameters)
                    .all(|(block, function)| block.name == function.name)
        });
        let mut addressed = HashSet::new();
        for parameter in &mut result.parameters {
            if self.signatures.is_aggregate(&parameter.ty) {
                parameter.ty = Type::Address(Box::new(parameter.ty.clone()));
                parameter.convention = None;
                addressed.insert(parameter.name.clone());
                self.address
                    .insert(parameter.name.clone(), parameter.name.clone());
            }
        }
        if mirrored && let Some(entry) = result.blocks.first_mut() {
            for parameter in &mut entry.parameters {
                if addressed.contains(&parameter.name) {
                    parameter.ty = Type::Address(Box::new(parameter.ty.clone()));
                    parameter.convention = None;
                }
            }
        }
        if !aggregate_result {
            return None;
        }
        let name = self.fresh.name("result");
        let parameter = Parameter {
            name: name.clone(),
            convention: None,
            context: false,
            ty: Type::Address(Box::new(result.result.clone())),
            span: result.span,
        };
        result.parameters.insert(0, parameter.clone());
        if mirrored && let Some(entry) = result.blocks.first_mut() {
            entry.parameters.insert(0, parameter);
        }
        result.result = Type::Unit;
        Some(name)
    }

    /// Names the address of every aggregate value.
    fn assign(&mut self, function: &Function) {
        let parameters: HashSet<&Name> = function.parameters.iter().map(|p| &p.name).collect();
        for block in &function.blocks {
            for parameter in &block.parameters {
                if self.aggregate(&parameter.name) && !parameters.contains(&parameter.name) {
                    let slot = self.fresh.name(parameter.name.as_str());
                    self.address.insert(parameter.name.clone(), slot);
                }
            }
            for instruction in &block.instructions {
                for value in &instruction.results {
                    if !self.aggregate(value) {
                        continue;
                    }
                    let address = match &instruction.operation {
                        Operation::Move(source) => source.clone(),
                        Operation::Select { .. } => value.clone(),
                        _ => self.fresh.name(value.as_str()),
                    };
                    self.address.insert(value.clone(), address);
                }
            }
        }
    }

    /// The address of the aggregate `value`, following `move`s.
    fn at(&self, value: &Name) -> Option<Name> {
        let mut current = self.address.get(value)?;
        for _ in 0..64 {
            match self.address.get(current) {
                Some(next) if next != current => current = next,
                _ => return Some(current.clone()),
            }
        }
        None
    }

    fn push(&mut self, results: Vec<Name>, operation: Operation) {
        self.emitted.push(Instruction {
            results,
            operation,
            debug_location: None,
            span: self.span,
        });
    }

    fn slot(&mut self, name: Name, ty: Type) {
        self.slots.push(Instruction {
            results: vec![name],
            operation: Operation::StackAlloc(ty),
            debug_location: None,
            span: self.span,
        });
    }

    fn temporary(&mut self, operation: Operation) -> Name {
        let name = self.fresh.name("t");
        self.push(vec![name.clone()], operation);
        name
    }

    fn block(&mut self, original: &Block, block: &Block, outgoing: Option<&Name>) -> Option<Block> {
        let mut block = block.clone();
        self.emitted.clear();
        for parameter in &mut block.parameters {
            let Some(slot) = self.address.get(&parameter.name).cloned() else {
                continue;
            };
            if slot == parameter.name {
                continue;
            }
            let ty = self.ty(&parameter.name)?;
            parameter.ty = Type::Address(Box::new(ty.clone()));
            parameter.convention = None;
            self.slot(slot.clone(), ty);
            self.push(
                Vec::new(),
                Operation::MoveAddr {
                    source: parameter.name.clone(),
                    dest: slot,
                    mode: StoreMode::Init,
                },
            );
        }
        for instruction in &original.instructions {
            self.instruction(instruction, outgoing)?;
        }
        block.instructions = std::mem::take(&mut self.emitted);
        Some(block)
    }

    #[allow(clippy::too_many_lines)]
    fn instruction(&mut self, instruction: &Instruction, outgoing: Option<&Name>) -> Option<()> {
        let results = &instruction.results;
        match &instruction.operation {
            Operation::StructNew { fields, .. } => {
                let target = self.constructed(results)?;
                for (index, field) in fields.iter().enumerate() {
                    let address = self.field(&target, index)?;
                    self.put(field, address)?;
                }
            }
            Operation::TupleNew(elements) => {
                let target = self.constructed(results)?;
                for (index, element) in elements.iter().enumerate() {
                    let address = self.field(&target, index)?;
                    self.put(element, address)?;
                }
            }
            Operation::ArrayNew { elements, .. } => {
                let target = self.constructed(results)?;
                for (index, element) in elements.iter().enumerate() {
                    let position = self.temporary(Operation::ConstInt {
                        ty: Type::Integer(64),
                        value: i128::try_from(index).ok()?,
                    });
                    let address = self.temporary(Operation::ElemAddr {
                        base: target.clone(),
                        index: position,
                    });
                    self.put(element, address)?;
                }
            }
            Operation::EnumNew {
                variant, payload, ..
            } => {
                let target = self.constructed(results)?;
                if !payload.is_empty() {
                    let base = self.temporary(Operation::EnumPayloadAddr {
                        base: target.clone(),
                        variant: *variant,
                    });
                    if let [only] = payload.as_slice() {
                        self.put(only, base)?;
                    } else {
                        for (index, value) in payload.iter().enumerate() {
                            let address = self.field(&base, index)?;
                            self.put(value, address)?;
                        }
                    }
                }
                self.push(
                    Vec::new(),
                    Operation::EnumInject {
                        address: target,
                        variant: *variant,
                    },
                );
            }
            Operation::StructExtract {
                aggregate: whole,
                field: index,
            }
            | Operation::TupleExtract {
                tuple: whole,
                index,
            } => {
                let [value] = results.as_slice() else {
                    return None;
                };
                let base = self.at(whole)?;
                self.read_field(&base, *index as usize, value, LoadMode::Trivial)?;
            }
            Operation::StructDestructure(whole) | Operation::TupleDestructure(whole) => {
                let base = self.at(whole)?;
                for (index, value) in results.iter().enumerate() {
                    let mode = self.taking(value)?;
                    self.read_field(&base, index, value, mode)?;
                }
            }
            Operation::EnumTake { value, variant } | Operation::EnumProject { value, variant } => {
                let taking = matches!(instruction.operation, Operation::EnumTake { .. });
                let base = self.at(value)?;
                if results.is_empty() {
                    return Some(());
                }
                if let [only] = results.as_slice()
                    && self.aggregate(only)
                {
                    let name = self.at(only)?;
                    let operation = Operation::EnumPayloadAddr {
                        base,
                        variant: *variant,
                    };
                    self.push(vec![name], operation);
                    return Some(());
                }
                let payload = self.temporary(Operation::EnumPayloadAddr {
                    base,
                    variant: *variant,
                });
                if let [only] = results.as_slice() {
                    let mode = if taking {
                        self.taking(only)?
                    } else {
                        LoadMode::Trivial
                    };
                    self.read(payload, only, mode)?;
                } else {
                    for (index, result) in results.iter().enumerate() {
                        let mode = if taking {
                            self.taking(result)?
                        } else {
                            LoadMode::Trivial
                        };
                        self.read_field(&payload, index, result, mode)?;
                    }
                }
            }
            Operation::Move(value) if self.aggregate(value) => {}
            Operation::Copy(value) if self.aggregate(value) => {
                let target = self.constructed(results)?;
                let source = self.at(value)?;
                let ty = self.ty(value)?;
                self.copy(&source, &target, &ty)?;
            }
            Operation::Destroy(value) if self.aggregate(value) => {
                let address = self.at(value)?;
                let ty = self.ty(value)?;
                self.destroy(&address, &ty)?;
            }
            Operation::Store {
                mode,
                value,
                address,
            } if self.aggregate(value) => {
                let mode = if *mode == StoreMode::Trivial {
                    StoreMode::Init
                } else {
                    *mode
                };
                let source = self.at(value)?;
                self.push(
                    Vec::new(),
                    Operation::MoveAddr {
                        source,
                        dest: address.clone(),
                        mode,
                    },
                );
            }
            Operation::Load { mode, address } if results.iter().any(|r| self.aggregate(r)) => {
                let target = self.constructed(results)?;
                let ty = self.ty(results.first()?)?;
                match mode {
                    LoadMode::Take => self.push(
                        Vec::new(),
                        Operation::MoveAddr {
                            source: address.clone(),
                            dest: target,
                            mode: StoreMode::Init,
                        },
                    ),
                    LoadMode::Trivial | LoadMode::Copy => self.copy(address, &target, &ty)?,
                }
            }
            Operation::Call {
                callee,
                generic_args,
                arguments,
            } => self.call(callee, generic_args, arguments, results)?,
            Operation::Return(Some(value)) if self.aggregate(value) => {
                let dest = outgoing?.clone();
                let source = self.at(value)?;
                self.push(
                    Vec::new(),
                    Operation::MoveAddr {
                        source,
                        dest,
                        mode: StoreMode::Init,
                    },
                );
                self.push(Vec::new(), Operation::Return(None));
            }
            operation => {
                let touches = operation
                    .used_values()
                    .into_iter()
                    .chain(results)
                    .any(|value| self.aggregate(value));
                if touches && !Self::passes_addresses(operation) {
                    return None;
                }
                let mut operation = operation.clone();
                for value in operation.used_values_mut() {
                    if self.aggregate(value) {
                        *value = self.at(value)?;
                    }
                }
                self.push(results.clone(), operation);
            }
        }
        Some(())
    }

    /// Operations that take an aggregate's address in its place unchanged.
    fn passes_addresses(operation: &Operation) -> bool {
        match operation {
            Operation::Select { .. } | Operation::EnumTag { .. } | Operation::SwitchEnum(_) => true,
            Operation::Return(_) | Operation::Fail(_) => false,
            operation => operation.is_terminator(),
        }
    }

    /// The slot of the one aggregate `results` names.
    fn constructed(&mut self, results: &[Name]) -> Option<Name> {
        let [value] = results else {
            return None;
        };
        let slot = self.at(value)?;
        let ty = self.ty(value)?;
        self.slot(slot.clone(), ty);
        Some(slot)
    }

    fn field(&mut self, base: &Name, index: usize) -> Option<Name> {
        Some(self.temporary(Operation::FieldAddr {
            base: base.clone(),
            field: u32::try_from(index).ok()?,
        }))
    }

    /// Defines `value` from the field `index` of the aggregate at `base`.
    fn read_field(
        &mut self,
        base: &Name,
        index: usize,
        value: &Name,
        mode: LoadMode,
    ) -> Option<()> {
        if self.aggregate(value) {
            let name = self.at(value)?;
            self.push(
                vec![name],
                Operation::FieldAddr {
                    base: base.clone(),
                    field: u32::try_from(index).ok()?,
                },
            );
            return Some(());
        }
        let address = self.field(base, index)?;
        self.read(address, value, mode)
    }

    /// Defines the scalar `value` by loading the storage at `address`.
    fn read(&mut self, address: Name, value: &Name, mode: LoadMode) -> Option<()> {
        if self.aggregate(value) {
            return None;
        }
        self.push(vec![value.clone()], Operation::Load { mode, address });
        Some(())
    }

    /// How a consuming read takes `value` out of storage.
    fn taking(&self, value: &Name) -> Option<LoadMode> {
        let ty = self.ty(value)?;
        Some(if self.signatures.is_trivial(&ty) {
            LoadMode::Trivial
        } else {
            LoadMode::Take
        })
    }

    /// Initializes the storage at `address` with `value`, consuming it.
    fn put(&mut self, value: &Name, address: Name) -> Option<()> {
        if self.aggregate(value) {
            let source = self.at(value)?;
            self.push(
                Vec::new(),
                Operation::MoveAddr {
                    source,
                    dest: address,
                    mode: StoreMode::Init,
                },
            );
        } else {
            self.push(
                Vec::new(),
                Operation::Store {
                    mode: StoreMode::Init,
                    value: value.clone(),
                    address,
                },
            );
        }
        Some(())
    }

    fn call(
        &mut self,
        callee: &Name,
        generic_args: &[Type],
        arguments: &[Name],
        results: &[Name],
    ) -> Option<()> {
        let aggregate_result = results.iter().any(|result| self.aggregate(result));
        let aggregate_argument = arguments.iter().any(|argument| self.aggregate(argument));
        if !self.signatures.is_converted(callee.as_str()) {
            if aggregate_result || aggregate_argument {
                return None;
            }
            self.push(
                results.to_vec(),
                Operation::Call {
                    callee: callee.clone(),
                    generic_args: generic_args.to_vec(),
                    arguments: arguments.to_vec(),
                },
            );
            return Some(());
        }
        let mut passed = Vec::with_capacity(arguments.len() + 1);
        let mut kept = results.to_vec();
        if aggregate_result {
            passed.push(self.constructed(results)?);
            kept.clear();
        }
        for argument in arguments {
            passed.push(if self.aggregate(argument) {
                self.at(argument)?
            } else {
                argument.clone()
            });
        }
        self.push(
            kept,
            Operation::Call {
                callee: callee.clone(),
                generic_args: generic_args.to_vec(),
                arguments: passed,
            },
        );
        Some(())
    }

    /// Initializes the storage at `target` with a copy of the `ty` at
    /// `source`, through each strong reference's count.
    fn copy(&mut self, source: &Name, target: &Name, ty: &Type) -> Option<()> {
        if self.signatures.is_trivial(ty) || matches!(ty, Type::Reference(_) | Type::Weak(_)) {
            self.push(
                Vec::new(),
                Operation::CopyAddr {
                    source: source.clone(),
                    dest: target.clone(),
                    mode: StoreMode::Init,
                },
            );
            return Some(());
        }
        for (index, field) in self.members(ty)?.into_iter().enumerate() {
            let from = self.field(source, index)?;
            let to = self.field(target, index)?;
            self.copy(&from, &to, &field)?;
        }
        Some(())
    }

    /// Destroys the `ty` at `address` member by member.
    fn destroy(&mut self, address: &Name, ty: &Type) -> Option<()> {
        if self.signatures.is_trivial(ty) {
            return Some(());
        }
        if matches!(ty, Type::Reference(_) | Type::Unique(_) | Type::Weak(_)) {
            self.push(Vec::new(), Operation::DestroyAddr(address.clone()));
            return Some(());
        }
        for (index, field) in self.members(ty)?.into_iter().enumerate() {
            let member = self.field(address, index)?;
            self.destroy(&member, &field)?;
        }
        Some(())
    }

    /// The member types of a struct or tuple whose copy and destroy are
    /// memberwise; `None` for anything that needs a policy function, an
    /// enum's tag or an array's loop.
    fn members(&self, ty: &Type) -> Option<Vec<Type>> {
        match ty {
            Type::Tuple(elements) => Some(elements.clone()),
            Type::Nominal(name, _) => {
                let declaration = self.signatures.declaration(name.as_str())?;
                let memberwise = matches!(declaration.kind, crate::TypeKind::Struct)
                    && matches!(declaration.policy.destroy, crate::DestroyPolicy::Trivial)
                    && matches!(
                        declaration.policy.copy,
                        crate::CopyPolicy::Trivial | crate::CopyPolicy::None
                    );
                memberwise.then(|| declaration.fields.iter().map(|f| f.ty.clone()).collect())
            }
            _ => None,
        }
    }
}
