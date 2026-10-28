//! Helpers the optimization passes share: fresh names, renaming, use counts
//! and edge walks.

use klf_core::Name;
use klf_kir::{Block, Function, Instruction, Operation, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::Cell;

std::thread_local! {
    static FRESH: Cell<usize> = const { Cell::new(0) };
}

/// A name no frontend writes: frontends never put a `.` in a value or block
/// name, and the number is past every one the function holds since
/// [`start_fresh`].
pub(super) fn fresh(stem: &str) -> Name {
    let n = FRESH.get();
    FRESH.set(n + 1);
    Name::new(format!("o{n}.{stem}"))
}

/// Numbers this thread's fresh names past every fresh name `function`
/// holds, so the names a pass makes depend on the function alone and never
/// on what else the process optimized first.
pub(super) fn start_fresh(function: &Function) {
    let number = |name: &Name| -> usize {
        name.as_str()
            .strip_prefix('o')
            .and_then(|rest| rest.split_once('.'))
            .and_then(|(digits, _)| digits.parse::<usize>().ok())
            .map_or(0, |n| n + 1)
    };
    let mut next = function
        .parameters
        .iter()
        .map(|p| number(&p.name))
        .max()
        .unwrap_or(0);
    for block in &function.blocks {
        next = next.max(number(&block.name));
        for parameter in &block.parameters {
            next = next.max(number(&parameter.name));
        }
        for instruction in &block.instructions {
            for result in &instruction.results {
                next = next.max(number(result));
            }
        }
    }
    FRESH.set(next);
}

/// Applies `renames` to every operand of `function`, following chains.
pub(super) fn rename_uses(function: &mut Function, renames: &HashMap<Name, Name>) {
    if renames.is_empty() {
        return;
    }
    let resolve = |name: &Name| {
        let mut current = name;
        let mut steps = 0;
        while let Some(next) = renames.get(current) {
            current = next;
            steps += 1;
            if steps > renames.len() {
                break;
            }
        }
        current.clone()
    };
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            for used in instruction.operation.used_values_mut() {
                if renames.contains_key(used) {
                    *used = resolve(used);
                }
            }
        }
    }
}

/// How many times each value is used as an operand.
pub(super) fn use_counts(function: &Function) -> HashMap<Name, usize> {
    let mut counts: HashMap<Name, usize> = HashMap::default();
    for block in &function.blocks {
        for instruction in &block.instructions {
            for used in instruction.operation.used_values() {
                *counts.entry(used.clone()).or_default() += 1;
            }
        }
    }
    counts
}

pub(super) fn block_positions(function: &Function) -> HashMap<Name, usize> {
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(position, block)| (block.name.clone(), position))
        .collect()
}

/// Blocks something other than an edge names: `block.addr` and
/// `br.indirect`. Their parameters and edges cannot change.
pub(super) fn pinned_blocks(function: &Function) -> HashSet<Name> {
    let mut pinned = HashSet::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        match &instruction.operation {
            Operation::BlockAddress(block) => {
                pinned.insert(block.clone());
            }
            Operation::BranchIndirect { targets, .. } => pinned.extend(targets.iter().cloned()),
            _ => {}
        }
    }
    pinned
}

/// Whether the edges of this terminator hand their target values the edge
/// arguments do not list: a call's result or failure.
pub(super) fn has_implicit_arguments(operation: &Operation) -> bool {
    matches!(operation, Operation::TryCall(_) | Operation::Invoke(_))
}

/// Blocks reachable from the entry, by position, in reverse postorder.
pub(super) fn reverse_postorder(function: &Function) -> Vec<usize> {
    let positions = block_positions(function);
    let mut roots = vec![0];
    for name in pinned_blocks(function) {
        if let Some(&position) = positions.get(&name) {
            roots.push(position);
        }
    }
    let mut visited = vec![false; function.blocks.len()];
    let mut order = Vec::with_capacity(function.blocks.len());
    for root in roots {
        if function.blocks.is_empty() || visited[root] {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        visited[root] = true;
        while let Some((block, next)) = stack.pop() {
            let successors = successors_of(&function.blocks[block], &positions);
            if let Some(&successor) = successors.get(next) {
                stack.push((block, next + 1));
                if !visited[successor] {
                    visited[successor] = true;
                    stack.push((successor, 0));
                }
            } else {
                order.push(block);
            }
        }
    }
    order.reverse();
    order
}

pub(super) fn successors_of(block: &Block, positions: &HashMap<Name, usize>) -> Vec<usize> {
    block
        .instructions
        .last()
        .map(|terminator| {
            terminator
                .operation
                .successors()
                .into_iter()
                .filter_map(|name| positions.get(name).copied())
                .collect()
        })
        .unwrap_or_default()
}

/// Predecessor positions of every block, one entry per edge.
pub(super) fn predecessors(function: &Function) -> Vec<Vec<usize>> {
    let positions = block_positions(function);
    let mut predecessors = vec![Vec::new(); function.blocks.len()];
    for (position, block) in function.blocks.iter().enumerate() {
        for successor in successors_of(block, &positions) {
            predecessors[successor].push(position);
        }
    }
    predecessors
}

/// The types an optimization may keep in an SSA value instead of memory:
/// they copy and drop as bits.
pub(super) fn is_scalar(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Integer(_) | Type::Float(_) | Type::Pointer | Type::Address(_)
    )
}

pub(super) fn instruction(
    results: Vec<Name>,
    operation: Operation,
    like: &Instruction,
) -> Instruction {
    Instruction {
        results,
        operation,
        debug_location: None,
        span: like.span,
    }
}

pub(super) fn const_int(value: &Operation) -> Option<(u16, i128)> {
    match value {
        Operation::ConstInt {
            ty: Type::Integer(width),
            value,
        } => Some((*width, *value)),
        Operation::ConstBool(value) => Some((1, i128::from(*value))),
        _ => None,
    }
}

/// The constant each value is defined as, by name.
pub(super) fn constants(function: &Function) -> HashMap<Name, (u16, i128)> {
    let mut constants = HashMap::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let ([result], Some(constant)) = (
            instruction.results.as_slice(),
            const_int(&instruction.operation),
        ) {
            constants.insert(result.clone(), constant);
        }
    }
    constants
}
