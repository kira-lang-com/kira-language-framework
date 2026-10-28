//! Register assignment for one function. A frame holds at most 65,535
//! registers (section 15.3), so SSA values share registers once their live
//! ranges no longer overlap.
//!
//! Each value gets one interval over a linear numbering of the function: the
//! hull of its definition, its uses, the starts of the blocks it is live into
//! and the terminators of the blocks it is live out of. A block parameter is
//! written by every predecessor's terminator, so its interval covers those
//! terminators too. Intervals that share a position never share a register,
//! which keeps an instruction from overwriting an operand it still reads.

use super::{LowerError, err};
use klf_core::Name;
use klf_kir::Function;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

pub(super) struct Assignment {
    pub registers: HashMap<Name, u16>,
    pub count: u32,
}

#[derive(Clone, Copy)]
struct Interval {
    start: u32,
    end: u32,
}

impl Interval {
    fn cover(&mut self, position: u32) {
        self.start = self.start.min(position);
        self.end = self.end.max(position);
    }
}

struct Layout<'a> {
    starts: Vec<u32>,
    terminators: Vec<u32>,
    predecessors: Vec<Vec<usize>>,
    values: HashMap<&'a Name, usize>,
    definitions: Vec<usize>,
    intervals: Vec<Interval>,
}

pub(super) fn assign(function: &Function) -> Result<Assignment, LowerError> {
    let mut layout = Layout::number(function);
    layout.extend_live_ranges(function);
    layout.extend_block_parameters(function);
    layout.scan(function)
}

impl<'a> Layout<'a> {
    fn number(function: &'a Function) -> Self {
        let blocks: HashMap<&Name, usize> = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (&block.name, index))
            .collect();
        let mut layout = Self {
            starts: Vec::with_capacity(function.blocks.len()),
            terminators: Vec::with_capacity(function.blocks.len()),
            predecessors: vec![Vec::new(); function.blocks.len()],
            values: HashMap::new(),
            definitions: Vec::new(),
            intervals: Vec::new(),
        };
        for parameter in &function.parameters {
            layout.define(&parameter.name, 0, 0);
        }
        let mut position = 0u32;
        for (index, block) in function.blocks.iter().enumerate() {
            layout.starts.push(position);
            for parameter in &block.parameters {
                layout.define(&parameter.name, index, position);
            }
            for instruction in &block.instructions {
                position += 1;
                for result in &instruction.results {
                    layout.define(result, index, position);
                }
                for successor in instruction.operation.successors() {
                    if let Some(&target) = blocks.get(successor) {
                        layout.predecessors[target].push(index);
                    }
                }
            }
            layout.terminators.push(position);
            position += 1;
        }
        layout
    }

    fn define(&mut self, name: &'a Name, block: usize, position: u32) {
        if self.values.contains_key(name) {
            return;
        }
        self.values.insert(name, self.intervals.len());
        self.definitions.push(block);
        self.intervals.push(Interval {
            start: position,
            end: position,
        });
    }

    /// Walks back from each use to the defining block, marking the blocks the
    /// value is live into and out of.
    fn extend_live_ranges(&mut self, function: &Function) {
        let mut visited = vec![usize::MAX; function.blocks.len()];
        let mut pending = Vec::new();
        for (index, block) in function.blocks.iter().enumerate() {
            let mut position = self.starts[index];
            for instruction in &block.instructions {
                position += 1;
                for used in instruction.operation.used_values() {
                    let Some(&value) = self.values.get(used) else {
                        continue;
                    };
                    self.intervals[value].cover(position);
                    if self.definitions[value] == index {
                        continue;
                    }
                    pending.push(index);
                    while let Some(live_in) = pending.pop() {
                        if visited[live_in] == value {
                            continue;
                        }
                        visited[live_in] = value;
                        self.intervals[value].cover(self.starts[live_in]);
                        for &predecessor in &self.predecessors[live_in] {
                            self.intervals[value].cover(self.terminators[predecessor]);
                            if self.definitions[value] != predecessor {
                                pending.push(predecessor);
                            }
                        }
                    }
                }
            }
        }
    }

    fn extend_block_parameters(&mut self, function: &Function) {
        for (index, block) in function.blocks.iter().enumerate() {
            for parameter in &block.parameters {
                let value = self.values[&parameter.name];
                for &predecessor in &self.predecessors[index] {
                    self.intervals[value].cover(self.terminators[predecessor]);
                }
            }
        }
    }

    /// Linear scan: the function's parameters keep registers 0..n, as the
    /// call convention places arguments there. In a function that runs
    /// `setjmp` no two values share a register: a `longjmp` resumes it with
    /// whatever its registers hold, so a value the path to the `longjmp` no
    /// longer needed must still be there.
    fn scan(self, function: &Function) -> Result<Assignment, LowerError> {
        let shared = !function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(instruction.operation, klf_kir::Operation::SetJmp(_)));
        let parameters = function.parameters.len();
        let mut order: Vec<usize> = (parameters..self.intervals.len()).collect();
        order.sort_by_key(|&value| self.intervals[value].start);
        let mut assigned = vec![0u16; self.intervals.len()];
        let mut active: BinaryHeap<Reverse<(u32, u16)>> = BinaryHeap::new();
        let mut free: BinaryHeap<Reverse<u16>> = BinaryHeap::new();
        let mut count = 0u32;
        for (slot, interval) in assigned.iter_mut().zip(&self.intervals).take(parameters) {
            *slot = take(&mut count)?;
            active.push(Reverse((interval.end, *slot)));
        }
        for value in order {
            let interval = self.intervals[value];
            while let Some(&Reverse((end, register))) = active.peek() {
                if end >= interval.start {
                    break;
                }
                active.pop();
                free.push(Reverse(register));
            }
            let register = match if shared { free.pop() } else { None } {
                Some(Reverse(register)) => register,
                None => take(&mut count)?,
            };
            assigned[value] = register;
            active.push(Reverse((interval.end, register)));
        }
        let registers = self
            .values
            .into_iter()
            .map(|(name, value)| (name.clone(), assigned[value]))
            .collect();
        Ok(Assignment { registers, count })
    }
}

fn take(count: &mut u32) -> Result<u16, LowerError> {
    let register =
        u16::try_from(*count).map_err(|_| err("function needs more than 65535 live registers"))?;
    *count += 1;
    Ok(register)
}
