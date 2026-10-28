//! The functions that can take part in recursion.
//!
//! A backend counts call depth to stop runaway recursion (`KLF-RT-0021`), and
//! the count costs every call it is kept on. Recursion needs a cycle, and a
//! cycle of calls either stays among direct calls, where it is a strongly
//! connected component of the direct call graph, or passes through a function
//! entered some other way: through a reference to it, a vtable, a witness
//! table, a type's copy, destroy or relocate policy, or from outside the
//! module. Counting the functions in those cycles and every function entered
//! another way counts at least one function of every cycle there can be, so
//! the depth still rises without bound on every recursion that has none, while
//! a leaf such as a byte comparison is never counted at all.

use crate::{Module, Operation};
use klf_core::Name;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

impl Module {
    /// The indices in `functions` of the functions whose depth must be
    /// counted: those on a cycle of direct calls, those entered other than by
    /// a direct call, and those that call a function defined elsewhere, which
    /// may call back in.
    #[must_use]
    pub fn recursive_functions(&self) -> HashSet<usize> {
        let index: HashMap<&Name, usize> = self
            .functions
            .iter()
            .enumerate()
            .map(|(position, function)| (&function.name, position))
            .collect();
        let mut counted = HashSet::default();
        let mut callees: Vec<Vec<usize>> = vec![Vec::new(); self.functions.len()];
        let mut entered: Vec<&Name> = self.implicit_roots().collect();
        for (position, function) in self.functions.iter().enumerate() {
            if function.linkage == crate::Linkage::Exported || function.c_prototype.is_some() {
                counted.insert(position);
            }
            let body = function.blocks.iter().flat_map(|block| &block.instructions);
            for instruction in body {
                match &instruction.operation {
                    Operation::FunctionReference(name) => entered.push(name),
                    operation => {
                        let Some(callee) = direct_callee(operation) else {
                            continue;
                        };
                        match index.get(callee) {
                            Some(&target) if !self.functions[target].foreign => {
                                callees[position].push(target);
                            }
                            Some(_) => {}
                            None => {
                                counted.insert(position);
                            }
                        }
                    }
                }
            }
        }
        counted.extend(
            entered
                .into_iter()
                .filter_map(|name| index.get(name).copied()),
        );
        counted.extend(cyclic(&callees));
        counted
    }
}

fn direct_callee(operation: &Operation) -> Option<&Name> {
    match operation {
        Operation::Call { callee, .. } | Operation::CallVariadic { callee, .. } => Some(callee),
        Operation::TryCall(call) => Some(&call.callee),
        Operation::Invoke(call) => Some(&call.callee),
        _ => None,
    }
}

/// The nodes of `edges` on a cycle: each strongly connected component of more
/// than one node, and each node with an edge to itself. Tarjan's algorithm,
/// with an explicit stack so a deep call chain cannot overflow this one.
fn cyclic(edges: &[Vec<usize>]) -> Vec<usize> {
    const UNVISITED: usize = usize::MAX;
    let mut order = vec![UNVISITED; edges.len()];
    let mut low = vec![0; edges.len()];
    let mut on_stack = vec![false; edges.len()];
    let mut stack = Vec::new();
    let mut found = Vec::new();
    let mut next = 0;
    for root in 0..edges.len() {
        if order[root] != UNVISITED {
            continue;
        }
        // Each frame is a node and how many of its edges it has followed.
        let mut frames = vec![(root, 0)];
        order[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(&mut (node, ref mut followed)) = frames.last_mut() {
            if let Some(&target) = edges[node].get(*followed) {
                *followed += 1;
                if order[target] == UNVISITED {
                    order[target] = next;
                    low[target] = next;
                    next += 1;
                    stack.push(target);
                    on_stack[target] = true;
                    frames.push((target, 0));
                } else if on_stack[target] {
                    low[node] = low[node].min(order[target]);
                }
                continue;
            }
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == order[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                if component.len() > 1 || edges[node].contains(&node) {
                    found.extend(component);
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::cyclic;

    #[test]
    fn only_nodes_on_a_cycle_are_cyclic() {
        // 0 -> 1 -> 2 -> 0 is a cycle, 3 calls itself, 4 -> 5 is a chain.
        let edges = vec![vec![1], vec![2, 4], vec![0], vec![3], vec![5], vec![]];
        let mut found = cyclic(&edges);
        found.sort_unstable();
        assert_eq!(found, vec![0, 1, 2, 3]);
    }
}
