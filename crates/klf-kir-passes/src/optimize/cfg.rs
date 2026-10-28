//! CFG simplification (section 14.1, O1): unreachable blocks go, blocks that
//! only branch on are skipped, and a block entered from one edge joins its
//! predecessor. Blocks end in reverse postorder, so every value is defined
//! before a backend that walks blocks in order meets a use.

use super::util::{
    block_positions, constants, fresh, has_implicit_arguments, instruction, pinned_blocks,
    predecessors, rename_uses, reverse_postorder, use_counts,
};
use klf_core::Name;
use klf_kir::{Function, Operation};
use rustc_hash::FxHashMap as HashMap;

/// Drops the blocks the entry cannot reach and orders the rest in reverse
/// postorder. Answers whether a block went.
pub(super) fn remove_unreachable(function: &mut Function) -> bool {
    if function.blocks.is_empty() {
        return false;
    }
    let order = reverse_postorder(function);
    let removed = order.len() != function.blocks.len();
    let mut blocks: Vec<Option<klf_kir::Block>> = std::mem::take(&mut function.blocks)
        .into_iter()
        .map(Some)
        .collect();
    function.blocks = order
        .into_iter()
        .filter_map(|position| blocks[position].take())
        .collect();
    removed
}

pub(super) fn simplify(function: &mut Function) -> bool {
    if function.blocks.is_empty() {
        return false;
    }
    let mut changed = fold_same_targets(function);
    changed |= skip_forwarding_blocks(function);
    changed |= pass_known_conditions(function);
    changed |= thread_constants(function);
    changed |= remove_unreachable(function);
    changed |= merge_chains(function);
    changed |= remove_unreachable(function);
    changed
}

fn fold_same_targets(function: &mut Function) -> bool {
    let mut changed = false;
    for block in &mut function.blocks {
        let Some(terminator) = block.instructions.last_mut() else {
            continue;
        };
        if let Operation::ConditionalBranch(branch) = &terminator.operation
            && branch.then_target == branch.else_target
        {
            terminator.operation = Operation::Branch(branch.then_target.clone());
            changed = true;
        }
    }
    changed
}

/// Points every edge into a block that holds only `br` at that branch's
/// target instead.
fn skip_forwarding_blocks(function: &mut Function) -> bool {
    let pinned = pinned_blocks(function);
    let incoming = predecessors(function);
    let uses = use_counts(function);
    let mut forwards: HashMap<Name, (Vec<Name>, klf_kir::BlockTarget)> = HashMap::default();
    for (position, block) in function.blocks.iter().enumerate().skip(1) {
        if block.instructions.len() != 1 || pinned.contains(&block.name) {
            continue;
        }
        let Operation::Branch(target) = &block.instructions[0].operation else {
            continue;
        };
        // A block it dominates may read a parameter directly.
        let read_elsewhere = block.parameters.iter().any(|parameter| {
            let here = target
                .arguments
                .iter()
                .filter(|argument| **argument == parameter.name)
                .count();
            uses.get(&parameter.name).copied().unwrap_or(0) != here
        });
        if target.block == block.name || read_elsewhere {
            continue;
        }
        let explicit_only = incoming[position].iter().all(|&from| {
            let terminator = &function.blocks[from]
                .instructions
                .last()
                .expect("a terminator")
                .operation;
            !has_implicit_arguments(terminator)
                && terminator
                    .targets()
                    .iter()
                    .filter(|edge| edge.block == block.name)
                    .all(|edge| edge.arguments.len() == block.parameters.len())
        });
        if explicit_only {
            let parameters = block
                .parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect();
            forwards.insert(block.name.clone(), (parameters, target.clone()));
        }
    }
    if forwards.is_empty() {
        return false;
    }
    let mut changed = false;
    for block in &mut function.blocks {
        let Some(terminator) = block.instructions.last_mut() else {
            continue;
        };
        if has_implicit_arguments(&terminator.operation) {
            continue;
        }
        for edge in terminator.operation.targets_mut() {
            let mut hops = 0;
            while let Some((parameters, onward)) = forwards.get(&edge.block) {
                if hops > forwards.len() {
                    break;
                }
                let bound: HashMap<&Name, &Name> = parameters.iter().zip(&edge.arguments).collect();
                let arguments = onward
                    .arguments
                    .iter()
                    .map(|argument| {
                        bound
                            .get(argument)
                            .map_or_else(|| argument.clone(), |&value| value.clone())
                    })
                    .collect();
                edge.block = onward.block.clone();
                edge.arguments = arguments;
                hops += 1;
                changed = true;
            }
        }
    }
    changed
}

