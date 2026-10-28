//! Destroy hoisting and copy sinking (sections 14.1, O2, and 14.3): a
//! `destroy` moves up to just after the last use of its value, and a `copy`
//! moves down to just before the first use of its result, across
//! instructions that can neither release a value, read a count, nor run a
//! copy or destroy function. A module whose types keep `lifetimes(scoped)`
//! keeps every destroy where the frontend put it.

use super::Context;
use super::clean::pure;
use klf_core::Name;
use klf_kir::{Function, Instruction, Operation};
use rustc_hash::FxHashSet as HashSet;

/// Answers whether anything moved.
pub(super) fn tighten(function: &mut Function, context: &Context<'_>) -> bool {
    let scoped = context.module.attributes.iter().any(|attribute| {
        attribute.name == "lifetimes" && attribute.value.as_deref() == Some("scoped")
    });
    let mut changed = false;
    for block in 0..function.blocks.len() {
        if !scoped {
            changed |= hoist_destroys(function, block);
        }
        changed |= sink_copies(function, block);
    }
    changed
}

/// Whether an instruction that uses none of the moved values may change
/// places with a `copy` or `destroy`.
fn crossable(operation: &Operation) -> bool {
    !operation.is_terminator()
        && (pure(operation)
            || matches!(
                operation,
                Operation::RefAddr(_)
                    | Operation::BorrowBegin(_)
                    | Operation::BorrowEnd(_)
                    | Operation::RefUnowned(_)
            ))
}

fn hoist_destroys(function: &mut Function, block: usize) -> bool {
    let mut changed = false;
    let mut position = 0;
    while position < function.blocks[block].instructions.len() {
        let Operation::Destroy(value) = &function.blocks[block].instructions[position].operation
        else {
            position += 1;
            continue;
        };
        let instructions = &function.blocks[block].instructions;
        let alone: HashSet<Name> = std::iter::once(value.clone()).collect();
        if position == 0 || !free_of(&instructions[position - 1], &alone) {
            position += 1;
            continue;
        }
        let tied = tied(function, value);
        let instructions = &mut function.blocks[block].instructions;
        let mut to = position;
        while to > 0 && free_of(&instructions[to - 1], &tied) {
            to -= 1;
        }
        if to < position {
            let destroy = instructions.remove(position);
            instructions.insert(to, destroy);
            changed = true;
        }
        position += 1;
    }
    changed
}

fn sink_copies(function: &mut Function, block: usize) -> bool {
    let mut changed = false;
    let mut position = function.blocks[block].instructions.len();
    while position > 0 {
        position -= 1;
        let instructions = &function.blocks[block].instructions;
        let (Operation::Copy(original), [copy]) = (
            &instructions[position].operation,
            instructions[position].results.as_slice(),
        ) else {
            continue;
        };
        let mut tied: HashSet<Name> = [original.clone(), copy.clone()].into_iter().collect();
        if position + 1 == instructions.len() || !free_of(&instructions[position + 1], &tied) {
            continue;
        }
        tied.extend(self::tied(function, original));
        let instructions = &mut function.blocks[block].instructions;
        let mut to = position;
        while to + 1 < instructions.len() && free_of(&instructions[to + 1], &tied) {
            to += 1;
        }
        if to > position {
            let moved = instructions.remove(position);
            instructions.insert(to, moved);
            changed = true;
        }
    }
    changed
}

fn free_of(instruction: &Instruction, tied: &HashSet<Name>) -> bool {
    crossable(&instruction.operation)
        && !instruction
            .operation
            .used_values()
            .into_iter()
            .chain(&instruction.results)
            .any(|name| tied.contains(name))
}

/// The value and everything that reaches into it: addresses, borrows and
/// unowned references made from it, and block parameters they flow into.
fn tied(function: &Function, value: &Name) -> HashSet<Name> {
    let mut tied: HashSet<Name> = std::iter::once(value.clone()).collect();
    loop {
        let before = tied.len();
        for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
            let reaches = matches!(
                instruction.operation,
                Operation::RefAddr(_)
                    | Operation::UniqAddr(_)
                    | Operation::BorrowBegin(_)
                    | Operation::LoadBorrow(_)
                    | Operation::RefUnowned(_)
                    | Operation::FieldAddr { .. }
                    | Operation::ElemAddr { .. }
                    | Operation::EnumPayloadAddr { .. }
                    | Operation::AddrCast { .. }
                    | Operation::AddrToPointer(_)
            );
            let uses = || instruction.operation.used_values().into_iter();
            if reaches && uses().any(|used| tied.contains(used)) {
                tied.extend(instruction.results.iter().cloned());
            }
            for target in instruction.operation.targets() {
                let Some(successor) = function
                    .blocks
                    .iter()
                    .find(|block| block.name == target.block)
                else {
                    continue;
                };
                for (argument, parameter) in target.arguments.iter().zip(&successor.parameters) {
                    if tied.contains(argument) {
                        tied.insert(parameter.name.clone());
                    }
                }
            }
        }
        if tied.len() == before {
            return tied;
        }
    }
}
