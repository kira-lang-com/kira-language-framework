use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, TextInstruction};

mod binary;
mod binary_cfg;
mod module;

pub use binary::verify_binary_gpu_module;
pub use module::verify_gpu_module;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Uniformity {
    Dispatch,
    Threadgroup,
    Subgroup,
    Quad,
    Divergent,
}

impl Uniformity {
    fn join(self, other: Self) -> Self {
        self.max(other)
    }

    pub fn satisfies(self, required: Self) -> bool {
        self <= required
    }
}

pub struct FunctionAnalysis {
    pub diagnostics: Vec<Diagnostic>,
    pub control: Vec<Uniformity>,
    pub required_on_entry: Option<Uniformity>,
}

struct Edge {
    source: usize,
    target: usize,
    arguments: Vec<String>,
}

pub fn analyze_gpu_function(source: &str, body: &FunctionBody, entry: bool) -> FunctionAnalysis {
    let Ok(tokens) = lex(source) else {
        return FunctionAnalysis {
            diagnostics: Vec::new(),
            control: vec![Uniformity::Dispatch; body.blocks.len()],
            required_on_entry: None,
        };
    };
    let entry_parameters = if entry {
        entry_parameter_uniformity(&tokens)
    } else {
        HashMap::new()
    };
    let mut labels = HashMap::with_capacity(body.blocks.len());
    for (index, block) in body.blocks.iter().enumerate() {
        labels.insert(block.label.as_str(), index);
    }
    let mut values = HashMap::new();
    for (block_index, block) in body.blocks.iter().enumerate() {
        for parameter in &block.parameters {
            let uniformity = if block_index == 0 {
                entry_parameters
                    .get(&parameter.name)
                    .copied()
                    .unwrap_or(Uniformity::Divergent)
            } else {
                Uniformity::Dispatch
            };
            values.insert(parameter.name.clone(), uniformity);
        }
        for instruction in &block.instructions {
            for result in &instruction.results {
                values.insert(result.clone(), Uniformity::Dispatch);
            }
        }
    }

    let edges = control_flow_edges(body, &labels);
    let successors = successors(body, &labels);
    let mut control = vec![Uniformity::Dispatch; body.blocks.len()];
    loop {
        let mut changed = false;
        for block in &body.blocks {
            for instruction in &block.instructions {
                let candidate = instruction_uniformity(instruction, &values);
                for result in &instruction.results {
                    changed |= raise(&mut values, result, candidate);
                }
            }
        }
        changed |= update_control(body, &labels, &successors, &values, &mut control);
        let previous_values = values.clone();
        changed |= update_block_parameters(body, &edges, &previous_values, &control, &mut values);
        if !changed {
            break;
        }
    }

    let (diagnostics, required_on_entry) = verify_required_uniformity(body, &control, &values);
    FunctionAnalysis {
        diagnostics,
        control,
        required_on_entry,
    }
}

