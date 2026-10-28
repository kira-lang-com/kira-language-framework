//! Aggregate values in storage (section 12.6). The aggregate instructions
//! work on structs, tuples, arrays and enums held as SSA values, which no
//! backend keeps in registers. Before a backend lowers a module, each
//! function that holds an aggregate value is rewritten so the value lives in
//! a stack slot of the entry block and every SSA name for it is the slot's
//! address: `struct.new` stores the fields into a fresh slot, `struct.extract`
//! loads one through `field.addr`, `enum.take` through `enum.payload.addr`,
//! and so on. Only instructions every backend already lowers come out.
//!
//! A module or private function that takes or returns an aggregate, has no
//! failure type and is only ever named by direct `call`s gets the address
//! instead: an aggregate parameter becomes `*T`, and an aggregate result a
//! leading `*T` parameter the callee moves its result into. An exported or
//! foreign function keeps its signature, since its convention belongs to
//! section 22.
//!
//! A function this cannot rewrite (an aggregate crossing a call it cannot
//! change, a copy or destroy that needs a policy function, an operation it
//! does not know) is left as it was, and so is every function whose
//! signature its calls depend on.

mod body;

use crate::{Function, Linkage, Module, ModuleIndex, Operation, Type, TypeDeclaration, TypeKind};
use klf_core::Name;
use std::collections::{HashMap, HashSet};

/// The module with its aggregate values moved into storage, or `None` when
/// it holds none this can move.
#[must_use]
pub fn aggregates_to_memory(module: &Module) -> Option<Module> {
    if !module.functions.iter().any(holds_aggregate_values) {
        return None;
    }
    let index = ModuleIndex::of(module);
    let referenced = referenced_functions(module);
    let mut converted: HashSet<&str> = module
        .functions
        .iter()
        .filter(|function| convertible(function, &index, &referenced))
        .map(|function| function.name.as_str())
        .collect();
    let mut failed: HashSet<&str> = HashSet::new();
    loop {
        let mut rewritten = HashMap::new();
        let mut changed = false;
        for function in &module.functions {
            let calls_converted = calls_any(function, &converted);
            let wanted = converted.contains(function.name.as_str())
                || calls_converted
                || holds_aggregate_values(function);
            if !wanted || failed.contains(function.name.as_str()) {
                continue;
            }
            let signatures = Signatures {
                index: &index,
                converted: &converted,
            };
            if let Some(function) = body::rewrite(function, &signatures) {
                rewritten.insert(function.name.clone(), function);
            } else {
                failed.insert(function.name.as_str());
                converted.remove(function.name.as_str());
                for callee in callees(function) {
                    converted.remove(callee);
                }
                changed = true;
            }
        }
        if changed {
            // A callee that lost its new signature changes what its callers
            // see, so every rewrite starts over. `failed` only grows and
            // `converted` only shrinks, so this ends.
            continue;
        }
        if rewritten.is_empty() {
            return None;
        }
        let mut result = module.clone();
        for function in &mut result.functions {
            if let Some(new) = rewritten.remove(&function.name) {
                *function = new;
            }
        }
        return Some(result);
    }
}

/// What a rewrite needs to know about the module: its declarations and
/// which functions take aggregates by address.
pub(super) struct Signatures<'a, 'm> {
    pub(super) index: &'a ModuleIndex<'m>,
    pub(super) converted: &'a HashSet<&'m str>,
}

impl Signatures<'_, '_> {
    pub(super) fn declaration(&self, name: &str) -> Option<&TypeDeclaration> {
        self.index.types.get(name).copied()
    }

    pub(super) fn is_converted(&self, callee: &str) -> bool {
        self.converted.contains(callee)
    }

    /// Whether a value of `ty` is an aggregate this rewrite keeps in storage.
    pub(super) fn is_aggregate(&self, ty: &Type) -> bool {
        is_aggregate(ty, self.index)
    }

    /// Whether copying and destroying a `ty` are bit copies and nothing.
    pub(super) fn is_trivial(&self, ty: &Type) -> bool {
        is_trivial(ty, self.index, 0)
    }
}

