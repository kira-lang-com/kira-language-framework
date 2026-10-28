//! Vector values as lanes (section 12.5). No backend keeps a `vec<N x T>`
//! in a register, so before one lowers a module each function that holds a
//! vector value is rewritten to hold its `N` lanes as separate `T` values:
//! `vec.splat`, `vec.extract`, `vec.insert` and `vec.shuffle` only pick
//! lanes, elementwise arithmetic, comparisons, `select`, `fma` and
//! conversions happen once per lane, and a load or store of a vector is one
//! per lane through `elem.addr`.
//!
//! A module or private function that takes or returns a vector, has no
//! failure type and is only ever named by direct `call`s takes the lanes as
//! parameters and returns them as a tuple, which the aggregate rewrite then
//! moves into storage. A function this cannot rewrite is left as it was,
//! and so is every function whose signature its calls depend on.

mod body;

use crate::aggregates::{callees, referenced_functions};
use crate::{Function, Linkage, Module, ModuleIndex, Type};
use std::collections::{HashMap, HashSet};

/// The module with its vector values split into lanes, or `None` when it
/// holds none this can split.
#[must_use]
pub fn vectors_to_lanes(module: &Module) -> Option<Module> {
    let index = ModuleIndex::of(module);
    let holding: HashSet<&str> = module
        .functions
        .iter()
        .filter(|function| holds_vectors(function, &index))
        .map(|function| function.name.as_str())
        .collect();
    if holding.is_empty() {
        return None;
    }
    let referenced = referenced_functions(module);
    let mut converted: HashSet<&str> = module
        .functions
        .iter()
        .filter(|function| convertible(function, &referenced))
        .map(|function| function.name.as_str())
        .collect();
    let mut failed: HashSet<&str> = HashSet::new();
    loop {
        let mut rewritten = HashMap::new();
        let mut changed = false;
        for function in &module.functions {
            let wanted = holding.contains(function.name.as_str())
                || callees(function).any(|callee| converted.contains(callee));
            if !wanted || failed.contains(function.name.as_str()) {
                continue;
            }
            if let Some(function) = body::rewrite(function, &index, &converted) {
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

/// Whether any value, parameter or result of `function` is a vector.
fn holds_vectors(function: &Function, index: &ModuleIndex<'_>) -> bool {
    if is_vector(&function.result)
        || function
            .parameters
            .iter()
            .any(|parameter| is_vector(&parameter.ty))
    {
        return true;
    }
    if function.blocks.is_empty() {
        return false;
    }
    crate::infer_value_types_in(function, index)
        .values()
        .any(is_vector)
}

/// Whether `function` can take and return its vectors as lanes: it has a
/// body, is not exported, fails with nothing and is only called directly.
fn convertible(function: &Function, referenced: &HashSet<&str>) -> bool {
    !function.foreign
        && !function.blocks.is_empty()
        && function.failure.is_none()
        && function.generics.is_empty()
        && matches!(function.linkage, Linkage::Module | Linkage::Private)
        && !referenced.contains(function.name.as_str())
}

fn is_vector(ty: &Type) -> bool {
    matches!(ty, Type::Vector(..))
}

/// The lanes a value of `ty` splits into, when it is a vector.
fn lanes(ty: &Type) -> Option<(u32, &Type)> {
    match ty {
        Type::Vector(count, element) => Some((*count, element)),
        _ => None,
    }
}

/// What a converted function returns in place of a vector: its lanes.
fn lane_tuple(ty: &Type) -> Type {
    match lanes(ty) {
        Some((count, element)) => Type::Tuple(vec![element.clone(); count as usize]),
        None => ty.clone(),
    }
}
