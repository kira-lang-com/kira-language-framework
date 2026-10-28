//! The call graph the optimizer goes up, callees before their callers.

use klf_core::Name;
use klf_kir::{Function, Operation};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) struct CallGraph {
    /// Function positions in waves, each calling only into earlier waves
    /// and its own cycles.
    pub(super) waves: Vec<Vec<usize>>,
    /// For each function on a cycle of direct calls, the cycle's number.
    pub(super) cycle: Vec<Option<usize>>,
    /// The functions some function calls directly.
    pub(super) called: HashSet<usize>,
}

/// The functions `function` calls directly, each once.
pub(super) fn calls_of(function: &Function) -> Vec<Name> {
    let mut seen: HashSet<&Name> = HashSet::default();
    let mut found: Vec<Name> = Vec::new();
    for instruction in function.blocks.iter().flat_map(|block| &block.instructions) {
        if let Operation::Call { callee, .. } = &instruction.operation
            && seen.insert(callee)
        {
            found.push(callee.clone());
        }
    }
    found
}

/// The call graph of functions calling, each, the functions `calls` names.
pub(super) fn bottom_up(calls: &[&[Name]], positions: &HashMap<Name, usize>) -> CallGraph {
    let callees: Vec<Vec<usize>> = calls
        .iter()
        .map(|names| {
            let mut found: Vec<usize> = names
                .iter()
                .filter_map(|name| positions.get(name).copied())
                .collect();
            found.sort_unstable();
            found.dedup();
            found
        })
        .collect();
    let components = strongly_connected(&callees);
    let mut cycle = vec![None; calls.len()];
    for (number, component) in components.iter().enumerate() {
        if component.len() > 1 || callees[component[0]].contains(&component[0]) {
            for &member in component {
                cycle[member] = Some(number);
            }
        }
    }
    // A component's wave is one past the latest wave of what it calls
    // outside itself.
    let mut component_of = vec![0; calls.len()];
    for (number, component) in components.iter().enumerate() {
        for &member in component {
            component_of[member] = number;
        }
    }
    let mut waves: Vec<Vec<usize>> = Vec::new();
    let mut wave_of = vec![0; components.len()];
    for (number, component) in components.iter().enumerate() {
        let wave = component
            .iter()
            .flat_map(|&member| &callees[member])
            .filter(|&&callee| component_of[callee] != number)
            .map(|&callee| wave_of[component_of[callee]] + 1)
            .max()
            .unwrap_or(0);
        wave_of[number] = wave;
        if waves.len() <= wave {
            waves.resize_with(wave + 1, Vec::new);
        }
        waves[wave].extend(component);
    }
    CallGraph {
        waves,
        cycle,
        called: callees.iter().flatten().copied().collect(),
    }
}

/// Tarjan's algorithm, iteratively. Components come out callees first.
pub(super) fn strongly_connected(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let count = edges.len();
    let mut index = vec![usize::MAX; count];
    let mut low = vec![0; count];
    let mut on_stack = vec![false; count];
    let mut stack = Vec::new();
    let mut components = Vec::new();
    let mut next = 0;
    for root in 0..count {
        if index[root] != usize::MAX {
            continue;
        }
        let mut frames = vec![(root, 0usize)];
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        while let Some(frame) = frames.last_mut() {
            let node = frame.0;
            if let Some(&successor) = edges[node].get(frame.1) {
                frame.1 += 1;
                if index[successor] == usize::MAX {
                    index[successor] = next;
                    low[successor] = next;
                    next += 1;
                    stack.push(successor);
                    on_stack[successor] = true;
                    frames.push((successor, 0));
                } else if on_stack[successor] {
                    low[node] = low[node].min(index[successor]);
                }
                continue;
            }
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                components.push(component);
            }
        }
    }
    components
}
