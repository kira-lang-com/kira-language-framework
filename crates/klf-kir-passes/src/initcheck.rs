//! Definite initialization (section 11.6 pass 1). A `load` and a `store [assign]`
//! require their stack slot to be initialized on every path; `store [init]`
//! requires it uninitialized. Violations are `init.use-before-init` and
//! `init.double-init`.

use crate::common::Cfg;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Function, Operation, StoreMode};
use std::collections::BTreeSet;

const USE_BEFORE_INIT: &str = "KLF-KIR-1103";
const DOUBLE_INIT: &str = "KLF-KIR-1104";

pub(crate) fn check_initialization(function: &Function, diagnostics: &mut Vec<Diagnostic>) {
    let slots: BTreeSet<Name> = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| matches!(instruction.operation, Operation::StackAlloc(_)))
        .flat_map(|instruction| instruction.results.iter().cloned())
        .collect();
    if slots.is_empty() {
        return;
    }

    let cfg = Cfg::build(function);
    let count = function.blocks.len();
    // A "must be initialized on every path" analysis: start optimistic (every
    // slot initialized) everywhere and shrink to a fixed point, so a loop
    // back-edge does not spuriously clear a slot initialized before the loop.
    let mut init_in: Vec<BTreeSet<Name>> = vec![slots.clone(); count];
    let mut init_out: Vec<BTreeSet<Name>> = vec![slots.clone(); count];

    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..count {
            let incoming = intersect_predecessors(&cfg, &init_out, block);
            let outgoing = transfer(&function.blocks[block], &incoming);
            if incoming != init_in[block] || outgoing != init_out[block] {
                init_in[block] = incoming;
                init_out[block] = outgoing;
                changed = true;
            }
        }
    }

    for (index, block) in function.blocks.iter().enumerate() {
        report(block, &slots, &init_in[index], diagnostics);
    }
}

fn intersect_predecessors(cfg: &Cfg, init_out: &[BTreeSet<Name>], block: usize) -> BTreeSet<Name> {
    let preds = &cfg.predecessors[block];
    let mut iter = preds.iter();
    let Some(&first) = iter.next() else {
        return BTreeSet::new();
    };
    let mut result = init_out[first].clone();
    for &pred in iter {
        result.retain(|slot| init_out[pred].contains(slot));
    }
    result
}

fn transfer(block: &klf_kir::Block, incoming: &BTreeSet<Name>) -> BTreeSet<Name> {
    let mut init = incoming.clone();
    for instruction in &block.instructions {
        if let Operation::Store { mode, address, .. } = &instruction.operation
            && matches!(mode, StoreMode::Init | StoreMode::Trivial)
        {
            init.insert(address.clone());
        }
    }
    init
}

fn report(
    block: &klf_kir::Block,
    slots: &BTreeSet<Name>,
    incoming: &BTreeSet<Name>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut init = incoming.clone();
    for instruction in &block.instructions {
        match &instruction.operation {
            Operation::Store { mode, address, .. } if slots.contains(address) => match mode {
                StoreMode::Init => {
                    if init.contains(address) {
                        diagnostics.push(
                            Diagnostic::error(
                                DOUBLE_INIT,
                                format!("`store [init]` to already-initialized `%{address}`"),
                            )
                            .with_label(instruction.span, "init.double-init"),
                        );
                    }
                    init.insert(address.clone());
                }
                StoreMode::Assign => {
                    if !init.contains(address) {
                        diagnostics.push(
                            Diagnostic::error(
                                USE_BEFORE_INIT,
                                format!("`store [assign]` to uninitialized `%{address}`"),
                            )
                            .with_label(instruction.span, "init.use-before-init"),
                        );
                    }
                }
                StoreMode::Trivial => {
                    init.insert(address.clone());
                }
            },
            Operation::Load { address, .. }
                if slots.contains(address) && !init.contains(address) =>
            {
                diagnostics.push(
                    Diagnostic::error(
                        USE_BEFORE_INIT,
                        format!("`load` of uninitialized `%{address}`"),
                    )
                    .with_label(instruction.span, "init.use-before-init"),
                );
            }
            _ => {}
        }
    }
}
