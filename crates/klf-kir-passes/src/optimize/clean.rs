//! Dead code elimination (section 14.1, O1): pure instructions no one reads,
//! stack slots no one reads, block parameters every edge gives the same
//! value, and block parameters only other dead parameters read.

use super::Context;
use super::util::{
    block_positions, constants, has_implicit_arguments, instruction, is_scalar, pinned_blocks,
    rename_uses, use_counts,
};
use klf_core::Name;
use klf_kir::{
    BinaryOperator, ConvertKind, Function, Instruction, LoadMode, Operation, Overflow, StoreMode,
    Type,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Whether dropping the instruction, when nothing reads its results, leaves
/// behavior unchanged: it neither writes, traps, consumes nor calls.
pub(super) fn pure(operation: &Operation) -> bool {
    match operation {
        Operation::ConstInt { .. }
        | Operation::ConstBool(_)
        | Operation::ConstNull
        | Operation::ConstFloat { .. }
        | Operation::ConstZero(_)
        | Operation::FunctionReference(_)
        | Operation::GlobalAddress { .. }
        | Operation::DescriptorReference(_)
        | Operation::WitnessReference { .. }
        | Operation::IntegerUnary { .. }
        | Operation::IntegerCompare { .. }
        | Operation::Select { .. }
        | Operation::FloatBinary { .. }
        | Operation::FloatCompare { .. }
        | Operation::FloatUnary { .. }
        | Operation::FloatFma { .. }
        | Operation::FieldAddr { .. }
        | Operation::ElemAddr { .. }
        | Operation::EnumPayloadAddr { .. }
        | Operation::AddrCast { .. }
        | Operation::AddrToPointer(_)
        | Operation::PointerOffset { .. }
        | Operation::PointerDiff { .. }
        | Operation::StackAlloc(_)
        | Operation::TypeSize(_)
        | Operation::TypeAlign(_)
        | Operation::TypeStride(_)
        | Operation::Nop => true,
        Operation::Load { mode, .. } => *mode == LoadMode::Trivial,
        Operation::Convert { kind, .. } => !matches!(
            kind,
            ConvertKind::FloatToSigned | ConvertKind::FloatToUnsigned
        ),
        Operation::Binary {
            operator, overflow, ..
        } => {
            *overflow != Overflow::Trap
                && !matches!(
                    operator,
                    BinaryOperator::SignedDiv
                        | BinaryOperator::UnsignedDiv
                        | BinaryOperator::SignedRem
                        | BinaryOperator::UnsignedRem
                )
        }
        _ => false,
    }
}

/// Removes pure instructions whose results nothing reads, repeatedly.
pub(super) fn remove_dead(function: &mut Function, context: &Context<'_>) -> bool {
    let mut changed = false;
    // Removing an instruction changes no other value's type and only lowers
    // the counts of what it used, so both are found once.
    let types = context.types(function);
    let mut counts = use_counts(function);
    loop {
        let mut removed = false;
        let mut stores_only: HashSet<Name> = HashSet::default();
        dead_slots(function, &types, &mut stores_only);
        let mut released: Vec<Name> = Vec::new();
        for block in &mut function.blocks {
            let before = block.instructions.len();
            block.instructions.retain(|instruction| {
                let kept = keeps(instruction, &counts, &stores_only);
                if !kept {
                    released.extend(instruction.operation.used_values().into_iter().cloned());
                }
                kept
            });
            removed |= block.instructions.len() != before;
        }
        for name in released {
            if let Some(count) = counts.get_mut(&name) {
                *count -= 1;
                if *count == 0 {
                    counts.remove(&name);
                }
            }
        }
        if !removed {
            return changed;
        }
        changed = true;
    }
}

/// Whether `remove_dead` keeps `instruction`, given how many times each
/// value is used and the slots only ever written.
fn keeps(
    instruction: &Instruction,
    counts: &HashMap<Name, usize>,
    stores_only: &HashSet<Name>,
) -> bool {
    if let Operation::Store { address, .. } = &instruction.operation
        && stores_only.contains(address)
    {
        return false;
    }
    if let Operation::StackDealloc(address)
    | Operation::DebugAddr { address, .. }
    | Operation::EnumInject { address, .. } = &instruction.operation
        && stores_only.contains(address)
    {
        return false;
    }
    if matches!(instruction.operation, Operation::Nop) {
        return false;
    }
    !(pure(&instruction.operation)
        && instruction
            .results
            .iter()
            .all(|result| !counts.contains_key(result) || stores_only.contains(result)))
}

/// Stack slots nothing reads, with every address derived from them: their
/// writes and the slots themselves can go. A write that destroys an old
/// value keeps its slot.
fn dead_slots(function: &Function, types: &klf_kir::ValueTypes, out: &mut HashSet<Name>) {
    let mut roots: HashMap<Name, Name> = HashMap::default();
    let mut live: HashSet<Name> = HashSet::default();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        let root_of = |name: &Name| roots.get(name).cloned();
        match &instruction.operation {
            Operation::StackAlloc(_) => {
                if let [result] = instruction.results.as_slice() {
                    roots.insert(result.clone(), result.clone());
                }
            }
            Operation::FieldAddr { base, .. }
            | Operation::EnumPayloadAddr { base, .. }
            | Operation::AddrCast { address: base, .. } => {
                if let Some(root) = root_of(base) {
                    roots.insert(instruction.results[0].clone(), root);
                }
            }
            Operation::Store {
                mode,
                value,
                address,
            } => {
                live.extend(root_of(value));
                let plain = *mode == StoreMode::Trivial || types.get(value).is_some_and(is_scalar);
                if !plain {
                    live.extend(root_of(address));
                }
            }
            Operation::EnumInject { .. }
            | Operation::StackDealloc(_)
            | Operation::DebugAddr { .. } => {}
            other => {
                for used in other.used_values() {
                    live.extend(root_of(used));
                }
            }
        }
    }
    out.extend(
        roots
            .into_iter()
            .filter(|(_, root)| !live.contains(root))
            .map(|(name, _)| name),
    );
}

