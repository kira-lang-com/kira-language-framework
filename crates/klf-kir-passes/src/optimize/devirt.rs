//! Devirtualization of `call.witness` (section 14.1, O2). A witness call on
//! a concrete type goes through that type's witness table for the trait,
//! which the module declares; the call becomes a direct call of the
//! function in the method's slot.

use super::Context;
use klf_kir::{Function, Operation};

/// Turns every witness call whose table the module declares into a direct
/// call.
pub(super) fn devirtualize(function: &mut Function, context: &Context<'_>) -> bool {
    let mut changed = false;
    for instruction in function
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.instructions)
    {
        let Operation::CallWitness(call) = &instruction.operation else {
            continue;
        };
        let Some(callee) = context
            .module
            .witness_callee(&call.ty, &call.trait_name, &call.method)
        else {
            continue;
        };
        instruction.operation = Operation::Call {
            callee: callee.clone(),
            generic_args: Vec::new(),
            arguments: call.arguments.clone(),
        };
        changed = true;
    }
    changed
}
