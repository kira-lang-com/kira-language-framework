//! Loop versioning (section 14.1, O2). A loop whose header parameter enters
//! as a constant and keeps it on the paths that usually run, as a whole
//! `Number`'s scale stays zero, is specialized for that constant. Every edge
//! that may give the parameter another value goes to an unspecialized copy
//! of the loop instead, so the specialized loop folds what tests the value
//! and keeps nothing of the paths that leave it.

use super::Context;
use super::util::{
    block_positions, constants, fresh, has_implicit_arguments, instruction, is_scalar,
    pinned_blocks, predecessors,
};
use klf_core::Name;
use klf_kir::{Block, Function, Instruction, LoadMode, Operation, StoreMode, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// The most instructions a loop may hold to be copied.
const LARGEST_LOOP: usize = 1_500;

/// The most loops one function specializes.
const MOST_VERSIONS: usize = 8;

pub(super) struct Loop {
    pub(super) header: usize,
    pub(super) body: Vec<bool>,
    size: usize,
}

/// Specializes the function's loops, keeping a specialization only when
/// `simplify` leaves the specialized loop smaller than it was.
pub(super) fn version(
    function: &mut Function,
    context: &Context<'_>,
    simplify: &dyn Fn(&mut Function),
) -> bool {
    if function.blocks.is_empty() || !pinned_blocks(function).is_empty() {
        return false;
    }
    let implicit = function.blocks.iter().any(|block| {
        block
            .instructions
            .last()
            .is_some_and(|terminator| has_implicit_arguments(&terminator.operation))
    });
    if implicit {
        return false;
    }
    let mut changed = false;
    let mut tried: HashSet<Name> = HashSet::default();
    for _ in 0..MOST_VERSIONS {
        super::cfg::remove_unreachable(function);
        if !predecessors(function)[0].is_empty() {
            break;
        }
        let mut loops = natural_loops(function);
        loops.sort_by_key(|found| found.size);
        let mut specialized = false;
        // What each trial reads of the function, which only a kept trial
        // changes.
        let known = constants(function);
        let mut types = None;
        for found in loops {
            let header = function.blocks[found.header].name.clone();
            if found.size > LARGEST_LOOP || !tried.insert(header.clone()) {
                continue;
            }
            let Some(mut trial) = specialize(function, &found, context, &known, &mut types) else {
                continue;
            };
            simplify(&mut trial);
            if loop_size(&trial, &header).is_some_and(|size| size < found.size) {
                *function = trial;
                specialized = true;
                break;
            }
        }
        if !specialized {
            break;
        }
        changed = true;
        tried.clear();
    }
    changed
}

/// The immediate dominator of every block, for blocks in reverse postorder.
pub(super) fn dominators(incoming: &[Vec<usize>]) -> Vec<usize> {
    let mut idom = vec![usize::MAX; incoming.len()];
    idom[0] = 0;
    let mut changed = true;
    while changed {
        changed = false;
        for block in 1..incoming.len() {
            let mut new = usize::MAX;
            for &from in &incoming[block] {
                if idom[from] == usize::MAX {
                    continue;
                }
                new = if new == usize::MAX {
                    from
                } else {
                    let (mut left, mut right) = (from, new);
                    while left != right {
                        while left > right {
                            left = idom[left];
                        }
                        while right > left {
                            right = idom[right];
                        }
                    }
                    left
                };
            }
            if new != usize::MAX && idom[block] != new {
                idom[block] = new;
                changed = true;
            }
        }
    }
    idom
}

pub(super) fn dominates(idom: &[usize], dominator: usize, mut block: usize) -> bool {
    loop {
        if block == dominator {
            return true;
        }
        if block == 0 || idom[block] == usize::MAX {
            return false;
        }
        block = idom[block];
    }
}

/// Every natural loop of a function whose blocks are in reverse postorder.
pub(super) fn natural_loops(function: &Function) -> Vec<Loop> {
    let incoming = predecessors(function);
    let idom = dominators(&incoming);
    natural_loops_of(function, &incoming, &idom)
}

/// The natural loops, given each block's `incoming` edges and the immediate
/// dominators `idom` of the blocks in their own order.
pub(super) fn natural_loops_of(
    function: &Function,
    incoming: &[Vec<usize>],
    idom: &[usize],
) -> Vec<Loop> {
    let mut loops = Vec::new();
    for header in 0..function.blocks.len() {
        let latches: Vec<usize> = incoming[header]
            .iter()
            .copied()
            .filter(|&from| dominates(idom, header, from))
            .collect();
        if latches.is_empty() {
            continue;
        }
        let mut body = vec![false; function.blocks.len()];
        body[header] = true;
        let mut work = latches;
        while let Some(block) = work.pop() {
            if !body[block] {
                body[block] = true;
                work.extend(incoming[block].iter().copied());
            }
        }
        let size = (0..body.len())
            .filter(|&block| body[block])
            .map(|block| function.blocks[block].instructions.len())
            .sum();
        loops.push(Loop { header, body, size });
    }
    loops
}

fn loop_size(function: &Function, header: &Name) -> Option<usize> {
    natural_loops(function)
        .into_iter()
        .find(|found| function.blocks[found.header].name == *header)
        .map(|found| found.size)
}

fn edges(block: &Block) -> Vec<&klf_kir::BlockTarget> {
    block
        .instructions
        .last()
        .map(|terminator| terminator.operation.targets())
        .unwrap_or_default()
}

/// The specialization of `found`, or none when no header parameter enters
/// as a constant and keeps it around the loop.
fn specialize(
    function: &Function,
    found: &Loop,
    context: &Context<'_>,
    known: &HashMap<Name, (u16, i128)>,
    types: &mut Option<klf_kir::ValueTypes>,
) -> Option<Function> {
    let mut owner: HashMap<&Name, (usize, usize)> = HashMap::default();
    for (position, block) in function.blocks.iter().enumerate() {
        if found.body[position] {
            for (index, parameter) in block.parameters.iter().enumerate() {
                owner.insert(&parameter.name, (position, index));
            }
        }
    }
    let mut held = entry_constants(function, found, known);
    hold(function, found, &owner, known, &mut held);
    if held.is_empty() {
        return None;
    }
    let types = types.get_or_insert_with(|| klf_kir::infer_value_types_in(function, context.index));
    rewrite(function, found, &held, &owner, known, types)
}

fn width(function: &Function, position: usize, index: usize) -> Option<u16> {
    match function.blocks[position].parameters[index].ty {
        Type::Integer(width) => Some(width),
        _ => None,
    }
}

/// The header parameters every entry gives one constant.
fn entry_constants(
    function: &Function,
    found: &Loop,
    known: &HashMap<Name, (u16, i128)>,
) -> HashMap<Name, (u16, i128)> {
    let mut held: HashMap<Name, (u16, i128)> = HashMap::default();
    let header = &function.blocks[found.header];
    for (index, parameter) in header.parameters.iter().enumerate() {
        let Some(bits) = width(function, found.header, index) else {
            continue;
        };
        let mut value = None;
        let mut single = true;
        for (position, block) in function.blocks.iter().enumerate() {
            if found.body[position] {
                continue;
            }
            for edge in edges(block)
                .into_iter()
                .filter(|edge| edge.block == header.name)
            {
                let given = edge
                    .arguments
                    .get(index)
                    .and_then(|argument| known.get(argument));
                match given {
                    Some(&(given_bits, constant))
                        if given_bits == bits && value.is_none_or(|seen| seen == constant) =>
                    {
                        value = Some(constant);
                    }
                    _ => single = false,
                }
            }
        }
        if single && let Some(value) = value {
            held.insert(parameter.name.clone(), (bits, value));
        }
    }
    held
}

/// Grows `held` by the parameters inside the loop that carry a held value
/// around it, then drops each one an edge breaks until none is left.
fn hold(
    function: &Function,
    found: &Loop,
    owner: &HashMap<&Name, (usize, usize)>,
    known: &HashMap<Name, (u16, i128)>,
    held: &mut HashMap<Name, (u16, i128)>,
) {
    let body = &found.body;
    let positions = &block_positions(function);
    let idom = dominators(&predecessors(function));
    let in_loop_edges = |target: usize| {
        function
            .blocks
            .iter()
            .enumerate()
            .filter(|&(position, _)| body[position])
            .flat_map(|(position, block)| {
                edges(block)
                    .into_iter()
                    .enumerate()
                    .filter(move |(_, edge)| positions.get(&edge.block) == Some(&target))
                    .map(move |(at, edge)| (position, at, edge))
            })
            .collect::<Vec<_>>()
    };
    let mut refused: HashSet<Name> = HashSet::default();
    loop {
        let mut work: Vec<Name> = held.keys().cloned().collect();
        while let Some(name) = work.pop() {
            let (position, index) = owner[&name];
            let constant = held[&name];
            for (_, _, edge) in in_loop_edges(position) {
                let argument = &edge.arguments[index];
                if held.contains_key(argument) || refused.contains(argument) {
                    continue;
                }
                if let Some(&(from, at)) = owner.get(argument)
                    && width(function, from, at) == Some(constant.0)
                {
                    held.insert(argument.clone(), constant);
                    work.push(argument.clone());
                }
            }
        }
        let carried = |argument: &Name, constant: (u16, i128)| {
            known.get(argument) == Some(&constant) || held.get(argument) == Some(&constant)
        };
        let dropped: Vec<Name> = held
            .iter()
            .filter(|(name, constant)| {
                // A value no edge keeps is not held, nor one of an inner
                // loop no iteration keeps: that loop would never come round.
                let (position, index) = owner[*name];
                let incoming = in_loop_edges(position);
                let kept = |argument: &Name| carried(argument, **constant);
                let mut back = incoming
                    .iter()
                    .filter(|(from, _, _)| dominates(&idom, position, *from))
                    .peekable();
                !incoming
                    .iter()
                    .any(|(_, _, edge)| kept(&edge.arguments[index]))
                    || (back.peek().is_some()
                        && !back.any(|(_, _, edge)| kept(&edge.arguments[index])))
            })
            .map(|(name, _)| name.clone())
            .collect();
        if dropped.is_empty() {
            return;
        }
        for name in dropped {
            held.remove(&name);
            refused.insert(name);
        }
    }
}

/// Redirects the edges that break the held values to a copy of the loop,
/// gives the held values their constants inside the loop, and carries the
/// loop's values to the copy and past the loop through stack slots the
/// promotion pass turns back into SSA values.
fn rewrite(
    function: &Function,
    found: &Loop,
    held: &HashMap<Name, (u16, i128)>,
    owner: &HashMap<&Name, (usize, usize)>,
    known: &HashMap<Name, (u16, i128)>,
    types: &klf_kir::ValueTypes,
) -> Option<Function> {
    let body = &found.body;
    let redirect = redirected(function, found, held, known);
    let mut out = function.clone();
    let mut copy: Vec<Block> = Vec::new();
    let mut values: HashMap<Name, Name> = HashMap::default();
    if !redirect.is_empty() {
        let blocks = copy_loop(function, body, &mut copy, &mut values);
        for &(position, at) in &redirect {
            let terminator = out.blocks[position].instructions.last_mut()?;
            let target = &mut terminator.operation.targets_mut()[at].block;
            *target = blocks[target].clone();
        }
    }
    // Inside the loop each held value is its constant.
    let mut constant_of: HashMap<Name, Name> = HashMap::default();
    for (name, &(bits, value)) in held {
        let (position, _) = owner[name];
        let constant = fresh("held");
        let block = &mut out.blocks[position];
        let like = block.instructions.first()?.clone();
        let operation = Operation::ConstInt {
            ty: Type::Integer(bits),
            value,
        };
        block
            .instructions
            .insert(0, instruction(vec![constant.clone()], operation, &like));
        constant_of.insert(name.clone(), constant);
    }
    for (position, block) in out.blocks.iter_mut().enumerate() {
        if body[position] {
            for instruction in &mut block.instructions {
                for used in instruction.operation.used_values_mut() {
                    if let Some(constant) = constant_of.get(used) {
                        *used = constant.clone();
                    }
                }
            }
        }
    }
    carry(&mut out, body, &mut copy, &values, types)?;
    out.blocks.extend(copy);
    Some(out)
}

/// The edges of the loop that break a held value, with the edges into a
/// block every way out of which breaks one: that block goes to the copy
/// whole.
fn redirected(
    function: &Function,
    found: &Loop,
    held: &HashMap<Name, (u16, i128)>,
    known: &HashMap<Name, (u16, i128)>,
) -> HashSet<(usize, usize)> {
    let body = &found.body;
    let positions = block_positions(function);
    let carried = |argument: &Name, constant: (u16, i128)| {
        known.get(argument) == Some(&constant) || held.get(argument) == Some(&constant)
    };
    let mut redirect: HashSet<(usize, usize)> = HashSet::default();
    for (position, block) in function.blocks.iter().enumerate() {
        if !body[position] {
            continue;
        }
        for (at, edge) in edges(block).into_iter().enumerate() {
            let breaks = function
                .blocks
                .get(positions[&edge.block])
                .filter(|_| body[positions[&edge.block]])
                .is_some_and(|target| {
                    target
                        .parameters
                        .iter()
                        .enumerate()
                        .any(|(index, parameter)| {
                            held.get(&parameter.name)
                                .is_some_and(|&constant| !carried(&edge.arguments[index], constant))
                        })
                });
            if breaks {
                redirect.insert((position, at));
            }
        }
    }
    let mut doomed = vec![false; function.blocks.len()];
    let mut grew = true;
    while grew {
        grew = false;
        for (position, block) in function.blocks.iter().enumerate() {
            if !body[position] || position == found.header || doomed[position] {
                continue;
            }
            let out = edges(block);
            let leaves = !out.is_empty()
                && out.iter().enumerate().all(|(at, edge)| {
                    let target = positions[&edge.block];
                    redirect.contains(&(position, at)) || (body[target] && doomed[target])
                });
            if leaves {
                doomed[position] = true;
                grew = true;
            }
        }
    }
    for (position, block) in function.blocks.iter().enumerate() {
        if body[position] && !doomed[position] {
            for (at, edge) in edges(block).into_iter().enumerate() {
                if doomed[positions[&edge.block]] {
                    redirect.insert((position, at));
                }
            }
        }
    }
    redirect
}

/// Copies the loop's blocks into `copy` under fresh names, recording the
/// value names in `values`. Answers the block names.
fn copy_loop(
    function: &Function,
    body: &[bool],
    copy: &mut Vec<Block>,
    values: &mut HashMap<Name, Name>,
) -> HashMap<Name, Name> {
    let mut blocks: HashMap<Name, Name> = HashMap::default();
    for (position, block) in function.blocks.iter().enumerate() {
        if !body[position] {
            continue;
        }
        blocks.insert(block.name.clone(), fresh(block.name.as_str()));
        for parameter in &block.parameters {
            values.insert(parameter.name.clone(), fresh(parameter.name.as_str()));
        }
        for result in block
            .instructions
            .iter()
            .flat_map(|instruction| &instruction.results)
        {
            values.insert(result.clone(), fresh(result.as_str()));
        }
    }
    for (position, block) in function.blocks.iter().enumerate() {
        if !body[position] {
            continue;
        }
        let mut copied = block.clone();
        copied.name = blocks[&block.name].clone();
        for parameter in &mut copied.parameters {
            parameter.name = values[&parameter.name].clone();
        }
        for instruction in &mut copied.instructions {
            for result in &mut instruction.results {
                *result = values[result].clone();
            }
            for used in instruction.operation.used_values_mut() {
                if let Some(renamed) = values.get(used) {
                    *used = renamed.clone();
                }
            }
            for target in instruction.operation.targets_mut() {
                if let Some(renamed) = blocks.get(&target.block) {
                    target.block = renamed.clone();
                }
            }
        }
        copy.push(copied);
    }
    blocks
}

/// Gives each loop value read in the copy or past the loop a stack slot:
/// both definitions store to it and every such read loads from it.
fn carry(
    function: &mut Function,
    body: &[bool],
    copy: &mut [Block],
    values: &HashMap<Name, Name>,
    types: &klf_kir::ValueTypes,
) -> Option<()> {
    if copy.is_empty() {
        return Some(());
    }
    let mut defined: HashMap<Name, Option<Type>> = HashMap::default();
    for (position, block) in function.blocks.iter().enumerate() {
        if !body[position] {
            continue;
        }
        for parameter in &block.parameters {
            defined.insert(parameter.name.clone(), Some(parameter.ty.clone()));
        }
        for result in block
            .instructions
            .iter()
            .flat_map(|instruction| &instruction.results)
        {
            defined.insert(result.clone(), types.get(result).cloned());
        }
    }
    let originals: HashMap<&Name, &Name> =
        values.iter().map(|(name, copied)| (copied, name)).collect();
    let mut wanted: HashSet<Name> = HashSet::default();
    for (position, block) in function.blocks.iter().enumerate() {
        if !body[position] {
            let reads = block
                .instructions
                .iter()
                .flat_map(|instruction| instruction.operation.used_values());
            wanted.extend(reads.filter(|used| defined.contains_key(*used)).cloned());
        }
    }
    let reads = copy.iter().flat_map(|block| &block.instructions);
    for used in reads.flat_map(|instruction| instruction.operation.used_values()) {
        if let Some(&original) = originals.get(used) {
            wanted.insert(original.clone());
        }
    }
    if wanted.is_empty() {
        return Some(());
    }
    let mut slots: HashMap<Name, Name> = HashMap::default();
    let mut allocations = Vec::new();
    let like = function.blocks[0].instructions.first()?.clone();
    for original in wanted {
        let ty = defined
            .get(&original)?
            .as_ref()
            .filter(|ty| is_scalar(ty))?;
        let slot = fresh("carried");
        allocations.push(instruction(
            vec![slot.clone()],
            Operation::StackAlloc(ty.clone()),
            &like,
        ));
        slots.insert(original, slot);
    }
    let in_copy: HashMap<Name, Name> = slots
        .iter()
        .map(|(original, slot)| (values[original].clone(), slot.clone()))
        .collect();
    let none = HashMap::default();
    for (position, block) in function.blocks.iter_mut().enumerate() {
        if body[position] {
            thread(block, &none, &slots);
        } else {
            thread(block, &slots, &none);
        }
    }
    for block in copy {
        thread(block, &in_copy, &in_copy);
    }
    function.blocks[0].instructions.splice(0..0, allocations);
    Some(())
}

/// Loads each read of a `reads` value from its slot and stores each
/// definition of a `writes` value to its slot.
fn thread(block: &mut Block, reads: &HashMap<Name, Name>, writes: &HashMap<Name, Name>) {
    let mut threaded = Vec::with_capacity(block.instructions.len());
    for parameter in &block.parameters {
        if let Some(slot) = writes.get(&parameter.name)
            && let Some(like) = block.instructions.first()
        {
            threaded.push(store(&parameter.name, slot, like));
        }
    }
    for mut original in std::mem::take(&mut block.instructions) {
        let mut loads = Vec::new();
        for used in original.operation.used_values_mut() {
            if let Some(slot) = reads.get(used) {
                let loaded = fresh(used.as_str());
                loads.push((loaded.clone(), slot.clone()));
                *used = loaded;
            }
        }
        for (loaded, slot) in loads {
            threaded.push(instruction(
                vec![loaded],
                Operation::Load {
                    mode: LoadMode::Trivial,
                    address: slot,
                },
                &original,
            ));
        }
        let stores: Vec<Instruction> = original
            .results
            .iter()
            .filter_map(|result| Some(store(result, writes.get(result)?, &original)))
            .collect();
        threaded.push(original);
        threaded.extend(stores);
    }
    block.instructions = threaded;
}

fn store(value: &Name, slot: &Name, like: &Instruction) -> Instruction {
    instruction(
        Vec::new(),
        Operation::Store {
            mode: StoreMode::Trivial,
            value: value.clone(),
            address: slot.clone(),
        },
        like,
    )
}
