//! Inlining of direct calls (section 14.1: `@Inline` and trivial functions at
//! O1, a cost model at O2, a larger budget in Cranelift's tier 2).

use super::graph::{calls_of, strongly_connected};
use super::util::{block_positions, fresh, instruction, successors_of};
use klf_core::Name;
use klf_kir::{
    Block, BlockTarget, Engine, Function, FunctionAttribute, Instruction, Module, Operation,
    Parameter,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// How large a callee may be, in instructions, and how large a caller may
/// grow.
#[derive(Clone, Copy)]
pub(super) struct Budget {
    pub(super) callee: usize,
    pub(super) marked: usize,
    pub(super) caller: usize,
}

/// Whether `callee`'s body can stand in for a call to it, whatever the size.
pub(super) fn inlinable(callee: &Function) -> bool {
    if callee.foreign
        || callee.blocks.is_empty()
        || callee.failure.is_some()
        || !callee.generics.is_empty()
        || callee.attributes.iter().any(|attribute| {
            matches!(
                attribute,
                FunctionAttribute::NoInline
                    | FunctionAttribute::ReturnsTwice
                    | FunctionAttribute::Initializer
            )
        })
    {
        return false;
    }
    let cyclic = cyclic_blocks(callee);
    for (position, block) in callee.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            let refused = match &instruction.operation {
                Operation::StackAlloc(_) => position != 0 && cyclic[position],
                Operation::LandingPad { .. }
                | Operation::Invoke(_)
                | Operation::Resume(_)
                | Operation::Throw(_)
                | Operation::SetJmp(_)
                | Operation::LongJmp { .. }
                | Operation::VaStart(_)
                | Operation::VaArg { .. }
                | Operation::VaCopy { .. }
                | Operation::VaEnd(_)
                | Operation::Asm(_)
                | Operation::BranchIndirect { .. }
                | Operation::BlockAddress(_)
                | Operation::Fail(_) => true,
                _ => false,
            };
            if refused {
                return false;
            }
        }
    }
    true
}

pub(super) fn size(function: &Function) -> usize {
    function
        .blocks
        .iter()
        .map(|block| block.instructions.len())
        .sum()
}

/// The functions kept out of line as a slow path: each larger than a
/// trivial function, and called only from functions of at most `small`
/// instructions that are called themselves. Those callers are inlined
/// wherever they are called, and inlining the slow path into them, or into
/// what they are inlined into, would put a copy of it at every site of its
/// fast path. Out of line, a site pays one call, and only when it takes the
/// slow path. The answer is the module's as it stands, so every tier keeps
/// the same functions out of line.
pub(super) fn slow_paths(module: &Module, small: usize, trivial: usize) -> HashSet<Name> {
    let functions = &module.functions;
    let mut callers: HashMap<&Name, Vec<usize>> = HashMap::default();
    let calls: Vec<Vec<Name>> = functions.iter().map(calls_of).collect();
    for (position, names) in calls.iter().enumerate() {
        for name in names {
            callers.entry(name).or_default().push(position);
        }
    }
    let wrapper = |position: usize| {
        let function = &functions[position];
        callers.contains_key(&function.name) && size(function) <= small
    };
    functions
        .iter()
        .filter(|function| {
            !function.blocks.is_empty()
                && size(function) > trivial
                && !function.attributes.contains(&FunctionAttribute::Inline)
                && callers.get(&function.name).is_some_and(|from| {
                    from.iter().all(|&position| {
                        functions[position].name != function.name && wrapper(position)
                    })
                })
        })
        .map(|function| function.name.clone())
        .collect()
}

#[allow(clippy::similar_names)]
fn fits(caller: &Function, callee: &Function, budget: Budget) -> bool {
    if callee.engine != caller.engine && callee.engine != Engine::Any {
        return false;
    }
    let marked = callee.attributes.contains(&FunctionAttribute::Inline);
    let limit = if marked { budget.marked } else { budget.callee };
    size(callee) <= limit
}

