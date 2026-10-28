//! Address-only lowering (section 11.6 pass 5). Value operations on address-only
//! types (a nominal whose `relocate` policy is a function) are rewritten into
//! `copy.addr`/`move.addr`/`destroy.addr` on stack slots, so no address-only
//! value is ever held by value (verifier V8).

use crate::common::value_types;
use klf_kir::{
    Function, Instruction, Module, ModuleIndex, Operation, RelocatePolicy, StoreMode, Type,
};
use std::collections::BTreeSet;

pub(crate) fn lower_address_only(
    function: &mut Function,
    module: &Module,
    index: &ModuleIndex<'_>,
) {
    let address_only: BTreeSet<&str> = module
        .types
        .iter()
        .filter(|declaration| matches!(declaration.policy.relocate, RelocatePolicy::Function(_)))
        .map(|declaration| declaration.name.as_str())
        .collect();
    if address_only.is_empty() {
        return;
    }
    let types = value_types(function, index);
    let is_address_only = |name: &klf_core::Name| match types.get(name) {
        Some(Type::Nominal(nominal, _)) => address_only.contains(nominal.as_str()),
        Some(Type::Generic(_)) => true,
        _ => false,
    };

    for block in &mut function.blocks {
        let mut rewritten = Vec::with_capacity(block.instructions.len());
        for instruction in block.instructions.drain(..) {
            match &instruction.operation {
                Operation::Copy(value) if is_address_only(value) => {
                    let (dest, ty) = (&instruction.results[0], types[value].clone());
                    rewritten.push(alloc(dest, &ty, instruction.span));
                    rewritten.push(transfer(true, value, dest, instruction.span));
                }
                Operation::Move(value) if is_address_only(value) => {
                    let (dest, ty) = (&instruction.results[0], types[value].clone());
                    rewritten.push(alloc(dest, &ty, instruction.span));
                    rewritten.push(transfer(false, value, dest, instruction.span));
                }
                Operation::Destroy(value) if is_address_only(value) => {
                    rewritten.push(Instruction {
                        results: Vec::new(),
                        operation: Operation::DestroyAddr(value.clone()),
                        debug_location: None,
                        span: instruction.span,
                    });
                }
                _ => rewritten.push(instruction),
            }
        }
        block.instructions = rewritten;
    }
}

fn alloc(dest: &klf_core::Name, ty: &Type, span: klf_source::Span) -> Instruction {
    Instruction {
        results: vec![dest.clone()],
        operation: Operation::StackAlloc(ty.clone()),
        debug_location: None,
        span,
    }
}

fn transfer(
    copy: bool,
    source: &klf_core::Name,
    dest: &klf_core::Name,
    span: klf_source::Span,
) -> Instruction {
    let operation = if copy {
        Operation::CopyAddr {
            source: source.clone(),
            dest: dest.clone(),
            mode: StoreMode::Init,
        }
    } else {
        Operation::MoveAddr {
            source: source.clone(),
            dest: dest.clone(),
            mode: StoreMode::Init,
        }
    };
    Instruction {
        results: Vec::new(),
        operation,
        debug_location: None,
        span,
    }
}
