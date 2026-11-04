use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryEntryKind, BinaryFunctionBody, BinaryGpuEntry, BinaryGpuTable, BinaryImport,
    BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable,
};

use super::CallEdge;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const ZERO: u8 = 1;
const ONCE: u8 = 2;
const MANY: u8 = 4;

type CallTargets = HashMap<u64, Vec<usize>>;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Rule {
    Dispatch,
    Counts,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum IssueKind {
    Repeated,
    WriteBeforeCounts,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Issue {
    function: usize,
    block: usize,
    instruction: usize,
    kind: IssueKind,
}

#[derive(Clone, Default)]
struct Summary {
    returns: u8,
    terminates: u8,
    issues: HashSet<Issue>,
}

struct Analyzer<'a> {
    bodies: &'a [Option<&'a BinaryFunctionBody>],
    calls: &'a [CallTargets],
    rule: Rule,
    summaries: Vec<[Option<Summary>; 3]>,
    active: HashSet<(usize, u8)>,
}

pub(super) fn verify_entries(
    bodies: &[Option<&BinaryFunctionBody>],
    signatures: &BinarySignatureTable,
    graph: &[Vec<CallEdge>],
    gpu: Option<&BinaryGpuTable>,
    types: &BinaryTypeTable,
    imports: Option<&[BinaryImport]>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut calls = (0..graph.len())
        .map(|_| HashMap::<u64, Vec<usize>>::new())
        .collect::<Vec<_>>();
    for (caller, edges) in graph.iter().enumerate() {
        for edge in edges {
            let targets = calls[caller].entry(edge.offset).or_default();
            if !targets.contains(&edge.target) {
                targets.push(edge.target);
            }
        }
    }
    for (entry, signature) in signatures.records.iter().enumerate() {
        if let Some(entry_kind) = signature.entry_kind {
            verify_geometry_stage(entry, entry_kind, bodies, signatures, graph, diagnostics);
        }
        let rule = match signature.entry_kind {
            Some(BinaryEntryKind::Object) => Rule::Dispatch,
            Some(BinaryEntryKind::Mesh) => Rule::Counts,
            _ => continue,
        };
        let mut analyzer = Analyzer {
            bodies,
            calls: &calls,
            rule,
            summaries: (0..signatures.records.len())
                .map(|_| [None, None, None])
                .collect(),
            active: HashSet::new(),
        };
        let summary = analyzer.function(entry, ZERO);
        for issue in summary.issues {
            let Some(function) = signatures.records.get(issue.function) else {
                continue;
            };
            let Some(instruction) = bodies
                .get(issue.function)
                .copied()
                .flatten()
                .and_then(|body| body.blocks.get(issue.block))
                .and_then(|block| block.instructions.as_ref())
                .and_then(|instructions| instructions.get(issue.instruction))
            else {
                continue;
            };
            let message = match (rule, issue.kind) {
                (Rule::Dispatch, IssueKind::Repeated) => {
                    "object.dispatch_mesh may execute more than once on a control-flow path"
                }
                (Rule::Counts, IssueKind::Repeated) => {
                    "mesh.set_counts may execute more than once on a control-flow path"
                }
                (Rule::Counts, IssueKind::WriteBeforeCounts) => {
                    "mesh output write may execute before mesh.set_counts"
                }
                _ => continue,
            };
            diagnostics.push(Diagnostic::error(
                V2,
                format!(
                    "function @{} CODE offset {}: {message}",
                    function.name, instruction.offset
                ),
            ));
        }
        if (summary.returns | summary.terminates) & ZERO != 0 {
            let message = match rule {
                Rule::Dispatch => {
                    "object.dispatch_mesh must execute exactly once on every control-flow path"
                }
                Rule::Counts => {
                    "mesh.set_counts must execute exactly once on every control-flow path"
                }
            };
            diagnostics.push(Diagnostic::error(
                V2,
                format!("GPU entry @{}: {message}", signature.name),
            ));
        }
        if rule == Rule::Counts
            && let Some(gpu_entry) = gpu.and_then(|gpu| {
                gpu.entries
                    .iter()
                    .find(|item| item.function_index == entry as u64)
            })
        {
            verify_mesh_indices(
                gpu_entry,
                bodies,
                signatures,
                graph,
                types,
                imports,
                diagnostics,
            );
        }
    }
}

