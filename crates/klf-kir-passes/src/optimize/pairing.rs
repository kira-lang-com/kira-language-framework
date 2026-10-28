//! Retain and release pairing (sections 14.1, O1, and 14.3): a `copy` of a
//! counted value that a `destroy` of the copy or of the original follows on
//! every path, with nothing between that could release or observe the
//! count, cancels against it. A path may continue through branches into
//! blocks that only that edge enters.

use super::Context;
use super::clean::pure;
use super::util::{block_positions, predecessors, rename_uses};
use klf_core::Name;
use klf_kir::{Function, ModuleIndex, Operation, Pairing, Type};
use rustc_hash::FxHashMap as HashMap;

/// Whether a `copy` of a value of `ty` is a retain the pass may pair: a
/// `ref T`, or a type whose policy says `pairing: counted`.
pub(super) fn counted(ty: &Type, index: &ModuleIndex<'_>) -> bool {
    match ty {
        Type::Reference(_) => true,
        Type::Nominal(name, _) => index
            .types
            .get(name.as_str())
            .is_some_and(|declaration| declaration.policy.pairing == Pairing::Counted),
        _ => false,
    }
}

/// Whether an instruction that uses the copy or the original leaves both
/// alive and the count unread.
pub(super) fn borrows(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Copy(_)
            | Operation::RefAddr(_)
            | Operation::BorrowBegin(_)
            | Operation::BorrowEnd(_)
            | Operation::RefUnowned(_)
    )
}

/// Whether an instruction that uses neither value can stand between a pair:
/// it cannot release anything nor read a count. Calls, `raw.store`, writes
/// through addresses, destroys of other values and `ref.isunique` cannot.
pub(super) fn passable(operation: &Operation) -> bool {
    pure(operation)
        || matches!(
            operation,
            Operation::Copy(_)
                | Operation::Move(_)
                | Operation::RefAddr(_)
                | Operation::BorrowBegin(_)
                | Operation::BorrowEnd(_)
                | Operation::RefUnowned(_)
                | Operation::WeakNew(_)
        )
}

/// Answers whether any pair cancelled.
pub(super) fn pair(function: &mut Function, context: &Context<'_>) -> bool {
    let copies = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| matches!(instruction.operation, Operation::Copy(_)));
    if !copies {
        return false;
    }
    let types = context.types(function);
    let positions = block_positions(function);
    let incoming = predecessors(function);
    let mut renames: HashMap<Name, Name> = HashMap::default();
    let mut changed = false;
    for block in 0..function.blocks.len() {
        let mut position = 0;
        while position < function.blocks[block].instructions.len() {
            let instruction = &function.blocks[block].instructions[position];
            let (Operation::Copy(original), [copy]) =
                (&instruction.operation, instruction.results.as_slice())
            else {
                position += 1;
                continue;
            };
            if !types
                .get(original)
                .is_some_and(|ty| counted(ty, context.index))
            {
                position += 1;
                continue;
            }
            let pair = Pair {
                original: original.clone(),
                copy: copy.clone(),
            };
            let paths = Paths {
                function,
                positions: &positions,
                incoming: &incoming,
            };
            let Some(mut releases) = paths.releases(&pair, block, position + 1, 0) else {
                position += 1;
                continue;
            };
            // The copy's uses read the original instead: alive until the
            // release when it was the copy's, after it when it was the
            // original's, whose last owner the copy's last use becomes.
            releases.sort_unstable_by(|left, right| right.cmp(left));
            for (at_block, at) in releases {
                function.blocks[at_block].instructions.remove(at);
            }
            function.blocks[block].instructions.remove(position);
            renames.insert(pair.copy, pair.original);
            changed = true;
        }
    }
    rename_uses(function, &renames);
    changed
}

struct Pair {
    original: Name,
    copy: Name,
}

/// The blocks a pair's release may follow it into.
struct Paths<'a> {
    function: &'a Function,
    positions: &'a HashMap<Name, usize>,
    incoming: &'a [Vec<usize>],
}

/// How many blocks past the copy's own a release is looked for.
const FARTHEST: usize = 4;

impl Paths<'_> {
    /// Where the `destroy` of the copy or the original that pairs with the
    /// copy sits on every path from `start` in `block`: only borrows of
    /// either and passable instructions come before it, and a path leaves a
    /// block only by a branch to a block no other edge enters.
    fn releases(
        &self,
        pair: &Pair,
        block: usize,
        start: usize,
        depth: usize,
    ) -> Option<Vec<(usize, usize)>> {
        let instructions = &self.function.blocks[block].instructions;
        for (offset, instruction) in instructions.iter().enumerate().skip(start) {
            if let Operation::Destroy(value) = &instruction.operation {
                if *value == pair.copy || *value == pair.original {
                    return Some(vec![(block, offset)]);
                }
                return None;
            }
            let touches = instruction
                .operation
                .used_values()
                .into_iter()
                .any(|used| *used == pair.copy || *used == pair.original);
            if instruction.operation.is_terminator() {
                return if touches || depth == FARTHEST || !branches(&instruction.operation) {
                    None
                } else {
                    self.through(pair, block, &instruction.operation, depth)
                };
            }
            if touches {
                if !borrows(&instruction.operation) {
                    return None;
                }
            } else if !passable(&instruction.operation) {
                return None;
            }
        }
        None
    }

    fn through(
        &self,
        pair: &Pair,
        block: usize,
        terminator: &Operation,
        depth: usize,
    ) -> Option<Vec<(usize, usize)>> {
        let successors = terminator.successors();
        if successors.is_empty() {
            return None;
        }
        let mut found = Vec::new();
        for name in successors {
            let &next = self.positions.get(name)?;
            if next == block || self.incoming[next].len() != 1 {
                return None;
            }
            found.extend(self.releases(pair, next, 0, depth + 1)?);
        }
        Some(found)
    }
}

/// Terminators that only pick an edge.
fn branches(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Branch(_)
            | Operation::ConditionalBranch(_)
            | Operation::SwitchInt(_)
            | Operation::SwitchEnum(_)
    )
}
