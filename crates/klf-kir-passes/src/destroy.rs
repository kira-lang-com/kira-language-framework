//! Destroy insertion (section 11.6 pass 4). For each owned value that is not
//! consumed on a path, insert a `destroy` where the value dies: before an exit
//! terminator, or on an edge into the block where its live range ends.

use crate::common::{Cfg, body_consumes, edge_consumes, is_owned, value_types};
use klf_core::Name;
use klf_kir::{Function, Instruction, ModuleIndex, Operation};
use klf_source::Span;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Where destroys will be inserted: at each block's start, and before each
/// block's terminator.
struct Plan {
    at_start: Vec<HashSet<Name>>,
    before_terminator: Vec<Vec<Name>>,
}

pub(crate) fn insert_destroys(function: &mut Function, index: &ModuleIndex<'_>) {
    let types = value_types(function, index);
    let owned = collect_owned(function, &types);
    if owned.is_empty() {
        return;
    }
    let cfg = Cfg::build(function);

    let mut plan = Plan {
        at_start: vec![HashSet::default(); function.blocks.len()],
        before_terminator: vec![Vec::new(); function.blocks.len()],
    };
    for value in &owned {
        plan_value(function, &cfg, value, &mut plan);
    }
    apply(function, &plan);
}

fn collect_owned(function: &Function, types: &HashMap<Name, klf_kir::Type>) -> Vec<Name> {
    let mut owned = Vec::new();
    for block in &function.blocks {
        for parameter in &block.parameters {
            if is_owned(types.get(&parameter.name)) {
                owned.push(parameter.name.clone());
            }
        }
        for instruction in &block.instructions {
            for result in &instruction.results {
                if is_owned(types.get(result)) {
                    owned.push(result.clone());
                }
            }
        }
    }
    owned
}

/// Backward liveness of `value`: whether it has a future use.
fn liveness(function: &Function, cfg: &Cfg, value: &Name) -> (Vec<bool>, Vec<bool>) {
    let count = function.blocks.len();
    let mut live_in = vec![false; count];
    let mut live_out = vec![false; count];
    let mut changed = true;
    while changed {
        changed = false;
        for block in (0..count).rev() {
            let out = cfg.successors[block]
                .iter()
                .any(|&successor| live_in[successor]);
            let new_in = block_live_in(&function.blocks[block].instructions, value, out);
            if out != live_out[block] || new_in != live_in[block] {
                live_out[block] = out;
                live_in[block] = new_in;
                changed = true;
            }
        }
    }
    (live_in, live_out)
}

fn block_live_in(instructions: &[Instruction], value: &Name, live_out: bool) -> bool {
    let mut live = live_out;
    for instruction in instructions.iter().rev() {
        if instruction.results.contains(value) {
            live = false;
        } else if uses(&instruction.operation, value) {
            live = true;
        }
    }
    live
}

fn uses(operation: &Operation, value: &Name) -> bool {
    operation.used_values().contains(&value)
        || operation
            .targets()
            .iter()
            .any(|target| target.arguments.contains(value))
}

/// Whether `value` is available (defined, unconsumed) at the end of each block's
/// body, and available on every edge into each block.
fn availability(function: &Function, cfg: &Cfg, value: &Name) -> (Vec<bool>, Vec<bool>) {
    let count = function.blocks.len();
    let mut body_out = vec![false; count];
    let mut all_in = vec![false; count];
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..count {
            let defined_here = block_defines(&function.blocks[block], value);
            let preds = &cfg.predecessors[block];
            let all = !preds.is_empty()
                && preds
                    .iter()
                    .all(|&pred| edge_available(&function.blocks[pred], value, block, function));
            let consumed = body_consumed(&function.blocks[block], value);
            let new_body_out = (defined_here || all) && !consumed;
            if new_body_out != body_out[block] || all != all_in[block] {
                body_out[block] = new_body_out;
                all_in[block] = all;
                changed = true;
            }
        }
    }
    (body_out, all_in)
}

fn block_defines(block: &klf_kir::Block, value: &Name) -> bool {
    block.parameters.iter().any(|p| &p.name == value)
        || block.instructions.iter().any(|i| i.results.contains(value))
}

