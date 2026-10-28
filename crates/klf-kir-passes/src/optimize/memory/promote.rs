//! Promotion of stack slots to SSA values (section 14.1, O2). A scalar slot
//! that is only loaded from and stored to becomes block parameters, placed
//! only where the slot is live, so neither backend sees its memory.

use super::super::util::{
    block_positions, fresh, instruction, is_scalar, pinned_blocks, predecessors, rename_uses,
    reverse_postorder, successors_of,
};
use klf_core::Name;
use klf_kir::{Function, Instruction, LoadMode, Operation, Parameter, Type};
use rustc_hash::FxHashMap as HashMap;

struct Slots {
    names: Vec<Name>,
    types: Vec<Type>,
    by_name: HashMap<Name, usize>,
}

fn promotable(function: &Function) -> Slots {
    let mut by_name: HashMap<Name, usize> = HashMap::default();
    let mut names = Vec::new();
    let mut types = Vec::new();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let ([result], Operation::StackAlloc(ty)) =
            (instruction.results.as_slice(), &instruction.operation)
            && is_scalar(ty)
        {
            by_name.insert(result.clone(), names.len());
            names.push(result.clone());
            types.push(ty.clone());
        }
    }
    let mut refused = vec![false; names.len()];
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        match &instruction.operation {
            Operation::Load {
                mode: LoadMode::Trivial,
                ..
            }
            | Operation::StackDealloc(_)
            | Operation::DebugAddr { .. }
            | Operation::StackAlloc(_) => {}
            Operation::Store { value, .. } => {
                if let Some(&slot) = by_name.get(value) {
                    refused[slot] = true;
                }
            }
            other => {
                for used in other.used_values() {
                    if let Some(&slot) = by_name.get(used) {
                        refused[slot] = true;
                    }
                }
            }
        }
    }
    let mut kept = Slots {
        names: Vec::new(),
        types: Vec::new(),
        by_name: HashMap::default(),
    };
    for (slot, name) in names.into_iter().enumerate() {
        if !refused[slot] {
            kept.by_name.insert(name.clone(), kept.names.len());
            kept.names.push(name);
            kept.types.push(types[slot].clone());
        }
    }
    kept
}

/// Per block, per slot: whether the block reads the slot before writing it,
/// and whether it writes it.
fn liveness(function: &Function, slots: &Slots) -> Vec<Vec<bool>> {
    let count = slots.names.len();
    let blocks = function.blocks.len();
    let mut exposed = vec![vec![false; count]; blocks];
    let mut written = vec![vec![false; count]; blocks];
    for (position, block) in function.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            match &instruction.operation {
                Operation::Load { address, .. } => {
                    if let Some(&slot) = slots.by_name.get(address)
                        && !written[position][slot]
                    {
                        exposed[position][slot] = true;
                    }
                }
                Operation::Store { address, .. } => {
                    if let Some(&slot) = slots.by_name.get(address) {
                        written[position][slot] = true;
                    }
                }
                _ => {}
            }
        }
    }
    let positions = block_positions(function);
    let successors: Vec<Vec<usize>> = function
        .blocks
        .iter()
        .map(|block| successors_of(block, &positions))
        .collect();
    let mut live_in = exposed.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for position in (0..blocks).rev() {
            for slot in 0..count {
                if live_in[position][slot] || written[position][slot] {
                    continue;
                }
                if successors[position]
                    .iter()
                    .any(|&successor| live_in[successor][slot])
                {
                    live_in[position][slot] = true;
                    changed = true;
                }
            }
        }
    }
    live_in
}

pub(in crate::optimize) fn promote(function: &mut Function) -> bool {
    if function.blocks.is_empty() || !pinned_blocks(function).is_empty() {
        return false;
    }
    super::super::cfg::remove_unreachable(function);
    let incoming = predecessors(function);
    if !incoming[0].is_empty() {
        return false;
    }
    let slots = promotable(function);
    if slots.names.is_empty() {
        return false;
    }
    let order = reverse_postorder(function);
    let live_in = liveness(function, &slots);
    let count = slots.names.len();

    let mut parameters: Vec<Vec<Option<Name>>> = vec![vec![None; count]; function.blocks.len()];
    for (position, block) in function.blocks.iter_mut().enumerate().skip(1) {
        for slot in 0..count {
            if live_in[position][slot] {
                let name = fresh("phi");
                block.parameters.push(Parameter {
                    name: name.clone(),
                    convention: None,
                    context: false,
                    ty: slots.types[slot].clone(),
                    span: block.span,
                });
                parameters[position][slot] = Some(name);
            }
        }
    }

    let mut zeros: Vec<Option<Name>> = vec![None; count];
    let mut zero_instructions: Vec<Instruction> = Vec::new();
    let mut renames: HashMap<Name, Name> = HashMap::default();
    let positions = block_positions(function);
    for &position in &order {
        let mut current: Vec<Option<Name>> = parameters[position].clone();
        let block = &mut function.blocks[position];
        let mut kept: Vec<Instruction> = Vec::with_capacity(block.instructions.len());
        for original in block.instructions.drain(..) {
            match &original.operation {
                Operation::StackAlloc(_) if slots.by_name.contains_key(&original.results[0]) => {}
                Operation::StackDealloc(address) | Operation::DebugAddr { address, .. }
                    if slots.by_name.contains_key(address) => {}
                Operation::Load { address, .. } if slots.by_name.contains_key(address) => {
                    let slot = slots.by_name[address];
                    let value = current[slot].clone().unwrap_or_else(|| {
                        zero(slot, &slots, &mut zeros, &mut zero_instructions, &original)
                    });
                    renames.insert(original.results[0].clone(), value);
                }
                Operation::Store { value, address, .. } if slots.by_name.contains_key(address) => {
                    let slot = slots.by_name[address];
                    let mut stored = value.clone();
                    while let Some(next) = renames.get(&stored) {
                        stored = next.clone();
                    }
                    current[slot] = Some(stored);
                }
                _ => kept.push(original),
            }
        }
        if let Some(terminator) = kept.last_mut() {
            let like = terminator.clone();
            for target in terminator.operation.targets_mut() {
                let Some(&successor) = positions.get(&target.block) else {
                    continue;
                };
                for slot in 0..count {
                    if parameters[successor][slot].is_some() {
                        let value = current[slot].clone().unwrap_or_else(|| {
                            zero(slot, &slots, &mut zeros, &mut zero_instructions, &like)
                        });
                        target.arguments.push(value);
                    }
                }
            }
        }
        block.instructions = kept;
    }
    function.blocks[0]
        .instructions
        .splice(0..0, zero_instructions);
    rename_uses(function, &renames);
    true
}

/// The value a slot holds before any store: a read there is of
/// uninitialized memory, which any value answers.
fn zero(
    slot: usize,
    slots: &Slots,
    zeros: &mut [Option<Name>],
    out: &mut Vec<Instruction>,
    like: &Instruction,
) -> Name {
    if let Some(name) = &zeros[slot] {
        return name.clone();
    }
    let name = fresh("undef");
    out.push(instruction(
        vec![name.clone()],
        Operation::ConstZero(slots.types[slot].clone()),
        like,
    ));
    zeros[slot] = Some(name.clone());
    name
}
