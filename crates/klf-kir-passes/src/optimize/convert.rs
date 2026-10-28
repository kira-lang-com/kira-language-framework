//! Borrow-to-copy and copy-to-borrow conversion (section 14.1, O2).
//!
//! A borrow of a scalar costs a scope and saves nothing: `load.borrow` of
//! one becomes `load [trivial]`, and `borrow.begin` of one goes, with their
//! `borrow.end`. A `copy` of a value whose type copies implicitly, when the
//! copy is only read and then destroyed in its block while the original is
//! only read meanwhile, becomes a borrow of the original: the copy and its
//! destroy go and the copy's reads read the original. A counted type's
//! copies are left to pairing, which knows what may read a count.

use super::Context;
use super::util::{is_scalar, rename_uses};
use klf_core::Name;
use klf_kir::{
    Convention, CopyPolicy, Function, Instruction, LoadMode, ModuleIndex, Operation, Pairing, Type,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Answers whether anything changed.
pub(super) fn convert(function: &mut Function, context: &Context<'_>) -> bool {
    let types = context.types(function);
    let scalar = |name: &Name| types.get(name).is_some_and(is_scalar);
    let mut ended: HashSet<Name> = HashSet::default();
    for instruction in function
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
    {
        match (&instruction.operation, instruction.results.as_slice()) {
            (Operation::LoadBorrow(address), [result]) if scalar(result) => {
                ended.insert(result.clone());
                instruction.operation = Operation::Load {
                    mode: LoadMode::Trivial,
                    address: address.clone(),
                };
            }
            (Operation::BorrowBegin(value), []) if scalar(value) => {
                ended.insert(value.clone());
                instruction.operation = Operation::Nop;
            }
            _ => {}
        }
    }
    let mut changed = !ended.is_empty();
    for block in &mut function.blocks {
        block
            .instructions
            .retain(|instruction| match &instruction.operation {
                Operation::BorrowEnd(value) => !ended.contains(value),
                Operation::Nop => false,
                _ => true,
            });
    }
    let implicit =
        |name: &Name| match types.get(name) {
            Some(Type::Nominal(declared, _)) => context
                .index
                .types
                .get(declared.as_str())
                .is_some_and(|declaration| {
                    matches!(declaration.policy.copy, CopyPolicy::Implicit(_))
                        && declaration.policy.pairing != Pairing::Counted
                }),
            _ => false,
        };
    let mut renames: HashMap<Name, Name> = HashMap::default();
    for block in &mut function.blocks {
        let mut position = 0;
        while position < block.instructions.len() {
            let instruction = &block.instructions[position];
            let (Operation::Copy(original), [copy]) =
                (&instruction.operation, instruction.results.as_slice())
            else {
                position += 1;
                continue;
            };
            if !implicit(original) {
                position += 1;
                continue;
            }
            let Some(end) =
                read_until_destroyed(&block.instructions, position, original, copy, context.index)
            else {
                position += 1;
                continue;
            };
            renames.insert(copy.clone(), original.clone());
            block.instructions.remove(end);
            block.instructions.remove(position);
            changed = true;
        }
    }
    rename_uses(function, &renames);
    changed
}

/// The position of the `destroy` of `copy` when every instruction between
/// it and the copy at `at` uses the copy and the original only to read
/// them, and the block's later instructions use the copy no more.
fn read_until_destroyed(
    instructions: &[Instruction],
    at: usize,
    original: &Name,
    copy: &Name,
    index: &ModuleIndex<'_>,
) -> Option<usize> {
    for (offset, instruction) in instructions.iter().enumerate().skip(at + 1) {
        if matches!(&instruction.operation, Operation::Destroy(value) if value == copy) {
            return Some(offset);
        }
        let touches = instruction
            .operation
            .used_values()
            .into_iter()
            .any(|used| used == copy || used == original);
        if instruction.operation.is_terminator()
            || (touches && !reads(&instruction.operation, [original, copy], index))
        {
            return None;
        }
    }
    None
}

/// Whether the instruction leaves both values as they were: it copies
/// them, opens or closes a borrow of them, or passes them to parameters a
/// known callee borrows.
fn reads(operation: &Operation, values: [&Name; 2], index: &ModuleIndex<'_>) -> bool {
    match operation {
        Operation::Copy(_) | Operation::BorrowBegin(_) | Operation::BorrowEnd(_) => true,
        Operation::Call {
            callee, arguments, ..
        } => index
            .functions
            .get(callee.as_str())
            .is_some_and(|function| {
                arguments.iter().enumerate().all(|(position, argument)| {
                    !values.contains(&argument)
                        || function.parameters.get(position).is_some_and(|parameter| {
                            parameter.convention == Some(Convention::Borrow)
                        })
                })
            }),
        _ => false,
    }
}
