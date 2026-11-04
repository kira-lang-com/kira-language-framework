use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    TextModule,
    text::{Token, TokenKind, lex},
};
use tir_layout::LayoutEngine;
use tir_types::Type;

use super::graph::GpuFunction;
use super::policy::TypePolicies;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
pub(super) type CallTargets = HashMap<(usize, usize), Vec<usize>>;

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
    functions: &'a [GpuFunction],
    calls: &'a [CallTargets],
    rule: Rule,
    summaries: Vec<[Option<Summary>; 3]>,
    active: HashSet<(usize, u8)>,
}

pub(super) fn verify_entries(
    functions: &[GpuFunction],
    module: &TextModule,
    calls: &[CallTargets],
    policies: &TypePolicies,
    layouts: &LayoutEngine,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (entry_index, entry) in functions.iter().enumerate() {
        if !entry.entry {
            continue;
        }
        let source = module.item_text(entry.item);
        let Some(stage) = lex(source).ok().and_then(|tokens| entry_stage(&tokens)) else {
            continue;
        };
        verify_geometry_stage(entry_index, &stage, functions, calls, &mut diagnostics);
        let rule = match stage.as_str() {
            "object" => Rule::Dispatch,
            "mesh" => Rule::Counts,
            _ => continue,
        };
        let mut analyzer = Analyzer {
            functions,
            calls,
            rule,
            summaries: (0..functions.len()).map(|_| [None, None, None]).collect(),
            active: HashSet::new(),
        };
        let summary = analyzer.function(entry_index, ZERO);
        for issue in summary.issues {
            let Some(function) = functions.get(issue.function) else {
                continue;
            };
            let Some(instruction) = function
                .body
                .blocks
                .get(issue.block)
                .and_then(|block| block.instructions.get(issue.instruction))
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
            let mut diagnostic = Diagnostic::error(V2, message).with_primary_span(instruction.span);
            super::shift(&mut diagnostic, function.item.span.start);
            diagnostics.push(diagnostic);
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
            let mut diagnostic = Diagnostic::error(V2, message).with_primary_span(entry.body.span);
            super::shift(&mut diagnostic, entry.item.span.start);
            diagnostics.push(diagnostic);
        }
        if rule == Rule::Counts {
            verify_mesh_indices(
                entry_index,
                functions,
                module,
                calls,
                policies,
                layouts,
                &mut diagnostics,
            );
        }
    }
    diagnostics
}

fn verify_geometry_stage(
    entry_index: usize,
    stage: &str,
    functions: &[GpuFunction],
    calls: &[CallTargets],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut visited = vec![false; functions.len()];
    let mut pending = vec![entry_index];
    while let Some(function_index) = pending.pop() {
        let Some(seen) = visited.get_mut(function_index) else {
            continue;
        };
        if *seen {
            continue;
        }
        *seen = true;
        let function = &functions[function_index];
        for instruction in function
            .body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
        {
            let required_stage = match instruction.opcode.as_str() {
                "object.dispatch_mesh" => Some("object"),
                "mesh.set_counts"
                | "mesh.write.vertex"
                | "mesh.write.primitive"
                | "mesh.write.indices" => Some("mesh"),
                _ => None,
            };
            if let Some(required_stage) = required_stage
                && required_stage != stage
            {
                let mut diagnostic = Diagnostic::error(
                    V2,
                    format!(
                        "{} is only valid in the {required_stage} stage",
                        instruction.opcode
                    ),
                )
                .with_primary_span(instruction.span);
                super::shift(&mut diagnostic, function.item.span.start);
                diagnostics.push(diagnostic);
            }
        }
        pending.extend(
            calls
                .get(function_index)
                .into_iter()
                .flat_map(|targets| targets.values().flatten().copied()),
        );
    }
}