fn verify_geometry_stage(
    entry: usize,
    entry_kind: BinaryEntryKind,
    bodies: &[Option<&BinaryFunctionBody>],
    signatures: &BinarySignatureTable,
    graph: &[Vec<CallEdge>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut visited = vec![false; graph.len()];
    let mut pending = vec![entry];
    while let Some(function_index) = pending.pop() {
        let Some(seen) = visited.get_mut(function_index) else {
            continue;
        };
        if *seen {
            continue;
        }
        *seen = true;
        let Some(body) = bodies.get(function_index).copied().flatten() else {
            continue;
        };
        let Some(signature) = signatures.records.get(function_index) else {
            continue;
        };
        for instruction in body
            .blocks
            .iter()
            .filter_map(|block| block.instructions.as_ref())
            .flatten()
        {
            let required_stage = match instruction.opcode {
                0x4c0 => Some((BinaryEntryKind::Object, "object.dispatch_mesh")),
                0x4c1 => Some((BinaryEntryKind::Mesh, "mesh.set_counts")),
                0x4c2 => Some((BinaryEntryKind::Mesh, "mesh.write.vertex")),
                0x4c3 => Some((BinaryEntryKind::Mesh, "mesh.write.primitive")),
                0x4c4 => Some((BinaryEntryKind::Mesh, "mesh.write.indices")),
                _ => None,
            };
            if let Some((required_stage, opcode)) = required_stage
                && required_stage != entry_kind
            {
                let required_name = match required_stage {
                    BinaryEntryKind::Object => "object",
                    BinaryEntryKind::Mesh => "mesh",
                    _ => continue,
                };
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function @{} CODE offset {}: {opcode} is only valid in the {required_name} stage",
                        signature.name, instruction.offset
                    ),
                ));
            }
        }
        pending.extend(
            graph
                .get(function_index)
                .into_iter()
                .flatten()
                .map(|edge| edge.target),
        );
    }
}

fn verify_mesh_indices(
    entry: &BinaryGpuEntry,
    bodies: &[Option<&BinaryFunctionBody>],
    signatures: &BinarySignatureTable,
    graph: &[Vec<CallEdge>],
    types: &BinaryTypeTable,
    imports: Option<&[BinaryImport]>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let topology = match entry.mesh_topology {
        1 => (1, "points"),
        2 => (2, "lines"),
        3 => (3, "triangles"),
        _ => return,
    };
    let Ok(start) = usize::try_from(entry.function_index) else {
        return;
    };
    let mut visited = vec![false; graph.len()];
    let mut pending = vec![start];
    while let Some(function_index) = pending.pop() {
        let Some(seen) = visited.get_mut(function_index) else {
            continue;
        };
        if *seen {
            continue;
        }
        *seen = true;
        let Some(body) = bodies.get(function_index).copied().flatten() else {
            continue;
        };
        let Some(signature) = signatures.records.get(function_index) else {
            continue;
        };
        let value_types = super::super::infer_value_types(body, signatures, types, imports);
        for instruction in body
            .blocks
            .iter()
            .filter_map(|block| block.instructions.as_ref())
            .flatten()
            .filter(|instruction| instruction.opcode == 0x4c4)
        {
            let value = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Values(values) => values.get(1).copied(),
                    _ => None,
                });
            let Some(index_type) = value
                .and_then(|value| usize::try_from(value).ok())
                .and_then(|value| value_types.get(value))
                .copied()
                .flatten()
            else {
                continue;
            };
            if !matches_topology_index_type(types, index_type, topology.0) {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function @{} CODE offset {}: mesh.write.indices index operand must match the {} mesh topology",
                        signature.name, instruction.offset, topology.1
                    ),
                ));
            }
        }
        pending.extend(graph[function_index].iter().map(|edge| edge.target));
    }
}

fn matches_topology_index_type(types: &BinaryTypeTable, index: u64, lanes: u8) -> bool {
    match usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
    {
        Some(BinaryType::Integer(32)) => lanes == 1,
        Some(BinaryType::FixedVector {
            lanes: actual,
            element,
        }) => {
            *actual == u64::from(lanes)
                && matches!(
                    usize::try_from(*element)
                        .ok()
                        .and_then(|index| types.entries.get(index)),
                    Some(BinaryType::Integer(32))
                )
        }
        _ => false,
    }
}