/// Joins each block entered from a single edge to the block that branches
/// to it.
fn merge_chains(function: &mut Function) -> bool {
    let pinned = pinned_blocks(function);
    let incoming = predecessors(function);
    let positions = block_positions(function);
    let mut renames: HashMap<Name, Name> = HashMap::default();
    let mut gone = vec![false; function.blocks.len()];
    let mut changed = false;
    for position in 0..function.blocks.len() {
        if gone[position] {
            continue;
        }
        while let Some(Operation::Branch(target)) = function.blocks[position]
            .instructions
            .last()
            .map(|terminator| &terminator.operation)
        {
            let Some(&next) = positions.get(&target.block) else {
                break;
            };
            if next == 0
                || next == position
                || gone[next]
                || incoming[next].len() != 1
                || pinned.contains(&target.block)
                || function.blocks[next].parameters.len() != target.arguments.len()
            {
                break;
            }
            let arguments = target.arguments.clone();
            let joined = std::mem::take(&mut function.blocks[next].instructions);
            for (parameter, argument) in function.blocks[next].parameters.iter().zip(arguments) {
                renames.insert(parameter.name.clone(), argument);
            }
            let block = &mut function.blocks[position];
            block.instructions.pop();
            block.instructions.extend(joined);
            gone[next] = true;
            changed = true;
        }
    }
    if changed {
        let mut kept = Vec::with_capacity(function.blocks.len());
        for (position, block) in std::mem::take(&mut function.blocks).into_iter().enumerate() {
            if !gone[position] {
                kept.push(block);
            }
        }
        function.blocks = kept;
        rename_uses(function, &renames);
    }
    changed
}

/// Where a conditional branch hands its own condition to a target, the edge
/// passes the value the condition holds on it instead: true to the then
/// target, false to the else target. `a && b` reaches its join so, and the
/// constant lets the join be threaded.
fn pass_known_conditions(function: &mut Function) -> bool {
    let mut changed = false;
    for block in &mut function.blocks {
        let Some(terminator) = block.instructions.last() else {
            continue;
        };
        let Operation::ConditionalBranch(branch) = &terminator.operation else {
            continue;
        };
        let condition = branch.condition.clone();
        let passes = |target: &klf_kir::BlockTarget| target.arguments.contains(&condition);
        let (then_passes, else_passes) = (passes(&branch.then_target), passes(&branch.else_target));
        if !then_passes && !else_passes {
            continue;
        }
        let mut defined = Vec::new();
        let mut known = |value: bool, target: &mut klf_kir::BlockTarget| {
            let name = fresh(condition.as_str());
            for argument in &mut target.arguments {
                if *argument == condition {
                    *argument = name.clone();
                }
            }
            defined.push((name, value));
        };
        let last = block.instructions.len() - 1;
        let Operation::ConditionalBranch(branch) = &mut block.instructions[last].operation else {
            continue;
        };
        if then_passes {
            known(true, &mut branch.then_target);
        }
        if else_passes {
            known(false, &mut branch.else_target);
        }
        for (name, value) in defined {
            let constant = instruction(
                vec![name],
                Operation::ConstBool(value),
                &block.instructions[last],
            );
            block
                .instructions
                .insert(block.instructions.len() - 1, constant);
        }
        changed = true;
    }
    changed
}

