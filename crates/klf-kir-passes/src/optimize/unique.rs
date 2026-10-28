//! Uniqueness-check elision (section 14.1, O2). `ref.isunique` on a counted
//! object allocated on every path to it, which nothing on any of those
//! paths has been able to copy since, is known to answer true: the
//! allocation's strong count is one and only `ref.addr` has seen the
//! reference.

use super::util;
use klf_core::Name;
use klf_kir::{Block, Function, Operation};
use rustc_hash::FxHashSet as HashSet;

/// Replaces the uniqueness checks of fresh allocations with `true`.
pub(super) fn elide(function: &mut Function) -> bool {
    let entering = fresh_on_entry(function);
    let mut changed = false;
    for (block, fresh) in function.blocks.iter_mut().zip(entering) {
        let mut fresh = fresh.unwrap_or_default();
        for instruction in &mut block.instructions {
            if let Operation::RefIsUnique(value) = &instruction.operation
                && fresh.contains(value)
            {
                instruction.operation = Operation::ConstBool(true);
                changed = true;
                continue;
            }
            step(&mut fresh, &instruction.operation, &instruction.results);
        }
    }
    changed
}

/// The allocations still fresh as each block starts: those fresh at the
/// end of every predecessor. A block the entry cannot reach has none.
fn fresh_on_entry(function: &Function) -> Vec<Option<HashSet<Name>>> {
    let order = util::reverse_postorder(function);
    let predecessors = util::predecessors(function);
    let positions = util::block_positions(function);
    let mut roots = vec![false; function.blocks.len()];
    for name in util::pinned_blocks(function) {
        if let Some(&position) = positions.get(&name) {
            roots[position] = true;
        }
    }
    if let Some(entry) = roots.first_mut() {
        *entry = true;
    }
    // `None` is a block not yet reached, which constrains nothing.
    let mut entering: Vec<Option<HashSet<Name>>> = vec![None; function.blocks.len()];
    let mut leaving: Vec<Option<HashSet<Name>>> = vec![None; function.blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for &position in &order {
            let fresh = if roots[position] {
                HashSet::default()
            } else {
                let mut known = predecessors[position]
                    .iter()
                    .filter_map(|&predecessor| leaving[predecessor].as_ref());
                let Some(first) = known.next() else {
                    continue;
                };
                let mut fresh = first.clone();
                for other in known {
                    fresh.retain(|name| other.contains(name));
                }
                fresh
            };
            entering[position] = Some(fresh.clone());
            let out = through(&function.blocks[position], fresh);
            if leaving[position].as_ref() != Some(&out) {
                leaving[position] = Some(out);
                changed = true;
            }
        }
    }
    entering
}

fn through(block: &Block, mut fresh: HashSet<Name>) -> HashSet<Name> {
    for instruction in &block.instructions {
        step(&mut fresh, &instruction.operation, &instruction.results);
    }
    fresh
}

/// An allocation is fresh from where it is made until anything but
/// `ref.addr` or `ref.isunique` uses it.
fn step(fresh: &mut HashSet<Name>, operation: &Operation, results: &[Name]) {
    match operation {
        Operation::HeapAllocCounted(_) => fresh.extend(results.iter().cloned()),
        Operation::RefAddr(_) | Operation::RefIsUnique(_) => {}
        operation => {
            for used in operation.used_values() {
                fresh.remove(used);
            }
        }
    }
}