fn entry_parameter_uniformity(tokens: &[Token]) -> HashMap<String, Uniformity> {
    let header_end = tokens
        .iter()
        .position(|token| punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let mut parameters = HashMap::new();
    for index in 0..header_end.saturating_sub(2) {
        let Some(role) = atom(&tokens[index]) else {
            continue;
        };
        if !matches!(
            role,
            "root" | "immediate" | "payload" | "attrs" | "data" | "record" | "input"
        ) || !punctuation(&tokens[index + 1], '%')
        {
            continue;
        }
        let Some(name) = atom(&tokens[index + 2]) else {
            continue;
        };
        parameters.insert(
            name.to_owned(),
            if matches!(role, "root" | "immediate") {
                Uniformity::Dispatch
            } else {
                Uniformity::Divergent
            },
        );
    }
    parameters
}

fn instruction_uniformity(
    instruction: &TextInstruction,
    values: &HashMap<String, Uniformity>,
) -> Uniformity {
    let operands = value_operands(instruction);
    let operand_uniformity = if operands.is_empty() {
        Uniformity::Divergent
    } else {
        operands.iter().fold(Uniformity::Dispatch, |current, name| {
            current.join(values.get(name).copied().unwrap_or(Uniformity::Divergent))
        })
    };
    match instruction.opcode.as_str() {
        "gpu.builtin" => builtin_uniformity(instruction),
        "subgroup.elect" | "subgroup.inverse_ballot" => Uniformity::Divergent,
        "stack.alloc" if has_space_suffix(&instruction.tokens, "threadgroup") => {
            Uniformity::Threadgroup
        }
        "stack.alloc" => Uniformity::Divergent,
        "global.addr" | "target.has" | "target.pagesize" | "gpu.spec_const" => Uniformity::Dispatch,
        "load" | "load.borrow" | "raw.load" => {
            if operands.is_empty() {
                Uniformity::Divergent
            } else {
                operand_uniformity
            }
        }
        "call" | "tail.call" | "gpu.call.vft" => Uniformity::Divergent,
        opcode if opcode.starts_with("atomic.") || opcode == "tex.atomic" => Uniformity::Divergent,
        "quad.broadcast" => operands
            .last()
            .and_then(|lane| values.get(lane))
            .filter(|uniformity| uniformity.satisfies(Uniformity::Quad))
            .map_or(Uniformity::Divergent, |_| Uniformity::Quad),
        opcode if subgroup_uniform_result(opcode, instruction, &operands, values) => {
            Uniformity::Subgroup
        }
        "subgroup.scan_inclusive" | "subgroup.scan_exclusive" => Uniformity::Divergent,
        "quad.all" | "quad.any" => Uniformity::Quad,
        opcode if opcode.starts_with("const.") => Uniformity::Dispatch,
        _ => operand_uniformity,
    }
}

fn builtin_uniformity(instruction: &TextInstruction) -> Uniformity {
    let start = operands_start(instruction);
    let name = instruction.tokens[start..]
        .iter()
        .find_map(atom)
        .unwrap_or_default();
    match name {
        "threadgroup_position_in_grid" => Uniformity::Threadgroup,
        "subgroup_index_in_threadgroup" => Uniformity::Subgroup,
        "threads_per_threadgroup"
        | "threadgroups_per_grid"
        | "subgroups_per_threadgroup"
        | "subgroup_size" => Uniformity::Dispatch,
        _ => Uniformity::Divergent,
    }
}

fn subgroup_uniform_result(
    opcode: &str,
    instruction: &TextInstruction,
    operands: &[String],
    values: &HashMap<String, Uniformity>,
) -> bool {
    if matches!(
        opcode,
        "subgroup.all"
            | "subgroup.any"
            | "subgroup.all_equal"
            | "subgroup.broadcast_first"
            | "subgroup.ballot"
            | "subgroup.ballot_bit_count"
            | "subgroup.ballot_find_lsb"
            | "subgroup.ballot_find_msb"
    ) {
        return true;
    }
    if opcode == "subgroup.reduce" {
        return !has_atom(&instruction.tokens, "cluster");
    }
    if opcode == "subgroup.broadcast" {
        return operands
            .last()
            .and_then(|lane| values.get(lane))
            .is_some_and(|uniformity| uniformity.satisfies(Uniformity::Subgroup));
    }
    false
}

fn control_flow_edges(body: &FunctionBody, labels: &HashMap<&str, usize>) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (source, block) in body.blocks.iter().enumerate() {
        let Some(instruction) = block.instructions.last() else {
            continue;
        };
        for (index, token) in instruction.tokens.iter().enumerate() {
            let Some(label) = atom(token) else {
                continue;
            };
            if index > 0
                && (atom(&instruction.tokens[index - 1]) == Some("merge")
                    || atom(&instruction.tokens[index - 1]) == Some("continue")
                    || matches!(
                        instruction.tokens[index - 1].kind,
                        TokenKind::Punctuation('%' | '@')
                    ))
            {
                continue;
            }
            let Some(target) = labels.get(label).copied() else {
                continue;
            };
            edges.push(Edge {
                source,
                target,
                arguments: target_arguments(&instruction.tokens, index),
            });
        }
    }
    edges
}

fn successors(body: &FunctionBody, labels: &HashMap<&str, usize>) -> Vec<Vec<usize>> {
    let mut successors = vec![Vec::new(); body.blocks.len()];
    for (source, block) in body.blocks.iter().enumerate() {
        let Some(instruction) = block.instructions.last() else {
            continue;
        };
        for (index, token) in instruction.tokens.iter().enumerate() {
            let Some(label) = atom(token) else {
                continue;
            };
            if index > 0
                && (atom(&instruction.tokens[index - 1]) == Some("merge")
                    || atom(&instruction.tokens[index - 1]) == Some("continue")
                    || matches!(
                        instruction.tokens[index - 1].kind,
                        TokenKind::Punctuation('%' | '@')
                    ))
            {
                continue;
            }
            if let Some(target) = labels.get(label).copied()
                && !successors[source].contains(&target)
            {
                successors[source].push(target);
            }
        }
    }
    successors
}