fn verify_mesh_indices(
    entry_index: usize,
    functions: &[GpuFunction],
    module: &TextModule,
    calls: &[CallTargets],
    policies: &TypePolicies,
    layouts: &LayoutEngine,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(entry) = functions.get(entry_index) else {
        return;
    };
    let Some((lanes, topology)) = mesh_topology(module.item_text(entry.item)) else {
        return;
    };
    let mut visited = vec![false; functions.len()];
    let mut pending = vec![entry_index];
    while let Some(function_index) = pending.pop() {
        let Some(seen) = visited.get_mut(function_index) else {
            continue;
        };
        if *seen {
            continue;
        }
        *seen = true;
        let function = &functions[function_index];
        let source = module.item_text(function.item);
        let value_types = crate::text_types::inferred_value_types(
            source,
            &function.body,
            layouts,
            policies,
            None,
        );
        for instruction in function
            .body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| instruction.opcode == "mesh.write.indices")
        {
            let value_uses = crate::ssa::text_value_uses(instruction);
            let Some((index, _)) = value_uses.get(1) else {
                continue;
            };
            let Some(ty) = value_types.get(index) else {
                continue;
            };
            if !matches_topology_index_type(ty, lanes) {
                let mut diagnostic = Diagnostic::error(
                    V2,
                    format!(
                        "mesh.write.indices index operand must match the {topology} mesh topology"
                    ),
                )
                .with_primary_span(instruction.span);
                super::shift(&mut diagnostic, function.item.span.start);
                diagnostics.push(diagnostic);
            }
        }
        pending.extend(
            calls
                .get(function_index)
                .into_iter()
                .flat_map(|targets| targets.values().flatten().copied()),
        );
    }
}

fn mesh_topology(source: &str) -> Option<(u8, &'static str)> {
    let tokens = lex(source).ok()?;
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    tokens[..header_end].windows(3).find_map(|field| {
        (atom(&field[0]) == Some("topology") && punctuation(&field[1], ':'))
            .then(|| match atom(&field[2])? {
                "points" => Some((1, "points")),
                "lines" => Some((2, "lines")),
                "triangles" => Some((3, "triangles")),
                _ => None,
            })
            .flatten()
    })
}

fn matches_topology_index_type(ty: &Type, lanes: u8) -> bool {
    match ty {
        Type::Integer(32) => lanes == 1,
        Type::FixedVector {
            lanes: actual,
            element,
        } => *actual == u32::from(lanes) && element.as_ref() == &Type::Integer(32),
        _ => false,
    }
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(actual) if actual == expected)
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
        let Some(body) = self.functions.get(function).map(|item| item.body.clone()) else {
            self.active.remove(&(function, initial));
            return Summary {
                returns: initial,
                terminates: 0,
                issues: HashSet::new(),
            };
        };
        let successors = crate::text_cfg::text_successors(&body);
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
            let mut state = incoming[block_index];
            let mut terminated_by_tail_call = false;
            for (instruction_index, instruction) in block.instructions.iter().enumerate() {
                if let Some(event) = event(instruction.opcode.as_str()) {
                    match (self.rule, event) {
                        (Rule::Dispatch, Event::Dispatch) => {
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
                        (Rule::Counts, Event::SetCounts) => {
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
                        (Rule::Counts, Event::MeshWrite) if state & ZERO != 0 => {
                            summary.issues.insert(Issue {
                                function,
                                block: block_index,
                                instruction: instruction_index,
                                kind: IssueKind::WriteBeforeCounts,
                            });
                        }
                        _ => {}
                    }
                }
                let is_tail_call = instruction.opcode == "tail.call";
                if is_call(instruction.opcode.as_str()) {
                    let targets = self
                        .calls
                        .get(function)
                        .and_then(|map| map.get(&(block_index, instruction_index)))
                        .cloned()
                        .unwrap_or_default();
                    if is_tail_call {
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
            }
            if terminated_by_tail_call {
                continue;
            }
            let last = block.instructions.last();
            if last.is_some_and(|instruction| instruction.opcode == "return") {
                summary.returns |= state;
                continue;
            }
            if last.is_some_and(|instruction| crate::is_terminator(&instruction.opcode))
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

#[derive(Clone, Copy)]
enum Event {
    Dispatch,
    SetCounts,
    MeshWrite,
}

fn event(opcode: &str) -> Option<Event> {
    match opcode {
        "object.dispatch_mesh" => Some(Event::Dispatch),
        "mesh.set_counts" => Some(Event::SetCounts),
        "mesh.write.vertex" | "mesh.write.primitive" | "mesh.write.indices" => {
            Some(Event::MeshWrite)
        }
        _ => None,
    }
}

fn is_call(opcode: &str) -> bool {
    matches!(
        opcode,
        "call" | "tail.call" | "call.witness" | "gpu.call.vft"
    )
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

fn entry_stage(tokens: &[Token]) -> Option<String> {
    tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .nth(1)
        .and_then(|token| match &token.kind {
            TokenKind::Atom(atom) => Some(atom.clone()),
            _ => None,
        })
}

const ZERO: u8 = 1;
const ONCE: u8 = 2;
const MANY: u8 = 4;