/// Inlines into `caller` every direct call `bodies` answers with an
/// inlinable function that fits `budget`. Answers whether anything changed.
pub(super) fn inline_calls<'b>(
    caller: &mut Function,
    bodies: &dyn Fn(&Name) -> Option<(&'b Function, bool)>,
    budget: Budget,
) -> bool {
    if caller.attributes.contains(&FunctionAttribute::ReturnsTwice) {
        return false;
    }
    let entry_has_predecessors = caller.blocks.iter().any(|block| {
        block.instructions.last().is_some_and(|terminator| {
            terminator
                .operation
                .successors()
                .iter()
                .any(|name| **name == caller.blocks[0].name)
        })
    });
    // Which blocks lie on a cycle, by name: a callee's slots move to the
    // entry only from a call on a cycle, so each runs once per call.
    let mut on_cycle: HashSet<Name> = cyclic_blocks(caller)
        .into_iter()
        .zip(&caller.blocks)
        .filter(|&(cyclic, _)| cyclic)
        .map(|(_, block)| block.name.clone())
        .collect();
    let mut changed = false;
    let mut block = 0;
    let mut start = 0;
    // Blocks copied from a function on a cycle, where calls into a cycle
    // stay calls so inlining ends.
    let mut unrolled: HashSet<Name> = HashSet::default();
    while block < caller.blocks.len() {
        let inside = unrolled.contains(&caller.blocks[block].name);
        let found = caller.blocks[block].instructions[start..]
            .iter()
            .position(|instruction| match &instruction.operation {
                Operation::Call {
                    callee,
                    generic_args,
                    ..
                } if generic_args.is_empty() && *callee != caller.name => bodies(callee)
                    .is_some_and(|(body, cyclic)| {
                        !(inside && cyclic) && fits(caller, body, budget)
                    }),
                _ => false,
            });
        let Some(found) = found else {
            block += 1;
            start = 0;
            continue;
        };
        let at = start + found;
        let Operation::Call { callee, .. } = &caller.blocks[block].instructions[at].operation
        else {
            unreachable!("the position names a call");
        };
        let Some((body, cyclic)) = bodies(callee) else {
            unreachable!("the call was found inlinable");
        };
        if size(caller) + size(body) > budget.caller {
            start = at + 1;
            continue;
        }
        let site_on_cycle = on_cycle.contains(&caller.blocks[block].name);
        let hoist = site_on_cycle && !entry_has_predecessors;
        let added = splice(caller, block, at, body, hoist);
        let body_cycles = cyclic_blocks(body);
        for (name, cyclic) in added.iter().zip(body_cycles) {
            if cyclic || site_on_cycle {
                on_cycle.insert(name.clone());
            }
        }
        if site_on_cycle {
            on_cycle.insert(caller.blocks[block + 1 + added.len()].name.clone());
        }
        if inside {
            unrolled.insert(caller.blocks[block + 1 + added.len()].name.clone());
        }
        if inside || cyclic {
            unrolled.extend(added);
        }
        start = 0;
        changed = true;
    }
    changed
}

/// Replaces the call at `at` in block `block` with `callee`'s blocks.
/// Answers the names of the callee's copied blocks.
#[allow(clippy::similar_names)]
fn splice(
    caller: &mut Function,
    block: usize,
    at: usize,
    callee: &Function,
    hoist: bool,
) -> Vec<Name> {
    let mut names: HashMap<Name, Name> = HashMap::default();
    let mut rename = |name: &Name| {
        names
            .entry(name.clone())
            .or_insert_with(|| fresh(name.as_str()))
            .clone()
    };
    let mut blocks: Vec<Block> = Vec::with_capacity(callee.blocks.len() + 1);
    for original in &callee.blocks {
        let mut copied = original.clone();
        copied.name = rename(&original.name);
        for parameter in &mut copied.parameters {
            parameter.name = rename(&parameter.name);
        }
        for instruction in &mut copied.instructions {
            for result in &mut instruction.results {
                *result = rename(result);
            }
        }
        blocks.push(copied);
    }
    for (copied, original) in blocks.iter_mut().zip(&callee.blocks) {
        for instruction in &mut copied.instructions {
            for used in instruction.operation.used_values_mut() {
                *used = rename(used);
            }
            for target in instruction.operation.targets_mut() {
                target.block = rename(&target.block);
            }
        }
        debug_assert_eq!(copied.instructions.len(), original.instructions.len());
    }

    let call = caller.blocks[block].instructions[at].clone();
    let Operation::Call { arguments, .. } = &call.operation else {
        unreachable!("the position names a call");
    };
    let continuation = fresh("continue");
    let rest: Vec<Instruction> = caller.blocks[block].instructions.split_off(at + 1);
    caller.blocks[block].instructions.pop();
    caller.blocks[block].instructions.push(instruction(
        Vec::new(),
        Operation::Branch(BlockTarget {
            block: blocks[0].name.clone(),
            arguments: arguments.clone(),
        }),
        &call,
    ));
    let parameters = call
        .results
        .iter()
        .map(|result| Parameter {
            name: result.clone(),
            convention: None,
            context: false,
            ty: callee.result.clone(),
            span: call.span,
        })
        .collect();
    let returns_value = !call.results.is_empty();
    for copied in &mut blocks {
        for instruction in &mut copied.instructions {
            if let Operation::Return(value) = &instruction.operation {
                let arguments = if returns_value {
                    value.iter().cloned().collect()
                } else {
                    Vec::new()
                };
                instruction.operation = Operation::Branch(BlockTarget {
                    block: continuation.clone(),
                    arguments,
                });
            }
        }
    }
    if hoist {
        let allocs: Vec<Instruction> = blocks
            .iter_mut()
            .flat_map(|copied| {
                copied
                    .instructions
                    .extract_if(.., |instruction| {
                        matches!(instruction.operation, Operation::StackAlloc(_))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        caller.blocks[0].instructions.splice(0..0, allocs);
    }
    let after = Block {
        name: continuation,
        parameters,
        instructions: rest,
        span: caller.blocks[block].span,
    };
    let count = blocks.len();
    let added: Vec<Name> = blocks.iter().map(|copied| copied.name.clone()).collect();
    let first = block + 1;
    caller.blocks.splice(first..first, blocks);
    caller.blocks.insert(first + count, after);
    added
}

/// For each block of `function`, whether it lies on a cycle of the control
/// flow graph.
fn cyclic_blocks(function: &Function) -> Vec<bool> {
    let positions = block_positions(function);
    let edges: Vec<Vec<usize>> = function
        .blocks
        .iter()
        .map(|block| successors_of(block, &positions))
        .collect();
    let mut cyclic = vec![false; edges.len()];
    for component in strongly_connected(&edges) {
        if component.len() > 1 || edges[component[0]].contains(&component[0]) {
            for member in component {
                cyclic[member] = true;
            }
        }
    }
    cyclic
}
