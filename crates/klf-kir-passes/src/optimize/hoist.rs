//! Loop-invariant code motion (section 14.1). An instruction inside a loop
//! that computes the same value on every iteration, from operands defined
//! before the loop, moves to the end of the block that enters the loop. Only
//! instructions that cannot trap and touch no memory move, so running one
//! where the loop would not have is harmless.

use super::loops::{dominates, dominators};
use super::util::{pinned_blocks, predecessors, reverse_postorder};
use klf_core::Name;
use klf_kir::{BinaryOperator, ConvertKind, Function, Instruction, Operation, Overflow};
use rustc_hash::FxHashSet as HashSet;

/// Moves every loop's invariant instructions into the block that enters it.
pub(super) fn hoist(function: &mut Function) -> bool {
    if function.blocks.is_empty() || !pinned_blocks(function).is_empty() {
        return false;
    }
    // Hoisting moves no terminator, so the edges stay as they were found.
    let incoming = predecessors(function);
    let mut changed = false;
    for (header, body) in loops(function, &incoming) {
        changed |= hoist_loop(function, &incoming, header, &body);
    }
    changed
}

/// Every natural loop, as its header's position and whether each block
/// position is in its body, given each block's `incoming` edges.
fn loops(function: &Function, incoming: &[Vec<usize>]) -> Vec<(usize, Vec<bool>)> {
    let order = reverse_postorder(function);
    let mut rank = vec![usize::MAX; function.blocks.len()];
    for (index, &position) in order.iter().enumerate() {
        rank[position] = index;
    }
    // The dominator walk needs the blocks in reverse postorder.
    let ranked: Vec<Vec<usize>> = order
        .iter()
        .map(|&position| {
            incoming[position]
                .iter()
                .map(|&from| rank[from])
                .filter(|&from| from != usize::MAX)
                .collect()
        })
        .collect();
    let idom = dominators(&ranked);
    let mut found = Vec::new();
    for (index, &header) in order.iter().enumerate() {
        let latches: Vec<usize> = ranked[index]
            .iter()
            .copied()
            .filter(|&from| dominates(&idom, index, from))
            .collect();
        if latches.is_empty() {
            continue;
        }
        let mut body = vec![false; function.blocks.len()];
        body[header] = true;
        let mut work: Vec<usize> = latches.iter().map(|&from| order[from]).collect();
        while let Some(block) = work.pop() {
            if !body[block] {
                body[block] = true;
                work.extend(incoming[block].iter().copied());
            }
        }
        found.push((header, body));
    }
    found
}

fn hoist_loop(
    function: &mut Function,
    incoming: &[Vec<usize>],
    header: usize,
    body: &[bool],
) -> bool {
    let outside: Vec<usize> = incoming[header]
        .iter()
        .copied()
        .filter(|&from| !body[from])
        .collect();
    let [entry] = outside[..] else {
        return false;
    };
    let enters_directly = function.blocks[entry]
        .instructions
        .last()
        .is_some_and(|terminator| matches!(terminator.operation, Operation::Branch(_)));
    if !enters_directly {
        return false;
    }
    let mut defined: HashSet<Name> = HashSet::default();
    for (position, block) in function.blocks.iter().enumerate() {
        if body[position] {
            defined.extend(
                block
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.clone()),
            );
            for instruction in &block.instructions {
                defined.extend(instruction.results.iter().cloned());
            }
        }
    }
    let mut moved: Vec<Instruction> = Vec::new();
    loop {
        let mut progress = false;
        for (block, _) in function
            .blocks
            .iter_mut()
            .zip(body)
            .filter(|(_, inside)| **inside)
        {
            let instructions = std::mem::take(&mut block.instructions);
            let mut kept = Vec::with_capacity(instructions.len());
            for instruction in instructions {
                let invariant = movable(&instruction.operation)
                    && !instruction.results.is_empty()
                    && instruction
                        .operation
                        .used_values()
                        .iter()
                        .all(|name| !defined.contains(*name));
                if invariant {
                    for result in &instruction.results {
                        defined.remove(result);
                    }
                    moved.push(instruction);
                    progress = true;
                } else {
                    kept.push(instruction);
                }
            }
            block.instructions = kept;
        }
        if !progress {
            break;
        }
    }
    if moved.is_empty() {
        return false;
    }
    let instructions = &mut function.blocks[entry].instructions;
    let terminator = instructions.len() - 1;
    instructions.splice(terminator..terminator, moved);
    true
}

/// Whether `operation` computes its result from its operands alone, without
/// trapping or touching memory.
fn movable(operation: &Operation) -> bool {
    match operation {
        Operation::ConstInt { .. }
        | Operation::ConstBool(_)
        | Operation::ConstFloat { .. }
        | Operation::ConstNull
        | Operation::IntegerCompare { .. }
        | Operation::IntegerUnary { .. }
        | Operation::FloatBinary { .. }
        | Operation::FloatCompare { .. }
        | Operation::FloatUnary { .. }
        | Operation::FloatFma { .. }
        | Operation::Select { .. }
        | Operation::PointerOffset { .. }
        | Operation::PointerDiff { .. }
        | Operation::TypeSize(_)
        | Operation::TypeAlign(_)
        | Operation::TypeStride(_) => true,
        Operation::Binary {
            operator, overflow, ..
        } => {
            matches!(overflow, Overflow::Wrap)
                && !matches!(
                    operator,
                    BinaryOperator::SignedDiv
                        | BinaryOperator::UnsignedDiv
                        | BinaryOperator::SignedRem
                        | BinaryOperator::UnsignedRem
                )
        }
        Operation::Convert { kind, .. } => !matches!(
            kind,
            ConvertKind::FloatToSigned | ConvertKind::FloatToUnsigned
        ),
        _ => false,
    }
}