fn target_arguments(tokens: &[Token], label_index: usize) -> Vec<String> {
    if !tokens
        .get(label_index + 1)
        .is_some_and(|token| punctuation(token, '('))
    {
        return Vec::new();
    }
    let mut arguments = Vec::new();
    let mut cursor = label_index + 2;
    while cursor + 1 < tokens.len() && !punctuation(&tokens[cursor], ')') {
        if punctuation(&tokens[cursor], '%')
            && let Some(name) = atom(&tokens[cursor + 1])
        {
            arguments.push(name.to_owned());
            cursor += 2;
        } else {
            cursor += 1;
        }
    }
    arguments
}

fn update_control(
    body: &FunctionBody,
    labels: &HashMap<&str, usize>,
    successors: &[Vec<usize>],
    values: &HashMap<String, Uniformity>,
    control: &mut [Uniformity],
) -> bool {
    let mut changed = false;
    for (index, block) in body.blocks.iter().enumerate() {
        let Some(branch) = block.instructions.last() else {
            continue;
        };
        if !matches!(
            branch.opcode.as_str(),
            "cond.br" | "switch.int" | "switch.enum"
        ) {
            continue;
        }
        let merge =
            label_after(&branch.tokens, "merge").and_then(|label| labels.get(label).copied());
        if merge.is_none() && branch.opcode != "switch.enum" {
            continue;
        }
        let condition = value_operands(branch)
            .first()
            .and_then(|name| values.get(name))
            .copied()
            .unwrap_or(Uniformity::Divergent);
        let nested_control = control[index].join(condition);
        let mut pending = successors[index].clone();
        let mut visited = HashSet::new();
        while let Some(target) = pending.pop() {
            if Some(target) == merge || !visited.insert(target) {
                continue;
            }
            let raised = control[target].join(nested_control);
            if raised != control[target] {
                control[target] = raised;
                changed = true;
            }
            pending.extend(successors[target].iter().copied());
        }
    }
    changed
}

fn update_block_parameters(
    body: &FunctionBody,
    edges: &[Edge],
    values: &HashMap<String, Uniformity>,
    control: &[Uniformity],
    updated: &mut HashMap<String, Uniformity>,
) -> bool {
    let mut changed = false;
    for (block_index, block) in body.blocks.iter().enumerate().skip(1) {
        for (parameter_index, parameter) in block.parameters.iter().enumerate() {
            let incoming = edges
                .iter()
                .filter(|edge| edge.target == block_index)
                .filter_map(|edge| {
                    edge.arguments.get(parameter_index).map(|argument| {
                        (
                            argument,
                            values
                                .get(argument)
                                .copied()
                                .unwrap_or(Uniformity::Divergent),
                            control[edge.source],
                        )
                    })
                })
                .collect::<Vec<_>>();
            let candidate = if incoming.is_empty() {
                Uniformity::Divergent
            } else if incoming
                .iter()
                .all(|(argument, _, _)| *argument == incoming[0].0)
            {
                incoming
                    .iter()
                    .fold(Uniformity::Dispatch, |current, (_, value, _)| {
                        current.join(*value)
                    })
            } else {
                incoming
                    .iter()
                    .fold(Uniformity::Dispatch, |current, (_, value, path)| {
                        current.join(*value).join(*path)
                    })
            };
            changed |= raise(updated, &parameter.name, candidate);
        }
    }
    changed
}

fn verify_required_uniformity(
    body: &FunctionBody,
    control: &[Uniformity],
    values: &HashMap<String, Uniformity>,
) -> (Vec<Diagnostic>, Option<Uniformity>) {
    let mut diagnostics = Vec::new();
    let mut required_on_entry = None;
    for (block_index, block) in body.blocks.iter().enumerate() {
        for instruction in &block.instructions {
            if let Some((required, operation)) = dynamic_uniform_operand(instruction) {
                let selector = value_operands(instruction)
                    .last()
                    .and_then(|lane| values.get(lane))
                    .copied()
                    .unwrap_or(Uniformity::Divergent);
                if !selector.satisfies(required) {
                    diagnostics.push(
                        Diagnostic::error(
                            V15,
                            format!("{operation} lane selector must be dynamically uniform"),
                        )
                        .with_primary_span(instruction.span),
                    );
                }
            }
            let Some((required, description)) = required_uniformity(instruction) else {
                continue;
            };
            required_on_entry = Some(
                required_on_entry.map_or(required, |current: Uniformity| current.min(required)),
            );
            if has_atom(&instruction.tokens, "unchecked_uniformity") {
                continue;
            }
            if control[block_index].satisfies(required) {
                continue;
            }
            diagnostics.push(
                Diagnostic::error(V15, format!("{description} requires uniform control flow"))
                    .with_primary_span(instruction.span),
            );
        }
    }
    (diagnostics, required_on_entry)
}