/// The most instructions a block may hold to be copied for one edge.
const THREADED_SIZE: usize = 4;

/// Gives each forward edge that passes a constant into a small block ending
/// in a branch a copy of that block of its own, where the constant decides
/// the branch once folded. A block qualifies when nothing outside it reads
/// its parameters or results, so no other block depends on it.
fn thread_constants(function: &mut Function) -> bool {
    let pinned = pinned_blocks(function);
    let incoming = predecessors(function);
    let positions = block_positions(function);
    let known = constants(function);
    let uses = use_counts(function);
    let mut threaded: Vec<(usize, usize, usize)> = Vec::new();
    for (position, block) in function.blocks.iter().enumerate().skip(1) {
        let Some(terminator) = block.instructions.last() else {
            continue;
        };
        if block.parameters.is_empty()
            || block.instructions.len() > THREADED_SIZE
            || incoming[position].len() < 2
            || pinned.contains(&block.name)
            || !matches!(
                terminator.operation,
                Operation::ConditionalBranch(_) | Operation::SwitchInt(_)
            )
        {
            continue;
        }
        let mut inside: HashMap<&Name, usize> = HashMap::default();
        for instruction in &block.instructions {
            if !matches!(
                instruction.operation,
                Operation::ConstInt { .. }
                    | Operation::ConstBool(_)
                    | Operation::IntegerCompare { .. }
                    | Operation::Binary { .. }
                    | Operation::ConditionalBranch(_)
                    | Operation::SwitchInt(_)
            ) {
                inside.clear();
                break;
            }
            for used in instruction.operation.used_values() {
                *inside.entry(used).or_default() += 1;
            }
        }
        let local = |name: &Name| {
            uses.get(name).copied().unwrap_or(0) == inside.get(name).copied().unwrap_or(0)
        };
        let defined = block
            .parameters
            .iter()
            .map(|parameter| &parameter.name)
            .chain(
                block
                    .instructions
                    .iter()
                    .flat_map(|instruction| &instruction.results),
            );
        if inside.is_empty() || !defined.into_iter().all(local) {
            continue;
        }
        for &from in &incoming[position] {
            if from >= position {
                continue;
            }
            let Some(edges) = function.blocks[from].instructions.last() else {
                continue;
            };
            if has_implicit_arguments(&edges.operation) {
                continue;
            }
            for (at, edge) in edges.operation.targets().into_iter().enumerate() {
                if positions.get(&edge.block) == Some(&position)
                    && edge.arguments.len() == block.parameters.len()
                    && edge
                        .arguments
                        .iter()
                        .any(|argument| known.contains_key(argument))
                    && !threaded.contains(&(from, at, position))
                {
                    threaded.push((from, at, position));
                }
            }
        }
    }
    if threaded.is_empty() {
        return false;
    }
    for &(from, at, position) in &threaded {
        thread_edge(function, from, at, position);
    }
    true
}

/// Points edge `at` of `from` at a copy of block `position` of its own.
fn thread_edge(function: &mut Function, from: usize, at: usize, position: usize) {
    let original = &function.blocks[position];
    let mut renames: HashMap<Name, Name> = HashMap::default();
    let mut copied = original.clone();
    copied.name = fresh(original.name.as_str());
    for parameter in &mut copied.parameters {
        let copy = fresh(parameter.name.as_str());
        renames.insert(std::mem::replace(&mut parameter.name, copy.clone()), copy);
    }
    for instruction in &mut copied.instructions {
        for result in &mut instruction.results {
            let copy = fresh(result.as_str());
            renames.insert(std::mem::replace(result, copy.clone()), copy);
        }
        for used in instruction.operation.used_values_mut() {
            if let Some(renamed) = renames.get(used) {
                *used = renamed.clone();
            }
        }
    }
    let name = copied.name.clone();
    function.blocks.push(copied);
    if let Some(terminator) = function.blocks[from].instructions.last_mut() {
        terminator.operation.targets_mut()[at].block = name;
    }
}