impl Analyzer<'_> {
    fn function(&mut self, function: usize, initial: u8) -> Summary {
        let state_index = match initial {
            ZERO => 0,
            ONCE => 1,
            MANY => 2,
            _ => return Summary::default(),
        };
        if let Some(summary) = self
            .summaries
            .get(function)
            .and_then(|states| states[state_index].as_ref())
        {
            return summary.clone();
        }
        if !self.active.insert((function, initial)) {
            return Summary {
                returns: initial,
                terminates: 0,
                issues: HashSet::new(),
            };
        }
        let Some(body) = self.bodies.get(function).copied().flatten().cloned() else {
            self.active.remove(&(function, initial));
            return Summary {
                returns: initial,
                terminates: 0,
                issues: HashSet::new(),
            };
        };
        if body.blocks.iter().any(|block| block.instructions.is_none()) {
            self.active.remove(&(function, initial));
            return Summary {
                returns: initial,
                terminates: 0,
                issues: HashSet::new(),
            };
        }
        let successors = successors(&body);
        let mut incoming = vec![0_u8; body.blocks.len()];
        if incoming.is_empty() {
            self.active.remove(&(function, initial));
            return Summary::default();
        }
        incoming[0] = initial;
        let mut pending = vec![0_usize];
        let mut summary = Summary::default();
        while let Some(block_index) = pending.pop() {
            let Some(block) = body.blocks.get(block_index) else {
                continue;
            };
            let Some(instructions) = block.instructions.as_ref() else {
                continue;
            };
            let mut state = incoming[block_index];
            let mut terminated_by_tail_call = false;
            for (instruction_index, instruction) in instructions.iter().enumerate() {
                match (self.rule, instruction.opcode) {
                    (Rule::Dispatch, 0x4c0) => {
                        if state & (ONCE | MANY) != 0 {
                            summary.issues.insert(Issue {
                                function,
                                block: block_index,
                                instruction: instruction_index,
                                kind: IssueKind::Repeated,
                            });
                        }
                        state = increment(state);
                    }
                    (Rule::Counts, 0x4c1) => {
                        if state & (ONCE | MANY) != 0 {
                            summary.issues.insert(Issue {
                                function,
                                block: block_index,
                                instruction: instruction_index,
                                kind: IssueKind::Repeated,
                            });
                        }
                        state = increment(state);
                    }
                    (Rule::Counts, 0x4c2..=0x4c4) if state & ZERO != 0 => {
                        summary.issues.insert(Issue {
                            function,
                            block: block_index,
                            instruction: instruction_index,
                            kind: IssueKind::WriteBeforeCounts,
                        });
                    }
                    _ => {}
                }
                if instruction.opcode == 0x1e7 {
                    let targets = self.targets(function, instruction.offset);
                    if targets.is_empty() {
                        summary.returns |= state;
                    } else {
                        let called = self.call_targets(&targets, state);
                        summary.returns |= called.returns;
                        summary.terminates |= called.terminates;
                        summary.issues.extend(called.issues);
                    }
                    terminated_by_tail_call = true;
                    break;
                }
                let targets = self.targets(function, instruction.offset);
                if !targets.is_empty() {
                    let called = self.call_targets(&targets, state);
                    state = called.returns;
                    summary.terminates |= called.terminates;
                    summary.issues.extend(called.issues);
                    if state == 0 {
                        break;
                    }
                }
            }
            if terminated_by_tail_call || state == 0 {
                continue;
            }
            if instructions
                .last()
                .is_some_and(|instruction| instruction.opcode == 0x1e5)
            {
                summary.returns |= state;
                continue;
            }
            if instructions
                .last()
                .is_some_and(|instruction| instruction.terminator)
                && successors.get(block_index).is_none_or(Vec::is_empty)
            {
                summary.terminates |= state;
                continue;
            }
            for target in successors.get(block_index).into_iter().flatten().copied() {
                let Some(next) = incoming.get_mut(target) else {
                    continue;
                };
                let merged = *next | state;
                if merged != *next {
                    *next = merged;
                    pending.push(target);
                }
            }
        }
        self.active.remove(&(function, initial));
        if let Some(slot) = self
            .summaries
            .get_mut(function)
            .and_then(|states| states.get_mut(state_index))
        {
            *slot = Some(summary.clone());
        }
        summary
    }

    fn targets(&self, function: usize, offset: u64) -> Vec<usize> {
        self.calls
            .get(function)
            .and_then(|calls| calls.get(&offset))
            .cloned()
            .unwrap_or_default()
    }

    fn call_targets(&mut self, targets: &[usize], input: u8) -> Summary {
        let mut result = Summary::default();
        for state in [ZERO, ONCE, MANY] {
            if input & state == 0 {
                continue;
            }
            for target in targets {
                let called = self.function(*target, state);
                result.returns |= called.returns;
                result.terminates |= called.terminates;
                result.issues.extend(called.issues);
            }
        }
        result
    }
}

fn successors(body: &BinaryFunctionBody) -> Vec<Vec<usize>> {
    body.blocks
        .iter()
        .map(|block| {
            let mut targets = Vec::new();
            if let Some(instruction) = block.instructions.as_ref().and_then(|items| items.last()) {
                if instruction.terminator {
                    super::super::structure::visit_edges(instruction, |target, _| {
                        if let Ok(target) = usize::try_from(target) {
                            targets.push(target);
                        }
                    });
                }
            }
            targets
        })
        .collect()
}

fn increment(state: u8) -> u8 {
    let mut next = 0;
    if state & ZERO != 0 {
        next |= ONCE;
    }
    if state & (ONCE | MANY) != 0 {
        next |= MANY;
    }
    next
}