fn is_aggregate(ty: &Type, index: &ModuleIndex<'_>) -> bool {
    match ty {
        Type::Tuple(elements) => !elements.is_empty(),
        Type::Array(..) => true,
        Type::Nominal(name, arguments) => {
            arguments.is_empty()
                && index.types.get(name.as_str()).is_some_and(|declaration| {
                    declaration.generics.is_empty()
                        && matches!(declaration.kind, TypeKind::Struct | TypeKind::Enum)
                        && matches!(declaration.policy.relocate, crate::RelocatePolicy::Bitwise)
                })
        }
        _ => false,
    }
}

fn is_trivial(ty: &Type, index: &ModuleIndex<'_>, depth: u32) -> bool {
    if depth > 32 {
        return false;
    }
    match ty {
        Type::Reference(_) | Type::Unique(_) | Type::Weak(_) | Type::Generic(_) => false,
        Type::Tuple(elements) => elements.iter().all(|e| is_trivial(e, index, depth + 1)),
        Type::Array(_, element) => is_trivial(element, index, depth + 1),
        Type::Nominal(name, _) => index.types.get(name.as_str()).is_some_and(|declaration| {
            matches!(declaration.policy.copy, crate::CopyPolicy::Trivial)
                && matches!(declaration.policy.destroy, crate::DestroyPolicy::Trivial)
                && declaration
                    .fields
                    .iter()
                    .all(|field| is_trivial(&field.ty, index, depth + 1))
                && declaration
                    .variants
                    .iter()
                    .flat_map(|variant| &variant.payload)
                    .all(|payload| is_trivial(payload, index, depth + 1))
        }),
        _ => true,
    }
}

fn holds_aggregate_values(function: &Function) -> bool {
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| {
            matches!(
                instruction.operation,
                Operation::StructNew { .. }
                    | Operation::StructExtract { .. }
                    | Operation::StructDestructure(_)
                    | Operation::TupleNew(_)
                    | Operation::TupleExtract { .. }
                    | Operation::TupleDestructure(_)
                    | Operation::ArrayNew { .. }
                    | Operation::EnumNew { .. }
                    | Operation::EnumTake { .. }
                    | Operation::EnumProject { .. }
            )
        })
}

/// Whether `function` can take its aggregates by address: it has a body, is
/// not exported, fails with nothing and is only called directly.
fn convertible(function: &Function, index: &ModuleIndex<'_>, referenced: &HashSet<&str>) -> bool {
    !function.foreign
        && !function.blocks.is_empty()
        && function.failure.is_none()
        && function.generics.is_empty()
        && matches!(function.linkage, Linkage::Module | Linkage::Private)
        && !referenced.contains(function.name.as_str())
        && (is_aggregate(&function.result, index)
            || function
                .parameters
                .iter()
                .any(|parameter| is_aggregate(&parameter.ty, index)))
}

/// Every function a `func.ref` or anything but a direct `call` names.
pub(crate) fn referenced_functions(module: &Module) -> HashSet<&str> {
    let mut referenced = HashSet::new();
    for instruction in module
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
    {
        match &instruction.operation {
            Operation::FunctionReference(name) => {
                referenced.insert(name.as_str());
            }
            Operation::Call {
                generic_args,
                callee,
                ..
            } if !generic_args.is_empty() => {
                referenced.insert(callee.as_str());
            }
            _ => {}
        }
    }
    referenced
}

pub(crate) fn callees(function: &Function) -> impl Iterator<Item = &str> {
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match &instruction.operation {
            Operation::Call { callee, .. } => Some(callee.as_str()),
            _ => None,
        })
}

fn calls_any(function: &Function, converted: &HashSet<&str>) -> bool {
    callees(function).any(|callee| converted.contains(callee))
}

/// A name no value of `function` has, starting with `stem`.
pub(super) struct Fresh {
    taken: HashSet<Name>,
    next: u32,
}

impl Fresh {
    pub(super) fn new(function: &Function) -> Self {
        let mut taken: HashSet<Name> = function
            .parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        for block in &function.blocks {
            taken.extend(
                block
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.clone()),
            );
            for instruction in &block.instructions {
                taken.extend(instruction.results.iter().cloned());
            }
        }
        Self { taken, next: 0 }
    }

    pub(super) fn name(&mut self, stem: &str) -> Name {
        loop {
            let name = Name::new(format!("{stem}.agg{}", self.next));
            self.next += 1;
            if self.taken.insert(name.clone()) {
                return name;
            }
        }
    }
}
