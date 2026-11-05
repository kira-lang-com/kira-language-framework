use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, TextItem, TextItemKind, TextModule};
use tir_layout::LayoutEngine;
use tir_types::{FunctionType, Type};

use super::policy::TypePolicies;
use crate::variant_text::read_symbol;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);
const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) struct GpuFunction {
    pub(super) item: TextItem,
    pub(super) symbol: String,
    pub(super) symbol_span: Span,
    pub(super) body: FunctionBody,
    pub(super) entry: bool,
    pub(super) exported: bool,
    pub(super) function_type: Option<FunctionType>,
    pub(super) float64_capability: bool,
    pub(super) uses_float64: bool,
}

pub(super) fn global_properties(module: &TextModule) -> HashMap<String, (bool, bool)> {
    let mut globals = HashMap::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Global)
    {
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        let Some((symbol, _, _)) = tokens.iter().enumerate().find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('@'))
                .then(|| read_symbol(&tokens, index))
                .flatten()
        }) else {
            continue;
        };
        let constant_space = tokens.windows(3).any(|window| {
            matches!(window[0].kind, TokenKind::Punctuation('<'))
                && matches!(&window[1].kind, TokenKind::Atom(atom) if atom == "constant")
                && matches!(window[2].kind, TokenKind::Punctuation('>'))
        });
        let initialized = tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Punctuation('=')));
        globals.insert(symbol, (constant_space, initialized));
    }
    globals
}

pub(super) fn verify_call_graph(
    functions: &[GpuFunction],
    module: &TextModule,
    policies: &TypePolicies,
    layouts: &mut LayoutEngine,
) -> Vec<Diagnostic> {
    let local_callability = local_function_callability(module);
    let witness_conformances = super::witness::ConformanceIndex::from_module(module);
    let symbols = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.symbol.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut successors = vec![Vec::new(); functions.len()];
    let mut diagnostics = Vec::new();
    for (index, function) in functions.iter().enumerate() {
        let value_types = crate::text_types::inferred_value_types(
            module.item_text(function.item),
            &function.body,
            layouts,
            policies,
        );
        for instruction in function
            .body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
        {
            if instruction.opcode == "gpu.call.vft" {
                let vft_type = crate::ssa::text_value_uses(instruction)
                    .first()
                    .and_then(|(name, _)| value_types.get(name))
                    .and_then(|ty| match ty {
                        Type::VisibleFunctionTable(function) => Some(function.as_ref()),
                        _ => None,
                    });
                for (target_index, target) in functions.iter().enumerate() {
                    if !target.entry
                        && target.exported
                        && compatible_vft_type(
                            vft_type,
                            target.function_type.as_ref(),
                            &module.header.path,
                        )
                    {
                        successors[index].push(target_index);
                    }
                }
                continue;
            }
            let symbol = match instruction.opcode.as_str() {
                "call" | "tail.call" => referenced_symbol(&instruction.tokens),
                "call.witness" => match super::witness::resolve_target(
                    instruction,
                    module.item_text(function.item),
                    &module.header.path,
                    &witness_conformances,
                ) {
                    Ok(symbol) => Some(symbol),
                    Err(message) => {
                        let mut diagnostic =
                            Diagnostic::error(V15, message).with_primary_span(instruction.span);
                        shift(&mut diagnostic, function.item.span.start);
                        diagnostics.push(diagnostic);
                        None
                    }
                },
                _ => None,
            };
            let Some(symbol) = symbol else { continue };
            if local_callability.get(&symbol) == Some(&false) {
                let mut diagnostic = Diagnostic::error(
                    V15,
                    format!("GPU code cannot call non-GPU function @{symbol}"),
                )
                .with_primary_span(instruction.span);
                shift(&mut diagnostic, function.item.span.start);
                diagnostics.push(diagnostic);
            }
            if let Some(target) = symbols.get(symbol.as_str()).copied() {
                successors[index].push(target);
            }
        }
    }

    for (entry_index, entry) in functions.iter().enumerate().filter(|(_, f)| f.entry) {
        if entry.float64_capability {
            continue;
        }
        let mut visited = vec![false; functions.len()];
        let mut pending = vec![entry_index];
        while let Some(index) = pending.pop() {
            if std::mem::replace(&mut visited[index], true) {
                continue;
            }
            if functions[index].uses_float64 {
                diagnostics.push(
                    Diagnostic::error(V15, "GPU f64 use requires the float64 capability")
                        .with_primary_span(entry.item.span),
                );
                break;
            }
            pending.extend(successors[index].iter().copied());
        }
    }
    if let Some(index) = find_cycle_node(&successors) {
        let function = &functions[index];
        let mut diagnostic = Diagnostic::error(
            V15,
            format!(
                "GPU call graph contains a cycle involving @{}",
                function.symbol
            ),
        )
        .with_primary_span(function.symbol_span);
        shift(&mut diagnostic, function.item.span.start);
        diagnostics.push(diagnostic);
    }
    diagnostics.extend(verify_vft_stages(functions, &successors, module));
    diagnostics.extend(verify_builtin_stages(functions, &successors, module));
    diagnostics.extend(super::capabilities::verify(
        functions,
        &successors,
        module,
        policies,
        layouts,
    ));
    diagnostics
}