/// The arguments of every edge into each block, by the block's position.
fn incoming_arguments<'a>(
    function: &'a Function,
    positions: &HashMap<Name, usize>,
) -> Vec<Vec<&'a [Name]>> {
    let mut edges: Vec<Vec<&[Name]>> = vec![Vec::new(); function.blocks.len()];
    for block in &function.blocks {
        let Some(terminator) = block.instructions.last() else {
            continue;
        };
        for edge in terminator.operation.targets() {
            if let Some(&target) = positions.get(&edge.block) {
                edges[target].push(edge.arguments.as_slice());
            }
        }
    }
    edges
}

/// The explicit argument an edge gives parameter `parameter` of a block with
/// `count` parameters, when the edge gives it explicitly.
fn argument_for(arguments: &[Name], count: usize, parameter: usize) -> Option<&Name> {
    let implicit = count.checked_sub(arguments.len())?;
    parameter
        .checked_sub(implicit)
        .and_then(|at| arguments.get(at))
}

/// Removes block parameters every edge gives one value, other than the
/// parameter itself, and those only dead parameters read.
pub(super) fn remove_parameters(function: &mut Function) -> bool {
    if function.blocks.len() < 2 {
        return false;
    }
    if function.blocks[1..].iter().all(|block| block.parameters.is_empty()) {
        return false;
    }
    let pinned = pinned_blocks(function);
    let mut changed = false;
    loop {
        let positions = block_positions(function);
        let mut renames: HashMap<Name, Name> = HashMap::default();
        let mut drop: Vec<HashSet<usize>> = vec![HashSet::default(); function.blocks.len()];
        let known = constants(function);
        let mut defined: Vec<(usize, Name, u16, i128)> = Vec::new();
        let implicit_targets = implicit_targets(function, &positions);
        let edges = incoming_arguments(function, &positions);
        for (position, block) in function.blocks.iter().enumerate().skip(1) {
            if pinned.contains(&block.name)
                || implicit_targets.contains(&position)
                || block.parameters.is_empty()
            {
                continue;
            }
            let count = block.parameters.len();
            for (index, parameter) in block.parameters.iter().enumerate() {
                let mut only: Option<&Name> = None;
                let mut single = true;
                let mut constant: Option<Option<(u16, i128)>> = None;
                for &arguments in &edges[position] {
                    let argument = argument_for(arguments, count, index);
                    match argument {
                        Some(value) if *value == parameter.name => {}
                        Some(value) if only.is_none_or(|seen| seen == value) => {
                            only = Some(value);
                        }
                        _ => single = false,
                    }
                    if argument != Some(&parameter.name) {
                        let given = argument.and_then(|value| known.get(value)).copied();
                        if constant.is_none_or(|seen| seen == given) {
                            constant = Some(given);
                        } else {
                            constant = Some(None);
                        }
                    }
                }
                if single && let Some(value) = only {
                    renames.insert(parameter.name.clone(), value.clone());
                    drop[position].insert(index);
                } else if let Some(Some((bits, value))) = constant
                    && parameter.ty == Type::Integer(bits)
                {
                    // Every edge gives one constant under different names:
                    // the block defines it itself.
                    defined.push((position, parameter.name.clone(), bits, value));
                    drop[position].insert(index);
                }
            }
        }
        dead_parameters(function, &pinned, &implicit_targets, &renames, &mut drop);
        if drop.iter().all(HashSet::is_empty) {
            return changed;
        }
        drop_parameters(function, &drop, &positions);
        for (position, name, bits, value) in defined {
            let operation = if bits == 1 {
                Operation::ConstBool(value != 0)
            } else {
                Operation::ConstInt {
                    ty: Type::Integer(bits),
                    value,
                }
            };
            let block = &mut function.blocks[position];
            let defined = instruction(vec![name], operation, &block.instructions[0]);
            block.instructions.insert(0, defined);
        }
        rename_uses(function, &renames);
        changed = true;
    }
}

