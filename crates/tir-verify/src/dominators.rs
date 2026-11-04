pub struct DominatorTree {
    components: Vec<usize>,
    roots: Vec<usize>,
    starts: Vec<usize>,
    ends: Vec<usize>,
}

impl DominatorTree {
    pub fn compute(successors: &[Vec<usize>]) -> Self {
        let count = successors.len();
        let mut components = vec![usize::MAX; count];
        let mut roots = Vec::new();
        for root in 0..count {
            if components[root] != usize::MAX {
                continue;
            }
            let component = roots.len();
            roots.push(root);
            components[root] = component;
            let mut stack = vec![root];
            while let Some(block) = stack.pop() {
                for &target in &successors[block] {
                    if components[target] == usize::MAX {
                        components[target] = component;
                        stack.push(target);
                    }
                }
            }
        }

        let mut predecessors = vec![Vec::new(); count];
        for (source, targets) in successors.iter().enumerate() {
            for &target in targets {
                predecessors[target].push(source);
            }
        }

        let mut visited = vec![false; count];
        let mut postorder = Vec::with_capacity(count);
        let mut reverse_postorder = Vec::with_capacity(count);
        let mut rpo_index = vec![usize::MAX; count];
        let mut immediate_dominator = vec![None; count];
        for (component, root) in roots.iter().copied().enumerate() {
            append_postorder(
                root,
                component,
                successors,
                &components,
                &mut visited,
                &mut postorder,
            );
            let start = reverse_postorder.len();
            reverse_postorder.extend(postorder.drain(..).rev());
            for index in start..reverse_postorder.len() {
                rpo_index[reverse_postorder[index]] = index - start;
            }
            immediate_dominator[root] = Some(root);

            loop {
                let mut changed = false;
                for block in reverse_postorder[start + 1..].iter().copied() {
                    let mut reachable_predecessors = predecessors[block]
                        .iter()
                        .copied()
                        .filter(|predecessor| immediate_dominator[*predecessor].is_some());
                    let Some(mut next) = reachable_predecessors.next() else {
                        continue;
                    };
                    for predecessor in reachable_predecessors {
                        next = intersect_dominators(
                            next,
                            predecessor,
                            &immediate_dominator,
                            &rpo_index,
                        );
                    }
                    if immediate_dominator[block] != Some(next) {
                        immediate_dominator[block] = Some(next);
                        changed = true;
                    }
                }
                if !changed {
                    break;
                }
            }
        }

        let mut children = vec![Vec::new(); count];
        for block in 0..count {
            if let Some(parent) = immediate_dominator[block]
                && parent != block
            {
                children[parent].push(block);
            }
        }
        let mut starts = vec![0; count];
        let mut ends = vec![0; count];
        let mut clock = 0;
        for root in roots.iter().copied() {
            let mut stack = vec![(root, false)];
            while let Some((block, exiting)) = stack.pop() {
                if exiting {
                    ends[block] = clock;
                    continue;
                }
                starts[block] = clock;
                clock += 1;
                stack.push((block, true));
                for child in children[block].iter().rev().copied() {
                    stack.push((child, false));
                }
            }
        }

        Self {
            components,
            roots,
            starts,
            ends,
        }
    }

    pub fn dominates(&self, dominator: usize, block: usize) -> bool {
        self.components[dominator] == self.components[block]
            && self.starts[dominator] <= self.starts[block]
            && self.starts[block] < self.ends[dominator]
    }

    pub fn components(&self) -> &[usize] {
        &self.components
    }

    pub fn roots(&self) -> &[usize] {
        &self.roots
    }

    pub fn interval(&self, block: usize) -> (usize, usize) {
        (self.starts[block], self.ends[block])
    }
}

fn append_postorder(
    root: usize,
    component: usize,
    successors: &[Vec<usize>],
    components: &[usize],
    visited: &mut [bool],
    postorder: &mut Vec<usize>,
) {
    visited[root] = true;
    let mut stack = vec![(root, 0usize)];
    while let Some((block, next_successor)) = stack.last().copied() {
        let next = successors[block]
            .get(next_successor)
            .copied()
            .filter(|target| components[*target] == component);
        if let Some(target) = next {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
            if !visited[target] {
                visited[target] = true;
                stack.push((target, 0));
            }
        } else if next_successor < successors[block].len() {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
        } else {
            postorder.push(block);
            stack.pop();
        }
    }
}

fn intersect_dominators(
    mut left: usize,
    mut right: usize,
    immediate_dominator: &[Option<usize>],
    rpo_index: &[usize],
) -> usize {
    while left != right {
        while rpo_index[left] > rpo_index[right] {
            let Some(parent) = immediate_dominator[left] else {
                return left;
            };
            left = parent;
        }
        while rpo_index[right] > rpo_index[left] {
            let Some(parent) = immediate_dominator[right] else {
                return right;
            };
            right = parent;
        }
    }
    left
}
