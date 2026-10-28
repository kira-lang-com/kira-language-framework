//! Shared ownership analysis: the CFG, owned-value typing and consume counting.

use klf_core::Name;
use klf_kir::{BlockTarget, Function, ModuleIndex, Operation, Type};
use rustc_hash::FxHashMap as HashMap;

pub(crate) fn is_owned(ty: Option<&Type>) -> bool {
    matches!(
        ty,
        Some(Type::Reference(_) | Type::Unique(_) | Type::Weak(_))
    )
}

/// Value types for every SSA value in `function`, resolving call results against
/// the module `index` (whose function signatures are unchanged by these passes).
pub(crate) fn value_types(function: &Function, index: &ModuleIndex<'_>) -> klf_kir::ValueTypes {
    klf_kir::infer_value_types_in(function, index)
}

/// How many times a non-terminator operation consumes `value`.
pub(crate) fn body_consumes(operation: &Operation, value: &Name) -> usize {
    let count = |name: &Name| usize::from(name == value);
    match operation {
        Operation::Move(name) | Operation::Destroy(name) | Operation::IntoRaw(name) => count(name),
        Operation::Store { value: stored, .. } => count(stored),
        Operation::Call { arguments, .. } => arguments.iter().map(count).sum(),
        _ => 0,
    }
}

pub(crate) fn edge_consumes(target: &BlockTarget, value: &Name) -> usize {
    target.arguments.iter().filter(|arg| *arg == value).count()
}

/// Control-flow graph as successor and predecessor block indices.
pub(crate) struct Cfg {
    pub(crate) successors: Vec<Vec<usize>>,
    pub(crate) predecessors: Vec<Vec<usize>>,
}

impl Cfg {
    pub(crate) fn build(function: &Function) -> Self {
        let index: HashMap<&str, usize> = function
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| (block.name.as_str(), i))
            .collect();
        let mut successors = vec![Vec::new(); function.blocks.len()];
        let mut predecessors = vec![Vec::new(); function.blocks.len()];
        for (i, block) in function.blocks.iter().enumerate() {
            if let Some(terminator) = block.instructions.last() {
                for target in terminator.operation.targets() {
                    if let Some(&s) = index.get(target.block.as_str()) {
                        successors[i].push(s);
                        predecessors[s].push(i);
                    }
                }
            }
        }
        Self {
            successors,
            predecessors,
        }
    }
}