fn compatible_vft_type(
    expected: Option<&FunctionType>,
    actual: Option<&FunctionType>,
    module_path: &str,
) -> bool {
    let (Some(expected), Some(actual)) = (expected, actual) else {
        return true;
    };
    let canonical = |function: &FunctionType| {
        Type::Function(function.clone()).canonical_text_in(module_path, &HashMap::new())
    };
    match (canonical(expected), canonical(actual)) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => true,
    }
}

fn verify_vft_stages(
    functions: &[GpuFunction],
    successors: &[Vec<usize>],
    module: &TextModule,
) -> Vec<Diagnostic> {
    let allowed = [
        "compute",
        "raygen",
        "intersection",
        "anyhit",
        "closesthit",
        "miss",
        "callable",
    ];
    let mut reported = HashSet::new();
    let mut diagnostics = Vec::new();
    for (entry_index, entry) in functions.iter().enumerate().filter(|(_, f)| f.entry) {
        let source = module.item_text(entry.item);
        let stage = lex(source)
            .ok()
            .and_then(|tokens| {
                tokens
                    .iter()
                    .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
                    .nth(1)
                    .and_then(atom)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        if allowed.contains(&stage.as_str()) {
            continue;
        }
        let mut visited = vec![false; functions.len()];
        let mut pending = vec![entry_index];
        while let Some(index) = pending.pop() {
            if std::mem::replace(&mut visited[index], true) {
                continue;
            }
            for instruction in functions[index]
                .body
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter(|instruction| instruction.opcode == "gpu.call.vft")
            {
                if !reported.insert((index, instruction.span.start)) {
                    continue;
                }
                let mut diagnostic = Diagnostic::error(
                    V15,
                    format!("gpu.call.vft is not permitted in the {stage} stage"),
                )
                .with_primary_span(instruction.span);
                shift(&mut diagnostic, functions[index].item.span.start);
                diagnostics.push(diagnostic);
            }
            pending.extend(successors[index].iter().copied());
        }
    }
    diagnostics
}

fn verify_builtin_stages(
    functions: &[GpuFunction],
    successors: &[Vec<usize>],
    module: &TextModule,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (entry_index, entry) in functions.iter().enumerate().filter(|(_, f)| f.entry) {
        let source = module.item_text(entry.item);
        let stage = lex(source)
            .ok()
            .and_then(|tokens| {
                tokens
                    .iter()
                    .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
                    .nth(1)
                    .and_then(atom)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let mut visited = vec![false; functions.len()];
        let mut pending = vec![entry_index];
        while let Some(index) = pending.pop() {
            if std::mem::replace(&mut visited[index], true) {
                continue;
            }
            for instruction in functions[index]
                .body
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter(|instruction| instruction.opcode == "gpu.builtin")
            {
                let name = instruction
                    .tokens
                    .get(crate::ssa::text_instruction_operands_start(
                        &instruction.tokens,
                    ))
                    .and_then(atom);
                let Some(name) = name else {
                    continue;
                };
                let mut diagnostic = match builtin_stages(name) {
                    Some(stages) if stages.contains(&stage.as_str()) => continue,
                    Some(_) => Diagnostic::error(
                        V2,
                        format!("GPU builtin {name} is not available in the {stage} stage"),
                    ),
                    None => Diagnostic::error(V2, format!("unknown GPU builtin {name}")),
                }
                .with_primary_span(instruction.span);
                shift(&mut diagnostic, functions[index].item.span.start);
                diagnostics.push(diagnostic);
            }
            pending.extend(successors[index].iter().copied());
        }
    }
    diagnostics
}

fn builtin_stages(name: &str) -> Option<&'static [&'static str]> {
    Some(match name {
        "thread_position_in_grid"
        | "thread_position_in_threadgroup"
        | "threadgroup_position_in_grid"
        | "threads_per_threadgroup"
        | "thread_index_in_threadgroup"
        | "subgroup_index_in_threadgroup"
        | "subgroups_per_threadgroup" => &["compute", "object", "mesh", "tile"],
        "threadgroups_per_grid" => &["compute", "object", "mesh"],
        "lane_index" | "subgroup_size" => &[
            "compute",
            "object",
            "mesh",
            "tile",
            "vertex",
            "fragment",
            "raygen",
            "intersection",
            "anyhit",
            "closesthit",
            "miss",
            "callable",
            "node",
        ],
        "vertex_id" | "instance_id" | "base_vertex" | "base_instance" | "draw_index" => &["vertex"],
        "view_index" => &["vertex", "mesh", "fragment"],
        "position"
        | "front_facing"
        | "sample_index"
        | "sample_mask_in"
        | "barycentric_perspective"
        | "barycentric_linear"
        | "primitive_id"
        | "layer"
        | "viewport_index"
        | "point_coord"
        | "helper_invocation"
        | "shading_rate" => &["fragment"],
        _ => return None,
    })
}

fn local_function_callability(module: &TextModule) -> HashMap<String, bool> {
    let mut functions = HashMap::new();
    for item in &module.items {
        if !matches!(item.kind, TextItemKind::Function | TextItemKind::Entry) {
            continue;
        }
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        let Some((symbol, _, _)) = tokens.iter().enumerate().find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('@'))
                .then(|| read_symbol(&tokens, index))
                .flatten()
        }) else {
            continue;
        };
        functions.insert(
            symbol,
            item.kind == TextItemKind::Function && has_gpu_target(&tokens),
        );
    }
    functions
}