fn dynamic_uniform_operand(instruction: &TextInstruction) -> Option<(Uniformity, &'static str)> {
    match instruction.opcode.as_str() {
        "quad.broadcast" => Some((Uniformity::Quad, "quad.broadcast")),
        "subgroup.broadcast" => Some((Uniformity::Subgroup, "subgroup.broadcast")),
        _ => None,
    }
}

fn required_uniformity(instruction: &TextInstruction) -> Option<(Uniformity, &'static str)> {
    match instruction.opcode.as_str() {
        "gpu.barrier" => match barrier_scope(&instruction.tokens) {
            Some("subgroup") => Some((Uniformity::Subgroup, "subgroup barrier")),
            Some("threadgroup") => Some((Uniformity::Threadgroup, "threadgroup barrier")),
            None => Some((Uniformity::Threadgroup, "threadgroup barrier")),
            Some(_) => None,
        },
        "gpu.ddx" | "gpu.ddy" | "gpu.fwidth" | "quad.broadcast" | "quad.swap" | "quad.all"
        | "quad.any" | "tex.sample" | "tex.sample_bias" | "tex.sample_compare" | "tex.gather"
        | "tex.gather_compare" | "tex.lod" | "tex.feedback" => Some((
            Uniformity::Quad,
            "derivative, implicit sampling, or quad operation",
        )),
        "object.dispatch_mesh" | "mesh.set_counts" => {
            Some((Uniformity::Threadgroup, "mesh dispatch operation"))
        }
        _ => None,
    }
}

fn barrier_scope(tokens: &[Token]) -> Option<&str> {
    let start = tokens
        .iter()
        .position(|token| atom(token) == Some("barrier"))?
        + 1;
    tokens[start..].windows(3).find_map(|window| {
        (punctuation(&window[0], '[') && punctuation(&window[2], ']'))
            .then(|| atom(&window[1]))
            .flatten()
    })
}

fn label_after<'a>(tokens: &'a [Token], keyword: &str) -> Option<&'a str> {
    tokens.windows(2).find_map(|window| {
        (atom(&window[0]) == Some(keyword))
            .then(|| atom(&window[1]))
            .flatten()
    })
}

fn value_operands(instruction: &TextInstruction) -> Vec<String> {
    let start = operands_start(instruction);
    instruction.tokens[start..]
        .windows(2)
        .filter_map(|window| {
            (punctuation(&window[0], '%'))
                .then(|| atom(&window[1]))
                .flatten()
                .map(str::to_owned)
        })
        .collect()
}

fn operands_start(instruction: &TextInstruction) -> usize {
    let tokens = &instruction.tokens;
    let mut cursor = 0;
    if tokens.first().is_some_and(|token| punctuation(token, '%')) {
        loop {
            if !tokens
                .get(cursor)
                .is_some_and(|token| punctuation(token, '%'))
                || !tokens
                    .get(cursor + 1)
                    .is_some_and(|token| atom(token).is_some())
            {
                return 0;
            }
            cursor += 2;
            if tokens
                .get(cursor)
                .is_some_and(|token| punctuation(token, ','))
            {
                cursor += 1;
            } else if tokens
                .get(cursor)
                .is_some_and(|token| punctuation(token, '='))
            {
                cursor += 1;
                break;
            } else {
                return 0;
            }
        }
    }
    if tokens
        .get(cursor)
        .is_some_and(|token| atom(token).is_some())
    {
        cursor += 1;
    }
    while tokens
        .get(cursor)
        .is_some_and(|token| punctuation(token, '.'))
        && tokens
            .get(cursor + 1)
            .is_some_and(|token| atom(token).is_some())
    {
        cursor += 2;
    }
    cursor
}

fn raise(values: &mut HashMap<String, Uniformity>, name: &str, candidate: Uniformity) -> bool {
    let Some(current) = values.get_mut(name) else {
        return false;
    };
    let raised = current.join(candidate);
    if raised == *current {
        return false;
    }
    *current = raised;
    true
}

fn has_space_suffix(tokens: &[Token], expected: &str) -> bool {
    tokens.windows(3).rev().find_map(|window| {
        (punctuation(&window[0], '<')
            && atom(&window[1]) == Some(expected)
            && punctuation(&window[2], '>'))
        .then_some(expected)
    }) == Some(expected)
}

fn has_atom(tokens: &[Token], expected: &str) -> bool {
    tokens.iter().any(|token| atom(token) == Some(expected))
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}
