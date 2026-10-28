//! Owned arguments the callee only destroys become borrowed (section 14.3).
//! When a known callee takes a counted value it reads only through borrows
//! and destroys just before each `return`, the parameter becomes `borrow`,
//! the callee's destroys go, and each caller destroys the argument right
//! after the call instead. When the callee cannot release anything nor read
//! a count, a caller that copied the value just to pass it passes the
//! original and neither copies nor destroys.

use super::clean::pure;
use super::pairing::{borrows, counted, passable};
use klf_core::Name;
use klf_kir::{Convention, Function, Instruction, Linkage, Module, ModuleIndex, Operation};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Rewrites every parameter the rule covers, and the calls that pass it.
pub(super) fn borrow_arguments(module: &mut Module) -> bool {
    let referenced = referenced(module);
    let index = ModuleIndex::of(module);
    let mut borrowed: HashMap<Name, Vec<usize>> = HashMap::default();
    let mut inert: HashSet<Name> = HashSet::default();
    for function in &module.functions {
        if !candidate(function, &referenced) {
            continue;
        }
        let parameters: Vec<usize> = function
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, parameter)| {
                matches!(parameter.convention, None | Some(Convention::Owned))
                    && !parameter.context
                    && counted(&parameter.ty, &index)
                    && only_destroyed_at_returns(function, &parameter.name)
            })
            .map(|(position, _)| position)
            .collect();
        if !parameters.is_empty() {
            if releases_nothing(function, &parameters) {
                inert.insert(function.name.clone());
            }
            borrowed.insert(function.name.clone(), parameters);
        }
    }
    if borrowed.is_empty() {
        return false;
    }
    for function in &mut module.functions {
        if let Some(parameters) = borrowed.get(&function.name) {
            lend(function, parameters);
        }
        destroy_after_calls(function, &borrowed, &inert);
    }
    true
}

/// Every function named other than as a direct call's callee.
fn referenced(module: &Module) -> HashSet<Name> {
    let mut names = HashSet::default();
    for instruction in module
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.instructions)
    {
        match &instruction.operation {
            Operation::FunctionReference(name) => {
                names.insert(name.clone());
            }
            Operation::Call {
                callee,
                generic_args,
                ..
            } if !generic_args.is_empty() => {
                names.insert(callee.clone());
            }
            _ => {}
        }
    }
    names
}

/// Whether every caller of `function` is a direct call this pass can see.
fn candidate(function: &Function, referenced: &HashSet<Name>) -> bool {
    !function.foreign
        && !function.blocks.is_empty()
        && function.failure.is_none()
        && function.generics.is_empty()
        && matches!(function.linkage, Linkage::Module | Linkage::Private)
        && !referenced.contains(&function.name)
        && function.blocks.first().is_some_and(|entry| {
            entry.parameters.len() == function.parameters.len()
                && entry
                    .parameters
                    .iter()
                    .zip(&function.parameters)
                    .all(|(block, parameter)| block.name == parameter.name)
        })
}

/// Whether `parameter` is read only through borrows and destroyed exactly
/// once in each block that returns, right before nothing else uses it, and
/// nowhere else.
fn only_destroyed_at_returns(function: &Function, parameter: &Name) -> bool {
    for block in &function.blocks {
        let returns = block
            .instructions
            .last()
            .is_some_and(|terminator| matches!(terminator.operation, Operation::Return(_)));
        let mut destroyed = false;
        for instruction in &block.instructions {
            let uses = instruction.operation.used_values();
            if !uses.contains(&parameter) {
                continue;
            }
            if destroyed {
                return false;
            }
            match &instruction.operation {
                Operation::Destroy(_) if returns => destroyed = true,
                operation if borrows(operation) && !matches!(operation, Operation::Copy(_)) => {}
                _ => return false,
            }
        }
        if returns != destroyed {
            return false;
        }
    }
    true
}

