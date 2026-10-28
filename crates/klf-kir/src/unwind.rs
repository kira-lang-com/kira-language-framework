//! Unwinding with no handler to stop it (section 12.14). Section 12.14
//! does not say what a `catch %typeinfo` clause is matched against nor what
//! the selector a `landingpad` yields holds, so a function with a catch
//! clause is left as it is and every backend refuses it. Without one, no
//! frame can stop an unwind: like the platform's two-phase unwinder, whose
//! search finds no handler and runs no cleanup, a `throw` or `resume` ends
//! the run at once with `KLF-RT-0020`, the trap of an exception reaching
//! the boundary. An `invoke` is then a call whose unwind edge is never
//! taken, and the cleanup blocks only that edge reached are dropped.

use crate::{Function, Instruction, Module, ModuleIndex, Operation, Type};
use klf_core::Name;
use std::collections::HashSet;

const BOUNDARY: &str = "KLF-RT-0020";

/// The module with each handlerless unwinding instruction rewritten, or
/// `None` when it has none.
#[must_use]
pub fn unwinding_without_handlers(module: &Module) -> Option<Module> {
    let index = ModuleIndex::of(module);
    let mut result = module.clone();
    let mut changed = false;
    for function in &mut result.functions {
        if let Some(rewritten) = rewrite(function, &index) {
            *function = rewritten;
            changed = true;
        }
    }
    changed.then_some(result)
}

fn rewrite(function: &Function, index: &ModuleIndex<'_>) -> Option<Function> {
    let operations = || {
        function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .map(|instruction| &instruction.operation)
    };
    let unwinds = operations().any(|operation| {
        matches!(
            operation,
            Operation::Invoke(_)
                | Operation::LandingPad { .. }
                | Operation::Resume(_)
                | Operation::Throw(_)
        )
    });
    let catches = operations().any(|operation| {
        matches!(operation, Operation::LandingPad { catches, .. } if !catches.is_empty())
    });
    if !unwinds || catches {
        return None;
    }
    let mut taken = names(function);
    let mut result = function.clone();
    for block in &mut result.blocks {
        let mut out = Vec::with_capacity(block.instructions.len());
        for instruction in std::mem::take(&mut block.instructions) {
            let made = |results: Vec<Name>, operation: Operation| Instruction {
                results,
                operation,
                debug_location: instruction.debug_location.clone(),
                span: instruction.span,
            };
            match &instruction.operation {
                Operation::Invoke(call) => {
                    let ty = &index.functions.get(call.callee.as_str())?.result;
                    let results = if *ty == Type::Unit {
                        Vec::new()
                    } else {
                        vec![fresh(&mut taken, &format!("{}.result", call.callee))]
                    };
                    out.push(made(
                        results.clone(),
                        Operation::Call {
                            callee: call.callee.clone(),
                            generic_args: Vec::new(),
                            arguments: call.arguments.clone(),
                        },
                    ));
                    if let Some(value) = results.into_iter().next().filter(|_| !trivial(ty)) {
                        out.push(made(Vec::new(), Operation::Destroy(value)));
                    }
                    out.push(made(Vec::new(), Operation::Branch(call.normal.clone())));
                }
                Operation::Resume(_) | Operation::Throw(_) => {
                    out.push(made(Vec::new(), Operation::Trap(BOUNDARY.to_owned())));
                }
                _ => out.push(instruction.clone()),
            }
        }
        block.instructions = out;
    }
    drop_unreached(&mut result);
    Some(result)
}

/// Whether dropping a value of `ty` needs no `destroy`.
fn trivial(ty: &Type) -> bool {
    ty.is_integer() || ty.is_float() || matches!(ty, Type::Pointer)
}

/// Removes the blocks no path from the entry reaches.
fn drop_unreached(function: &mut Function) {
    let Some(entry) = function.blocks.first() else {
        return;
    };
    let mut reached: HashSet<Name> = HashSet::from([entry.name.clone()]);
    let mut pending = vec![entry.name.clone()];
    while let Some(name) = pending.pop() {
        let Some(block) = function.blocks.iter().find(|block| block.name == name) else {
            continue;
        };
        for instruction in &block.instructions {
            for target in instruction.operation.targets() {
                if reached.insert(target.block.clone()) {
                    pending.push(target.block.clone());
                }
            }
            if let Operation::BranchIndirect { targets, .. } = &instruction.operation {
                for target in targets {
                    if reached.insert(target.clone()) {
                        pending.push(target.clone());
                    }
                }
            }
        }
    }
    function
        .blocks
        .retain(|block| reached.contains(&block.name));
}

/// Every block and value name of `function`.
fn names(function: &Function) -> HashSet<Name> {
    let mut taken: HashSet<Name> = function
        .parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    for block in &function.blocks {
        taken.insert(block.name.clone());
        taken.extend(block.parameters.iter().map(|p| p.name.clone()));
        for instruction in &block.instructions {
            taken.extend(instruction.results.iter().cloned());
        }
    }
    taken
}

fn fresh(taken: &mut HashSet<Name>, stem: &str) -> Name {
    let mut attempt = 0u32;
    loop {
        let name = Name::new(format!("{stem}.{attempt}"));
        attempt += 1;
        if taken.insert(name.clone()) {
            return name;
        }
    }
}
