//! The functions a module's roots can reach.
//!
//! A function is reached through a call, `try.call`, `invoke`, variadic call
//! or function reference in a reached body. Vtables, witness tables and type
//! policies name functions that run without such an instruction, so those
//! functions are roots of their own.

use crate::{CopyPolicy, DestroyPolicy, Module, Operation, RelocatePolicy};
use klf_core::Name;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

impl Module {
    /// The indices in `functions` of every function `roots` reach.
    #[must_use]
    pub fn reachable_functions<'a>(
        &'a self,
        roots: impl IntoIterator<Item = &'a Name>,
    ) -> HashSet<usize> {
        let index: HashMap<&Name, usize> = self
            .functions
            .iter()
            .enumerate()
            .map(|(position, function)| (&function.name, position))
            .collect();
        let mut reached = HashSet::default();
        let mut pending: Vec<usize> = roots
            .into_iter()
            .chain(self.implicit_roots())
            .filter_map(|name| index.get(name).copied())
            .collect();
        while let Some(position) = pending.pop() {
            if !reached.insert(position) {
                continue;
            }
            let body = self.functions[position]
                .blocks
                .iter()
                .flat_map(|block| &block.instructions);
            pending.extend(
                body.filter_map(|instruction| named_function(&instruction.operation))
                    .filter_map(|name| index.get(name).copied())
                    .filter(|callee| !reached.contains(callee)),
            );
        }
        reached
    }

    /// Every function an instruction, vtable, witness table or type policy
    /// of the module names: the ones an object file needs a symbol for.
    #[must_use]
    pub fn named_functions(&self) -> HashSet<&Name> {
        self.functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| named_function(&instruction.operation))
            .chain(self.implicit_roots())
            .collect()
    }

    pub(crate) fn implicit_roots(&self) -> impl Iterator<Item = &Name> {
        let tables = self.vtables.iter().flat_map(|vtable| &vtable.entries);
        let witnesses = self.witnesses.iter().flat_map(|table| &table.witnesses);
        let policies = self.types.iter().flat_map(|declaration| {
            let policy = &declaration.policy;
            let copy = match &policy.copy {
                CopyPolicy::Implicit(name) | CopyPolicy::Explicit(name) => Some(name),
                CopyPolicy::Trivial | CopyPolicy::None => None,
            };
            let destroy = match &policy.destroy {
                DestroyPolicy::Function(name) => Some(name),
                DestroyPolicy::Trivial => None,
            };
            let relocate = match &policy.relocate {
                RelocatePolicy::Function(name) => Some(name),
                RelocatePolicy::Bitwise => None,
            };
            copy.into_iter().chain(destroy).chain(relocate)
        });
        tables.chain(witnesses).chain(policies)
    }
}

fn named_function(operation: &Operation) -> Option<&Name> {
    match operation {
        Operation::Call { callee, .. }
        | Operation::CallVariadic { callee, .. }
        | Operation::FunctionReference(callee)
        | Operation::CallForeign { symbol: callee, .. } => Some(callee),
        Operation::TryCall(call) => Some(&call.callee),
        Operation::Invoke(call) => Some(&call.callee),
        _ => None,
    }
}
