//! Witness calls on a concrete type (section 12.11): `call.witness T,
//! Trait.method(args)` is resolved statically when `T` is concrete. When
//! the module declares `T`'s witness table for the trait, the call becomes
//! a direct call of the function in the method's slot, before any backend
//! lowers the module and at every optimization level.

use crate::{Module, Operation, Type};
use klf_core::Name;

impl Module {
    /// The function in `method`'s slot of the witness table this module
    /// declares for `ty` and `trait_name`, or `None` when `ty` is generic
    /// or the module declares no such table or method.
    #[must_use]
    pub fn witness_callee(&self, ty: &Type, trait_name: &Name, method: &Name) -> Option<&Name> {
        if matches!(ty, Type::Generic(_)) {
            return None;
        }
        let slot = self
            .traits
            .iter()
            .find(|declaration| &declaration.name == trait_name)?
            .methods
            .iter()
            .position(|candidate| &candidate.name == method)?;
        self.witnesses
            .iter()
            .find(|table| &table.ty == ty && &table.trait_name == trait_name)?
            .witnesses
            .get(slot)
    }
}

/// The module with every witness call it can resolve made direct, or
/// `None` when it has none.
#[must_use]
pub fn witness_calls_to_direct(module: &Module) -> Option<Module> {
    let mut result = module.clone();
    let mut changed = false;
    for (index, function) in module.functions.iter().enumerate() {
        for (block_index, block) in function.blocks.iter().enumerate() {
            for (position, instruction) in block.instructions.iter().enumerate() {
                let Operation::CallWitness(call) = &instruction.operation else {
                    continue;
                };
                let Some(callee) = module.witness_callee(&call.ty, &call.trait_name, &call.method)
                else {
                    continue;
                };
                result.functions[index].blocks[block_index].instructions[position].operation =
                    Operation::Call {
                        callee: callee.clone(),
                        generic_args: Vec::new(),
                        arguments: call.arguments.clone(),
                    };
                changed = true;
            }
        }
    }
    changed.then_some(result)
}