fn implicit_targets(function: &Function, positions: &HashMap<Name, usize>) -> HashSet<usize> {
    let mut targets = HashSet::default();
    for block in &function.blocks {
        if let Some(terminator) = block.instructions.last()
            && has_implicit_arguments(&terminator.operation)
        {
            for name in terminator.operation.successors() {
                if let Some(&position) = positions.get(name) {
                    targets.insert(position);
                }
            }
        }
    }
    targets
}

/// Marks the parameters read only as edge arguments to parameters that are
/// themselves dead.
fn dead_parameters(
    function: &Function,
    pinned: &HashSet<Name>,
    implicit: &HashSet<usize>,
    renames: &HashMap<Name, Name>,
    drop: &mut [HashSet<usize>],
) {
    let mut owner: HashMap<&Name, (usize, usize)> = HashMap::default();
    for (position, block) in function.blocks.iter().enumerate().skip(1) {
        if pinned.contains(&block.name) || implicit.contains(&position) {
            continue;
        }
        for (index, parameter) in block.parameters.iter().enumerate() {
            if !drop[position].contains(&index) {
                owner.insert(&parameter.name, (position, index));
            }
        }
    }
    let positions = block_positions(function);
    let edges = incoming_arguments(function, &positions);
    let mut live: HashSet<&Name> = HashSet::default();
    let mut work: Vec<&Name> = Vec::new();
    // Uses outside edge arguments keep a value live.
    for block in &function.blocks {
        for instruction in &block.instructions {
            let edge_arguments: HashSet<*const Name> = instruction
                .operation
                .targets()
                .into_iter()
                .flat_map(|edge| edge.arguments.iter())
                .map(std::ptr::from_ref)
                .collect();
            let uses = instruction
                .operation
                .used_values()
                .into_iter()
                .filter(|used| {
                    edge_arguments.is_empty()
                        || !edge_arguments.contains(&std::ptr::from_ref(*used))
                });
            for used in uses {
                if let Some(name) = owner.get_key_value(used).map(|(name, _)| *name)
                    && live.insert(name)
                {
                    work.push(name);
                }
            }
        }
    }
    for value in renames.values() {
        if let Some(name) = owner.get_key_value(value).map(|(name, _)| *name)
            && live.insert(name)
        {
            work.push(name);
        }
    }
    // A live parameter keeps live what every edge gives it.
    while let Some(name) = work.pop() {
        let (position, index) = owner[name];
        let block = &function.blocks[position];
        let count = block.parameters.len();
        for &arguments in &edges[position] {
            if let Some(value) = argument_for(arguments, count, index)
                && let Some(argument) = owner.get_key_value(value).map(|(name, _)| *name)
                && live.insert(argument)
            {
                work.push(argument);
            }
        }
    }
    for (name, &(position, index)) in &owner {
        if !live.contains(name) {
            drop[position].insert(index);
        }
    }
}

fn drop_parameters(
    function: &mut Function,
    drop: &[HashSet<usize>],
    positions: &HashMap<Name, usize>,
) {
    let counts: Vec<usize> = function
        .blocks
        .iter()
        .map(|block| block.parameters.len())
        .collect();
    for block in &mut function.blocks {
        let Some(terminator) = block.instructions.last_mut() else {
            continue;
        };
        for edge in terminator.operation.targets_mut() {
            let Some(&target) = positions.get(&edge.block) else {
                continue;
            };
            if drop[target].is_empty() {
                continue;
            }
            let implicit = counts[target] - edge.arguments.len();
            let mut index = implicit;
            edge.arguments.retain(|_| {
                let keep = !drop[target].contains(&index);
                index += 1;
                keep
            });
        }
    }
    for (position, block) in function.blocks.iter_mut().enumerate() {
        if drop[position].is_empty() {
            continue;
        }
        let mut index = 0;
        block.parameters.retain(|_| {
            let keep = !drop[position].contains(&index);
            index += 1;
            keep
        });
    }
}
