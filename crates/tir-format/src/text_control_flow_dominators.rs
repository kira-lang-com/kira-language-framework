//! Dominance of text-level blocks before CODE exists. The binary verifier
//! independently recomputes dominance over decoded instruction operands.

use crate::text_control_flow_assembly::ControlTerminator;

pub(crate) struct FlowDominators {
    successors: Vec<Vec<usize>>,
    components: Vec<usize>,
    roots: Vec<usize>,
}

impl FlowDominators {
    pub(crate) fn new(terminators: &[ControlTerminator], labels: &[&str]) -> Self {
        let successors = terminators
            .iter()
            .map(|terminator| {
                let targets: Vec<&str> = match terminator {
                    ControlTerminator::Branch(target) => vec![&target.label],
                    ControlTerminator::Conditional {
                        when_true,
                        when_false,
                        ..
                    } => {
                        vec![&when_true.label, &when_false.label]
                    }
                    ControlTerminator::Return(_) => Vec::new(),
                };
                targets
                    .iter()
                    .filter_map(|label| labels.iter().position(|candidate| candidate == label))
                    .collect()
            })
            .collect::<Vec<Vec<usize>>>();
        let mut components = vec![usize::MAX; labels.len()];
        let mut roots = Vec::new();
        for root in 0..labels.len() {
            if components[root] != usize::MAX {
                continue;
            }
            let component = roots.len();
            roots.push(root);
            let mut pending = vec![root];
            components[root] = component;
            while let Some(block) = pending.pop() {
                for &target in &successors[block] {
                    if components[target] == usize::MAX {
                        components[target] = component;
                        pending.push(target);
                    }
                }
            }
        }
        Self {
            successors,
            components,
            roots,
        }
    }

    pub(crate) fn dominates(&self, definition: usize, use_block: usize) -> bool {
        if self.components.get(definition) != self.components.get(use_block) {
            return false;
        }
        if definition == use_block {
            return true;
        }
        let component = self.components[use_block];
        let root = self.roots[component];
        let mut visited = vec![false; self.successors.len()];
        let mut pending = vec![root];
        while let Some(block) = pending.pop() {
            if block == definition || visited[block] || self.components[block] != component {
                continue;
            }
            if block == use_block {
                // There is an entry path that avoids the definition.
                return false;
            }
            visited[block] = true;
            pending.extend(self.successors[block].iter().copied());
        }
        true
    }
}
