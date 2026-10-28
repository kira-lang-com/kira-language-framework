//! Which `stack.alloc`s need runtime frame memory rather than a stack slot.
//!
//! A native slot belongs to its instruction, so an alloc that runs again
//! before the function returns hands out the same address. That is sound
//! while no address it handed out earlier is still in use: none was stored,
//! returned or given to foreign code, and none is live where the alloc runs
//! again. Only an alloc on a cycle of the control flow graph can run again.

use klf_core::Name;
use klf_kir::{Function, ModuleIndex, Operation};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// The `stack.alloc` results that take runtime frame memory. Empty when every
/// alloc of the function can be a native slot.
pub(super) fn frame_allocs<'f>(function: &'f Function, index: &ModuleIndex) -> HashSet<&'f Name> {
    let cyclic: HashSet<usize> = cyclic_blocks(function).into_iter().collect();
    let mut allocs = Vec::new();
    for (block_index, block) in function
        .blocks
        .iter()
        .enumerate()
        .filter(|(at, _)| cyclic.contains(at))
    {
        for (position, instruction) in block.instructions.iter().enumerate() {
            if matches!(instruction.operation, Operation::StackAlloc(_)) {
                allocs.extend(instruction.results.iter().map(|name| Alloc {
                    name,
                    block: block_index,
                    position,
                }));
            }
        }
    }
    if allocs.is_empty() {
        return HashSet::default();
    }
    let flow = Flow::trace(function, &allocs);
    let mut sets = flow.sets;
    let mut outlive: Vec<usize> = escaping(function, &flow.origin, index);
    let live_out = live_out(function, &flow.origin);
    for (at, alloc) in allocs.iter().enumerate() {
        let block = &function.blocks[alloc.block];
        let mut live = live_out[alloc.block].clone();
        for instruction in block.instructions[alloc.position + 1..].iter().rev() {
            for result in &instruction.results {
                live.remove(result);
            }
            live.extend(
                instruction
                    .operation
                    .used_values()
                    .into_iter()
                    .filter(|used| flow.origin.contains_key(used)),
            );
        }
        // Past the alloc its own result names the fresh address.
        live.remove(alloc.name);
        let own = sets.root(at);
        if live
            .iter()
            .any(|value| sets.root(flow.origin[value]) == own)
        {
            outlive.push(at);
        }
    }
    let outlive: HashSet<usize> = outlive.into_iter().map(|set| sets.root(set)).collect();
    allocs
        .iter()
        .enumerate()
        .filter(|(at, _)| outlive.contains(&sets.root(*at)))
        .map(|(_, alloc)| alloc.name)
        .collect()
}

struct Alloc<'f> {
    name: &'f Name,
    block: usize,
    position: usize,
}

/// Where the addresses of the allocs go: each value derived from one, through
/// address arithmetic or a block argument, keyed to the alloc it came from,
/// with the allocs one value combines joined into one set.
struct Flow<'f> {
    origin: HashMap<&'f Name, usize>,
    sets: Sets,
}

impl<'f> Flow<'f> {
    fn trace(function: &'f Function, allocs: &[Alloc<'f>]) -> Self {
        let blocks: HashMap<&Name, &klf_kir::Block> = function
            .blocks
            .iter()
            .map(|block| (&block.name, block))
            .collect();
        let mut flow = Self {
            origin: allocs
                .iter()
                .enumerate()
                .map(|(at, alloc)| (alloc.name, at))
                .collect(),
            sets: Sets::new(allocs.len()),
        };
        let mut changed = true;
        while changed {
            changed = false;
            for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
                let operation = &instruction.operation;
                for target in operation.targets() {
                    let Some(block) = blocks.get(&target.block) else {
                        continue;
                    };
                    for (argument, parameter) in target.arguments.iter().zip(&block.parameters) {
                        changed |= flow.carry(&[argument], &parameter.name);
                    }
                }
                let passes_address = !matches!(
                    operation,
                    Operation::Load { .. }
                        | Operation::Store { .. }
                        | Operation::Call { .. }
                        | Operation::CallIndirect { .. }
                ) && !operation.is_terminator();
                if passes_address {
                    let used = operation.used_values();
                    for result in &instruction.results {
                        changed |= flow.carry(&used, result);
                    }
                }
            }
        }
        flow
    }

    /// Records that `result` may hold an address from any of `sources`.
    fn carry(&mut self, sources: &[&Name], result: &'f Name) -> bool {
        let sources: Vec<usize> = sources
            .iter()
            .filter_map(|source| self.origin.get(*source).copied())
            .collect();
        let Some((&first, rest)) = sources.split_first() else {
            return false;
        };
        let mut changed = false;
        for &other in rest {
            changed |= self.sets.join(first, other);
        }
        if let Some(&known) = self.origin.get(result) {
            changed | self.sets.join(first, known)
        } else {
            self.origin.insert(result, first);
            true
        }
    }
}