fn lend(function: &mut Function, parameters: &[usize]) {
    let names: HashSet<Name> = parameters
        .iter()
        .map(|&position| function.parameters[position].name.clone())
        .collect();
    for &position in parameters {
        function.parameters[position].convention = Some(Convention::Borrow);
        function.blocks[0].parameters[position].convention = Some(Convention::Borrow);
    }
    for block in &mut function.blocks {
        block.instructions.retain(
            |instruction| !matches!(&instruction.operation, Operation::Destroy(value) if names.contains(value)),
        );
    }
}

/// Whether the callee, once its lent parameters lose their destroys, can
/// neither release a value nor read a count: no calls, stores, destroys or
/// uniqueness checks.
fn releases_nothing(function: &Function, parameters: &[usize]) -> bool {
    let lent: Vec<&Name> = parameters
        .iter()
        .map(|&position| &function.parameters[position].name)
        .collect();
    function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .all(|instruction| match &instruction.operation {
            Operation::Destroy(value) => lent.contains(&value),
            Operation::Load { .. }
            | Operation::LoadBorrow(_)
            | Operation::RawLoad { .. }
            | Operation::Return(_)
            | Operation::Branch(_)
            | Operation::ConditionalBranch(_) => true,
            operation => pure(operation) || borrows(operation),
        })
}

fn destroy_after_calls(
    function: &mut Function,
    borrowed: &HashMap<Name, Vec<usize>>,
    inert: &HashSet<Name>,
) {
    for block in &mut function.blocks {
        let mut uncopied = HashSet::default();
        let mut after: HashMap<usize, Vec<Instruction>> = HashMap::default();
        for at in 0..block.instructions.len() {
            let Operation::Call { callee, .. } = &block.instructions[at].operation else {
                continue;
            };
            let Some(parameters) = borrowed.get(callee) else {
                continue;
            };
            let free = inert.contains(callee);
            for &position in parameters {
                let Operation::Call { arguments, .. } = &block.instructions[at].operation else {
                    unreachable!();
                };
                let Some(argument) = arguments.get(position).cloned() else {
                    continue;
                };
                if let Some((copy, original)) = free
                    .then(|| copied_for(&block.instructions, at, &argument))
                    .flatten()
                {
                    uncopied.insert(copy);
                    if let Operation::Call { arguments, .. } = &mut block.instructions[at].operation
                    {
                        arguments[position] = original;
                    }
                    continue;
                }
                after.entry(at).or_default().push(Instruction {
                    results: Vec::new(),
                    operation: Operation::Destroy(argument),
                    debug_location: None,
                    span: block.instructions[at].span,
                });
            }
        }
        let mut rewritten = Vec::with_capacity(block.instructions.len());
        for (at, instruction) in std::mem::take(&mut block.instructions)
            .into_iter()
            .enumerate()
        {
            if uncopied.contains(&at) {
                continue;
            }
            rewritten.push(instruction);
            rewritten.extend(after.remove(&at).into_iter().flatten());
        }
        block.instructions = rewritten;
    }
}

/// The position of the `copy` that made `argument` and the value it
/// copied, when nothing between it and the call at `call` uses the copy or
/// could release or read the count of either.
fn copied_for(instructions: &[Instruction], call: usize, argument: &Name) -> Option<(usize, Name)> {
    let at = instructions[..call]
        .iter()
        .rposition(|instruction| instruction.results.first() == Some(argument))?;
    let Operation::Copy(original) = &instructions[at].operation else {
        return None;
    };
    let clear = instructions[at + 1..call].iter().all(|instruction| {
        let uses = instruction.operation.used_values();
        if uses.contains(&argument) {
            return false;
        }
        if uses.contains(&original) {
            return borrows(&instruction.operation);
        }
        passable(&instruction.operation)
    });
    let once = instructions[call]
        .operation
        .used_values()
        .iter()
        .filter(|used| **used == argument)
        .count()
        == 1;
    (clear && once).then(|| (at, original.clone()))
}
