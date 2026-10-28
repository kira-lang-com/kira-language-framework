use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::Function;
use rustc_hash::FxHashMap as HashMap;

pub(crate) fn collect_predecessors(
    function: &Function,
    blocks: &HashMap<Name, usize>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Vec<usize>> {
    let mut predecessors = vec![Vec::new(); function.blocks.len()];
    for (source, block) in function.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            for target in instruction.operation.targets() {
                if let Some(destination) = blocks.get(&target.block) {
                    predecessors[*destination].push(source);
                } else {
                    diagnostics.push(
                        Diagnostic::error(
                            "KLF-KIR-1001",
                            format!("branch targets unknown block `{}`", target.block),
                        )
                        .with_label(instruction.span, "unknown block target"),
                    );
                }
            }
        }
    }
    predecessors
}

/// Which blocks dominate which: the immediate-dominator tree of the reachable
/// blocks (Cooper, Harvey and Kennedy), numbered so a query is two
/// comparisons. Memory and time stay linear in the blocks, so a function of
/// tens of thousands of blocks verifies without a set per block.
pub(crate) struct Dominance {
    entered: Vec<usize>,
    exited: Vec<usize>,
}

const UNREACHED: usize = usize::MAX;

impl Dominance {
    /// Whether every path from the entry to `block` passes `dominator`. An
    /// unreachable block never runs, so every block dominates it, as the
    /// usual definition of dominance has it; code lowered after a `break` or
    /// a throw is valid however it uses earlier values.
    pub(crate) fn dominates(&self, dominator: usize, block: usize) -> bool {
        if self.entered[block] == UNREACHED {
            return true;
        }
        if self.entered[dominator] == UNREACHED {
            return false;
        }
        self.entered[dominator] <= self.entered[block]
            && self.exited[block] <= self.exited[dominator]
    }
}

pub(crate) fn compute_dominators(block_count: usize, predecessors: &[Vec<usize>]) -> Dominance {
    let order = reverse_postorder(block_count, predecessors);
    let mut rank = vec![UNREACHED; block_count];
    for (position, block) in order.iter().enumerate() {
        rank[*block] = position;
    }
    let mut idom = vec![UNREACHED; block_count];
    if let Some(entry) = order.first() {
        idom[*entry] = *entry;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for block in order.iter().skip(1) {
            let mut next = UNREACHED;
            for predecessor in &predecessors[*block] {
                if idom[*predecessor] == UNREACHED {
                    continue;
                }
                next = if next == UNREACHED {
                    *predecessor
                } else {
                    intersect(&idom, &rank, *predecessor, next)
                };
            }
            if next != UNREACHED && idom[*block] != next {
                idom[*block] = next;
                changed = true;
            }
        }
    }
    number_tree(block_count, &order, &idom)
}

fn intersect(idom: &[usize], rank: &[usize], first: usize, second: usize) -> usize {
    let (mut left, mut right) = (first, second);
    while left != right {
        while rank[left] > rank[right] {
            left = idom[left];
        }
        while rank[right] > rank[left] {
            right = idom[right];
        }
    }
    left
}

fn number_tree(block_count: usize, order: &[usize], idom: &[usize]) -> Dominance {
    let mut children = vec![Vec::new(); block_count];
    for block in order.iter().skip(1) {
        children[idom[*block]].push(*block);
    }
    let mut entered = vec![UNREACHED; block_count];
    let mut exited = vec![UNREACHED; block_count];
    let mut clock = 0;
    let mut stack: Vec<(usize, usize)> =
        order.first().map(|entry| (*entry, 0)).into_iter().collect();
    while let Some((block, next_child)) = stack.pop() {
        if next_child == 0 {
            entered[block] = clock;
            clock += 1;
        }
        if let Some(child) = children[block].get(next_child) {
            stack.push((block, next_child + 1));
            stack.push((*child, 0));
        } else {
            exited[block] = clock;
            clock += 1;
        }
    }
    Dominance { entered, exited }
}

/// The blocks reachable from the entry, block 0, each after every block that
/// reaches it first along a depth-first walk.
fn reverse_postorder(block_count: usize, predecessors: &[Vec<usize>]) -> Vec<usize> {
    if block_count == 0 {
        return Vec::new();
    }
    let mut successors = vec![Vec::new(); block_count];
    for (block, incoming) in predecessors.iter().enumerate() {
        for predecessor in incoming {
            successors[*predecessor].push(block);
        }
    }
    let mut visited = vec![false; block_count];
    let mut postorder = Vec::with_capacity(block_count);
    let mut stack = vec![(0, 0)];
    visited[0] = true;
    while let Some((block, next_successor)) = stack.pop() {
        if let Some(successor) = successors[block].get(next_successor) {
            stack.push((block, next_successor + 1));
            if !visited[*successor] {
                visited[*successor] = true;
                stack.push((*successor, 0));
            }
        } else {
            postorder.push(block);
        }
    }
    postorder.reverse();
    postorder
}
