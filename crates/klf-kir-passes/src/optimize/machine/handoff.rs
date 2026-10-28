//! Hands values between the hot code and its overflow paths through stack
//! slots. A block parameter takes the register of every value its edges
//! pass, so one parameter a call on a slow path keeps alive drags the
//! loop's value onto the stack with it. Stored on a block of the edge's
//! own, the value is read where it is hot and nowhere else.

use super::super::util::{
    block_positions, fresh, has_implicit_arguments, instruction, is_scalar, pinned_blocks,
    predecessors, successors_of,
};
use super::edge::Edge;
use klf_core::Name;
use klf_kir::{Block, BlockTarget, Function, Instruction, LoadMode, Operation, StoreMode};
use rustc_hash::FxHashSet as HashSet;

/// Moves the values every edge between hot and cold blocks passes into
/// stack slots. A cold block is one only an overflow reaches.
pub(super) fn overflow_paths(function: &mut Function) -> bool {
    if function.blocks.len() < 2 {
        return false;
    }
    let hot = hot_blocks(function);
    if hot.iter().all(|&is_hot| is_hot) {
        return false;
    }
    let incoming = predecessors(function);
    let pinned = pinned_blocks(function);
    let implicit: Vec<bool> = function
        .blocks
        .iter()
        .map(|block| {
            block
                .instructions
                .last()
                .is_some_and(|terminator| has_implicit_arguments(&terminator.operation))
        })
        .collect();
    let mut changed = false;
    for (position, block) in function.blocks.clone().iter().enumerate().skip(1) {
        if block.parameters.is_empty()
            || pinned.contains(&block.name)
            || !block
                .parameters
                .iter()
                .all(|parameter| is_scalar(&parameter.ty))
        {
            continue;
        }
        let sources = &incoming[position];
        if sources.iter().any(|&from| implicit[from]) {
            continue;
        }
        let mut crossing: Vec<usize> = sources
            .iter()
            .copied()
            .filter(|&from| hot[from] != hot[position])
            .collect();
        crossing.sort_unstable();
        crossing.dedup();
        if crossing.is_empty() {
            continue;
        }
        // A cold block reads its slots itself, filled on every edge: a slot
        // one edge alone fills ahead of its read the backend reads as the
        // value stored. A hot block reads them only past a crossing edge.
        let every = !hot[position] || sources.iter().all(|from| crossing.contains(from));
        if every {
            crossing.clone_from(sources);
            crossing.sort_unstable();
            crossing.dedup();
        }
        hand_off(function, position, &crossing, every);
        changed = true;
    }
    changed
}

/// The blocks the entry reaches without taking an overflow.
fn hot_blocks(function: &Function) -> Vec<bool> {
    let positions = block_positions(function);
    let mut flags: HashSet<&Name> = HashSet::default();
    for current_instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let Operation::BinaryOverflow { .. } = current_instruction.operation {
            flags.extend(current_instruction.results.get(1));
        }
    }
    let mut hot = vec![false; function.blocks.len()];
    let mut work = vec![0];
    hot[0] = true;
    while let Some(position) = work.pop() {
        let block = &function.blocks[position];
        let overflow = match block.instructions.last().map(|last| &last.operation) {
            Some(Operation::ConditionalBranch(branch)) if flags.contains(&branch.condition) => {
                positions.get(&branch.then_target.block).copied()
            }
            _ => None,
        };
        for successor in successors_of(block, &positions) {
            if Some(successor) != overflow && !hot[successor] {
                hot[successor] = true;
                work.push(successor);
            }
        }
    }
    hot
}

/// Reads `position`'s parameters from slots the `crossing` edges fill: in
/// the block itself when `every` edge crosses, otherwise in a block of its
/// own the crossing edges enter.
fn hand_off(function: &mut Function, position: usize, crossing: &[usize], every: bool) {
    let like = function.blocks[position].instructions[0].clone();
    let target = function.blocks[position].name.clone();
    let types: Vec<_> = function.blocks[position]
        .parameters
        .iter()
        .map(|parameter| parameter.ty.clone())
        .collect();
    let slots: Vec<Name> = types.iter().map(|_| fresh("handed")).collect();
    let (landing, mut added) = if every {
        let parameters = std::mem::take(&mut function.blocks[position].parameters);
        let names = parameters.into_iter().map(|parameter| parameter.name);
        let loads = loads(names.collect(), &slots, &like);
        function.blocks[position].instructions.splice(0..0, loads);
        (target.clone(), Vec::new())
    } else {
        let names: Vec<Name> = slots.iter().map(|slot| fresh(slot.as_str())).collect();
        let mut instructions = loads(names.clone(), &slots, &like);
        instructions.push(branch(&target, names, &like));
        let landing = fresh(target.as_str());
        let block = Block {
            name: landing.clone(),
            parameters: Vec::new(),
            instructions,
            span: like.span,
        };
        (landing, vec![block])
    };
    for &from in crossing {
        let source = function.blocks[from].clone();
        let Some(terminator) = function.blocks[from].instructions.last_mut() else {
            continue;
        };
        for edge in terminator.operation.targets_mut() {
            if edge.block != target {
                continue;
            }
            let name = fresh(target.as_str());
            let mut rebuilt = Edge::new(&source, &target, &like);
            for (value, slot) in std::mem::take(&mut edge.arguments).into_iter().zip(&slots) {
                let store = Operation::Store {
                    mode: StoreMode::Trivial,
                    value: rebuilt.value(&value),
                    address: slot.clone(),
                };
                rebuilt
                    .instructions
                    .push(instruction(Vec::new(), store, &like));
            }
            let mut instructions = rebuilt.instructions;
            instructions.push(branch(&landing, Vec::new(), &like));
            edge.block = name.clone();
            added.push(Block {
                name,
                parameters: Vec::new(),
                instructions,
                span: like.span,
            });
        }
    }
    function.blocks.extend(added);
    let allocations = types
        .into_iter()
        .zip(slots)
        .map(|(ty, slot)| instruction(vec![slot], Operation::StackAlloc(ty), &like));
    let allocations: Vec<_> = allocations.collect();
    function.blocks[0].instructions.splice(0..0, allocations);
}

fn loads(names: Vec<Name>, slots: &[Name], like: &Instruction) -> Vec<Instruction> {
    names
        .into_iter()
        .zip(slots)
        .map(|(name, slot)| {
            let load = Operation::Load {
                mode: LoadMode::Trivial,
                address: slot.clone(),
            };
            instruction(vec![name], load, like)
        })
        .collect()
}

fn branch(target: &Name, arguments: Vec<Name>, like: &Instruction) -> Instruction {
    let target = BlockTarget {
        block: target.clone(),
        arguments,
    };
    instruction(Vec::new(), Operation::Branch(target), like)
}