/// The allocs an address of which leaves the function's values: stored as a
/// value, handed to a foreign function or an unmanaged operation, or
/// returned, failed or thrown.
fn escaping(
    function: &Function,
    origin: &HashMap<&Name, usize>,
    index: &ModuleIndex,
) -> Vec<usize> {
    let derived = |value: &Name| origin.get(value).copied();
    let foreign = |callee: &Name, arguments: &[Name]| {
        if index
            .functions
            .get(callee.as_str())
            .is_some_and(|declared| declared.foreign)
        {
            arguments.iter().filter_map(&derived).collect()
        } else {
            Vec::new()
        }
    };
    let mut escaped = Vec::new();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        match &instruction.operation {
            Operation::Store { value, .. } => escaped.extend(derived(value)),
            Operation::Call {
                callee, arguments, ..
            } => escaped.extend(foreign(callee, arguments)),
            Operation::TryCall(call) => escaped.extend(foreign(&call.callee, &call.arguments)),
            Operation::Invoke(call) => escaped.extend(foreign(&call.callee, &call.arguments)),
            Operation::Return(_)
            | Operation::Fail(_)
            | Operation::Throw(_)
            | Operation::Resume(_) => {
                escaped.extend(
                    instruction
                        .operation
                        .used_values()
                        .into_iter()
                        .filter_map(derived),
                );
            }
            operation if operation.is_unmanaged() => {
                escaped.extend(operation.used_values().into_iter().filter_map(derived));
            }
            _ => {}
        }
    }
    escaped
}

/// The derived values live out of each block, by the usual backward
/// dataflow over the values `origin` knows.
fn live_out<'f>(
    function: &'f Function,
    origin: &HashMap<&'f Name, usize>,
) -> Vec<HashSet<&'f Name>> {
    let position: HashMap<&Name, usize> = function
        .blocks
        .iter()
        .enumerate()
        .map(|(at, block)| (&block.name, at))
        .collect();
    let mut uses = Vec::with_capacity(function.blocks.len());
    let mut defs = Vec::with_capacity(function.blocks.len());
    let mut successors = Vec::with_capacity(function.blocks.len());
    for block in &function.blocks {
        let mut read = HashSet::default();
        let mut defined: HashSet<&Name> = block
            .parameters
            .iter()
            .map(|parameter| &parameter.name)
            .collect();
        for instruction in &block.instructions {
            for value in instruction.operation.used_values() {
                if origin.contains_key(value) && !defined.contains(value) {
                    read.insert(value);
                }
            }
            defined.extend(instruction.results.iter());
        }
        uses.push(read);
        defs.push(defined);
        successors.push(block.instructions.last().map_or_else(Vec::new, |last| {
            last.operation
                .successors()
                .into_iter()
                .filter_map(|name| position.get(name).copied())
                .collect::<Vec<_>>()
        }));
    }
    let mut live_in: Vec<HashSet<&Name>> = uses.clone();
    let mut live_out: Vec<HashSet<&Name>> = vec![HashSet::default(); function.blocks.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for block in (0..function.blocks.len()).rev() {
            let out: HashSet<&Name> = successors[block]
                .iter()
                .flat_map(|successor| live_in[*successor].iter().copied())
                .collect();
            let into: HashSet<&Name> = uses[block]
                .iter()
                .copied()
                .chain(
                    out.iter()
                        .copied()
                        .filter(|value| !defs[block].contains(value)),
                )
                .collect();
            if into.len() != live_in[block].len() {
                live_in[block] = into;
                changed = true;
            }
            live_out[block] = out;
        }
    }
    live_out
}

/// The blocks on a cycle: members of a strongly connected component with more
/// than one block, or with an edge to itself. Tarjan's algorithm, iterative so
/// a long function cannot exhaust the native stack.
fn cyclic_blocks(function: &Function) -> Vec<usize> {
    let position: HashMap<&Name, usize> = function
        .blocks
        .iter()
        .enumerate()
        .map(|(at, block)| (&block.name, at))
        .collect();
    let successors: Vec<Vec<usize>> = function
        .blocks
        .iter()
        .map(|block| {
            block.instructions.last().map_or_else(Vec::new, |last| {
                last.operation
                    .successors()
                    .into_iter()
                    .filter_map(|name| position.get(name).copied())
                    .collect()
            })
        })
        .collect();
    let count = function.blocks.len();
    let mut order = vec![usize::MAX; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut cyclic = Vec::new();
    let mut next = 0;
    for root in 0..count {
        if order[root] != usize::MAX {
            continue;
        }
        let mut work = vec![(root, 0)];
        while let Some((block, edge)) = work.pop() {
            if edge == 0 {
                order[block] = next;
                low[block] = next;
                next += 1;
                stack.push(block);
                on_stack[block] = true;
            }
            if let Some(&target) = successors[block].get(edge) {
                work.push((block, edge + 1));
                if order[target] == usize::MAX {
                    work.push((target, 0));
                } else if on_stack[target] {
                    low[block] = low[block].min(order[target]);
                }
                continue;
            }
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[block]);
            }
            if low[block] == order[block] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == block {
                        break;
                    }
                }
                if component.len() > 1 || successors[block].contains(&block) {
                    cyclic.extend(component);
                }
            }
        }
    }
    cyclic
}

/// Disjoint sets of allocs, joined when one address derives from several.
struct Sets {
    parent: Vec<usize>,
}

impl Sets {
    fn new(count: usize) -> Self {
        Self {
            parent: (0..count).collect(),
        }
    }

    fn root(&mut self, mut member: usize) -> usize {
        while self.parent[member] != member {
            self.parent[member] = self.parent[self.parent[member]];
            member = self.parent[member];
        }
        member
    }

    /// Joins the sets of `a` and `b`; whether they were apart.
    fn join(&mut self, a: usize, b: usize) -> bool {
        let (a, b) = (self.root(a), self.root(b));
        if a == b {
            return false;
        }
        self.parent[b] = a;
        true
    }
}