fn body_consumed(block: &klf_kir::Block, value: &Name) -> bool {
    let instructions = &block.instructions;
    instructions
        .iter()
        .take(instructions.len().saturating_sub(1))
        .any(|i| body_consumes(&i.operation, value) > 0)
}

fn edge_available(pred: &klf_kir::Block, value: &Name, succ: usize, function: &Function) -> bool {
    // Availability out of pred's body, minus consumption on the specific edge.
    let defined = block_defines(pred, value);
    let body_ok = defined && !body_consumed(pred, value);
    if !body_ok {
        return false;
    }
    let Some(terminator) = pred.instructions.last() else {
        return false;
    };
    for target in terminator.operation.targets() {
        if function.blocks.iter().position(|b| b.name == target.block) == Some(succ) {
            return edge_consumes(target, value) == 0;
        }
    }
    false
}

fn plan_value(function: &Function, cfg: &Cfg, value: &Name, plan: &mut Plan) {
    let (live_in, _live_out) = liveness(function, cfg, value);
    let (body_out, all_in) = availability(function, cfg, value);

    for (block_index, block) in function.blocks.iter().enumerate() {
        // Only where the value actually lives; a value that merely flows in
        // available-but-dead was destroyed on the edge into this block.
        let relevant = block_defines(block, value) || live_in[block_index];
        if !relevant || !body_out[block_index] {
            continue;
        }
        let Some(terminator) = block.instructions.last() else {
            continue;
        };
        match &terminator.operation {
            Operation::Return(returned) => {
                if returned.as_ref() != Some(value) {
                    plan.before_terminator[block_index].push(value.clone());
                }
            }
            Operation::Fail(failed) => {
                if failed != value {
                    plan.before_terminator[block_index].push(value.clone());
                }
            }
            Operation::Unreachable => plan.before_terminator[block_index].push(value.clone()),
            Operation::Trap(_) => {}
            _ => plan_edges(function, cfg, value, block_index, &live_in, &all_in, plan),
        }
    }
}

fn plan_edges(
    function: &Function,
    cfg: &Cfg,
    value: &Name,
    block_index: usize,
    live_in: &[bool],
    all_in: &[bool],
    plan: &mut Plan,
) {
    let terminator = function.blocks[block_index].instructions.last().unwrap();
    let targets = terminator.operation.targets();
    let mut dying = Vec::new();
    let mut any_survives = false;
    for target in &targets {
        let successor = function
            .blocks
            .iter()
            .position(|b| b.name == target.block)
            .unwrap();
        if edge_consumes(target, value) > 0 {
            any_survives = true; // consumed (transferred) on this edge
        } else if live_in[successor] {
            any_survives = true; // flows on live; destroyed later
        } else {
            dying.push(successor); // available but dead on this edge
        }
    }
    if dying.is_empty() {
        return;
    }
    if !any_survives {
        plan.before_terminator[block_index].push(value.clone());
        return;
    }
    for successor in dying {
        let single_pred = cfg.predecessors[successor].len() == 1;
        if single_pred || all_in[successor] {
            plan.at_start[successor].insert(value.clone());
        }
        // A dying edge into a multi-predecessor block not available on every
        // edge is left for the verifier (V4) to flag; edge splitting will handle
        // it when the frontends produce such shapes.
    }
}

fn apply(function: &mut Function, plan: &Plan) {
    for (index, block) in function.blocks.iter_mut().enumerate() {
        let Some(span) = block
            .instructions
            .last()
            .map(|instruction| instruction.span)
        else {
            continue;
        };
        let mut instructions = Vec::with_capacity(block.instructions.len());
        for value in &plan.at_start[index] {
            instructions.push(destroy(value.clone(), span));
        }
        let terminator_index = block.instructions.len().saturating_sub(1);
        for (position, instruction) in block.instructions.drain(..).enumerate() {
            if position == terminator_index {
                for value in &plan.before_terminator[index] {
                    instructions.push(destroy(value.clone(), span));
                }
            }
            instructions.push(instruction);
        }
        block.instructions = instructions;
    }
}

fn destroy(value: Name, span: Span) -> Instruction {
    Instruction {
        results: Vec::new(),
        operation: Operation::Destroy(value),
        debug_location: None,
        span,
    }
}
