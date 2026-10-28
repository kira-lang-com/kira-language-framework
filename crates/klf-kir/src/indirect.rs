//! Indirect branches as switches (section 12.13). `block.addr bbN` names a
//! block of its own function only, and `br.indirect` lists every block it
//! may reach, so before a backend lowers a module each block address
//! becomes the block's position in its function plus one, as a `ptr`, and
//! each `br.indirect` a `switch.int` over its listed blocks whose default
//! is unreachable.

use crate::{
    Block, BlockTarget, ConvertKind, Function, Instruction, IntegerSwitch, Module, Operation, Type,
};
use klf_core::Name;
use std::collections::{HashMap, HashSet};

/// The module with its block addresses and indirect branches rewritten, or
/// `None` when it has none this can rewrite.
#[must_use]
pub fn indirect_branches_to_switches(module: &Module) -> Option<Module> {
    let mut result = module.clone();
    let mut changed = false;
    for function in &mut result.functions {
        if let Some(rewritten) = rewrite(function) {
            *function = rewritten;
            changed = true;
        }
    }
    changed.then_some(result)
}

fn rewrite(function: &Function) -> Option<Function> {
    let indirect = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| {
            matches!(
                instruction.operation,
                Operation::BlockAddress(_) | Operation::BranchIndirect { .. }
            )
        });
    if !indirect {
        return None;
    }
    let numbers: HashMap<&Name, i128> = function
        .blocks
        .iter()
        .zip(1..)
        .map(|(block, number)| (&block.name, number))
        .collect();
    let mut taken = names(function);
    let mut fresh = |stem: String| {
        let mut attempt = 0u32;
        loop {
            let name = Name::new(format!("{stem}.{attempt}"));
            attempt += 1;
            if taken.insert(name.clone()) {
                return name;
            }
        }
    };
    let mut result = function.clone();
    let mut nowhere: Option<Name> = None;
    for block in &mut result.blocks {
        let mut out = Vec::with_capacity(block.instructions.len());
        for instruction in std::mem::take(&mut block.instructions) {
            let span = instruction.span;
            let made = |results: Vec<Name>, operation: Operation| Instruction {
                results,
                operation,
                debug_location: instruction.debug_location.clone(),
                span,
            };
            match &instruction.operation {
                Operation::BlockAddress(target) => {
                    let number = fresh(format!("{target}.number"));
                    out.push(made(
                        vec![number.clone()],
                        Operation::ConstInt {
                            ty: Type::Integer(64),
                            value: *numbers.get(target)?,
                        },
                    ));
                    out.push(made(
                        instruction.results.clone(),
                        Operation::Convert {
                            kind: ConvertKind::IntToPointer,
                            value: number,
                            ty: Type::Pointer,
                        },
                    ));
                }
                Operation::BranchIndirect { address, targets } => {
                    let number = fresh(format!("{address}.number"));
                    out.push(made(
                        vec![number.clone()],
                        Operation::Convert {
                            kind: ConvertKind::PointerToInt,
                            value: address.clone(),
                            ty: Type::Integer(64),
                        },
                    ));
                    let cases = cases(function, &numbers, targets)?;
                    let default = nowhere
                        .get_or_insert_with(|| fresh(format!("{}.nowhere", function.name)))
                        .clone();
                    out.push(made(
                        Vec::new(),
                        Operation::SwitchInt(Box::new(IntegerSwitch {
                            value: number,
                            cases,
                            default: plain(&default),
                        })),
                    ));
                }
                _ => out.push(instruction.clone()),
            }
        }
        block.instructions = out;
    }
    if let Some(name) = nowhere {
        let span = result.blocks[0].span;
        result.blocks.push(unreachable_block(name, span));
    }
    Some(result)
}

/// The `switch.int` cases of a `br.indirect` over `targets`, or `None`
/// when one takes block parameters, which no address can pass.
fn cases(
    function: &Function,
    numbers: &HashMap<&Name, i128>,
    targets: &[Name],
) -> Option<Vec<(i128, BlockTarget)>> {
    let mut cases = Vec::with_capacity(targets.len());
    for target in targets {
        let reached = function.blocks.iter().find(|b| &b.name == target)?;
        if !reached.parameters.is_empty() {
            return None;
        }
        let case = (*numbers.get(target)?, plain(target));
        if !cases.contains(&case) {
            cases.push(case);
        }
    }
    Some(cases)
}

fn plain(block: &Name) -> BlockTarget {
    BlockTarget {
        block: block.clone(),
        arguments: Vec::new(),
    }
}

/// Every block and value name of `function`.
fn names(function: &Function) -> HashSet<Name> {
    function
        .blocks
        .iter()
        .flat_map(|block| {
            block
                .parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .chain(block.instructions.iter().flat_map(|i| i.results.clone()))
                .chain(std::iter::once(block.name.clone()))
        })
        .collect()
}

fn unreachable_block(name: Name, span: klf_source::Span) -> Block {
    Block {
        name,
        parameters: Vec::new(),
        instructions: vec![Instruction {
            results: Vec::new(),
            operation: Operation::Unreachable,
            debug_location: None,
            span,
        }],
        span,
    }
}