fn has_gpu_target(tokens: &[Token]) -> bool {
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    let mut in_target = false;
    for token in &tokens[..header_end] {
        match &token.kind {
            TokenKind::Atom(name) if name == "target" => in_target = true,
            TokenKind::Punctuation(')') => in_target = false,
            TokenKind::Atom(name) if in_target && name == "gpu" => return true,
            _ => {}
        }
    }
    false
}

pub(super) fn referenced_symbol(tokens: &[Token]) -> Option<String> {
    tokens.iter().enumerate().find_map(|(index, token)| {
        matches!(token.kind, TokenKind::Punctuation('@'))
            .then(|| read_symbol(tokens, index).map(|(symbol, _, _)| symbol))
            .flatten()
    })
}

pub(super) fn has_capability(header: &[Token], capability: &str) -> bool {
    let mut in_caps = false;
    for token in header {
        match &token.kind {
            TokenKind::Atom(name) if name == "caps" => in_caps = true,
            TokenKind::Punctuation(')') => in_caps = false,
            TokenKind::Atom(name) if in_caps && name == capability => return true,
            _ => {}
        }
    }
    false
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn find_cycle_node(successors: &[Vec<usize>]) -> Option<usize> {
    let mut state = vec![0u8; successors.len()];
    for root in 0..successors.len() {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some((node, edge)) = stack.last().copied() {
            if edge == successors[node].len() {
                state[node] = 2;
                stack.pop();
                continue;
            }
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
            let target = successors[node][edge];
            match state[target] {
                1 => return Some(target),
                0 => {
                    state[target] = 1;
                    stack.push((target, 0));
                }
                _ => {}
            }
        }
    }
    None
}

fn shift(diagnostic: &mut Diagnostic, offset: usize) {
    if let Some(span) = &mut diagnostic.primary_span {
        span.start = span.start.saturating_add(offset);
        span.end = span.end.saturating_add(offset);
    }
    for label in &mut diagnostic.labels {
        label.span.start = label.span.start.saturating_add(offset);
        label.span.end = label.span.end.saturating_add(offset);
    }
}
