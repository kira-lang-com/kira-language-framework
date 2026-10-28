//! What a frame owns before each instruction, for the cleanup pads of
//! definition code (`KLF-EDITOR.md` section 8.6).
//!
//! Ownership is the verifier's rule V4: a value of `ref` or `uniq` type is
//! owned from its definition until something consumes it: `move`,
//! `destroy`, `store`, `fail`, `return`, the arguments of a `call` and of a
//! branch. A value counts only when it is owned on every path to the point,
//! so a pad never releases what another path already gave away.

use klf_core::Name;
use klf_kir::{Function, Instruction, Operation, Type, ValueTypes};
use rustc_hash::FxHashMap as HashMap;
use std::collections::BTreeSet;

/// How a pad gives up an owned value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Release {
    Counted,
    Unique,
}

pub(crate) fn release_of(ty: Option<&Type>) -> Option<Release> {
    match ty {
        Some(Type::Reference(_)) => Some(Release::Counted),
        Some(Type::Unique(_)) => Some(Release::Unique),
        _ => None,
    }
}

/// The owned values at the start of each block, by block index; empty for a
/// block no path reaches.
pub(crate) fn block_entries(function: &Function, types: &ValueTypes) -> Vec<BTreeSet<Name>> {
    let positions: HashMap<&Name, usize> = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (&block.name, index))
        .collect();
    let mut entries: Vec<Option<BTreeSet<Name>>> = vec![None; function.blocks.len()];
    let Some(first) = function.blocks.first() else {
        return Vec::new();
    };
    entries[0] = Some(
        first
            .parameters
            .iter()
            .filter(|parameter| release_of(Some(&parameter.ty)).is_some())
            .map(|parameter| parameter.name.clone())
            .collect(),
    );
    let mut work = vec![0];
    while let Some(index) = work.pop() {
        let mut owned = entries[index].clone().unwrap_or_default();
        let block = &function.blocks[index];
        for instruction in &block.instructions {
            step(&mut owned, instruction, types);
        }
        let Some(last) = block.instructions.last() else {
            continue;
        };
        for target in last.operation.targets() {
            let Some(&successor) = positions.get(&target.block) else {
                continue;
            };
            let mut out = owned.clone();
            for argument in &target.arguments {
                out.remove(argument);
            }
            for (parameter, argument) in function.blocks[successor]
                .parameters
                .iter()
                .zip(&target.arguments)
            {
                if owned.contains(argument) && release_of(Some(&parameter.ty)).is_some() {
                    out.insert(parameter.name.clone());
                }
            }
            let changed = match &mut entries[successor] {
                Some(existing) => {
                    let before = existing.len();
                    existing.retain(|name| out.contains(name));
                    existing.len() != before
                }
                slot @ None => {
                    *slot = Some(out);
                    true
                }
            };
            if changed {
                work.push(successor);
            }
        }
    }
    entries.into_iter().map(Option::unwrap_or_default).collect()
}

/// The values `operation` consumes.
pub(crate) fn consumed(operation: &Operation) -> &[Name] {
    match operation {
        Operation::Move(name)
        | Operation::Destroy(name)
        | Operation::Fail(name)
        | Operation::Store { value: name, .. }
        | Operation::Return(Some(name)) => std::slice::from_ref(name),
        Operation::Call { arguments, .. } => arguments,
        _ => &[],
    }
}

/// Moves `owned` past `instruction`.
pub(crate) fn step(owned: &mut BTreeSet<Name>, instruction: &Instruction, types: &ValueTypes) {
    let operation = &instruction.operation;
    let moved = match operation {
        Operation::Move(name) => owned.contains(name),
        _ => true,
    };
    for name in consumed(operation) {
        owned.remove(name);
    }
    let produces = !matches!(
        operation,
        Operation::BorrowBegin(_) | Operation::LoadBorrow(_) | Operation::RefUnowned(_)
    ) && moved;
    if produces {
        for result in &instruction.results {
            if release_of(types.get(result)).is_some() {
                owned.insert(result.clone());
            }
        }
    }
}
