use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{
    FunctionBody, TextFunctionSignature, TextInstruction, TextModule, parse_function_signature,
};
use tir_layout::LayoutEngine;
use tir_types::{AddressSpace, FloatType, Type};

use super::graph::GpuFunction;
use super::policy::TypePolicies;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

const OPTIONAL_CAPABILITIES: &[&str] = &[
    "float64",
    "atomic128",
    "float16.atomics",
    "tile",
    "framebuffer_fetch",
    "raster_order",
    "work_graphs",
    "work_graphs.mesh",
    "coop.fp8",
    "coop.blockscaled",
    "coop.reduce",
    "compute_derivatives",
    "float32.denorm_preserve",
    "sparse_residency",
    "sampler_feedback",
    "subgroup_size_control",
    "triangle_vertex_positions",
    "shading_rate",
    "printf",
];

pub(super) fn verify(
    functions: &[GpuFunction],
    successors: &[Vec<usize>],
    module: &TextModule,
    policies: &TypePolicies,
    layouts: &mut LayoutEngine,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (entry_index, entry) in functions.iter().enumerate().filter(|(_, f)| f.entry) {
        let entry_source = module.item_text(entry.item);
        let Ok(entry_tokens) = lex(entry_source) else {
            continue;
        };
        let header = &entry_tokens[..header_end(&entry_tokens)];
        let declared = declared_capabilities(header);
        for capability in &declared {
            if !OPTIONAL_CAPABILITIES.contains(&capability.as_str()) {
                diagnostics.push(
                    Diagnostic::error(V15, format!("unknown GPU capability '{capability}'"))
                        .with_primary_span(entry.item.span),
                );
            }
        }

        let entry_stage = entry_stage(header).unwrap_or_default();
        let mut required = HashSet::new();
        let mut visited = vec![false; functions.len()];
        let mut pending = vec![entry_index];
        while let Some(index) = pending.pop() {
            if std::mem::replace(&mut visited[index], true) {
                continue;
            }
            let function = &functions[index];
            let source = module.item_text(function.item);
            if let Ok(tokens) = lex(source) {
                let signature = parse_function_signature(source).ok().flatten();
                required.extend(function_capabilities(
                    source,
                    &tokens[..header_end(&tokens)],
                    &function.body,
                    signature.as_ref(),
                    entry_stage,
                    policies,
                    layouts,
                ));
            }
            pending.extend(successors[index].iter().copied());
        }
        for capability in required {
            if !declared.contains(capability) {
                diagnostics.push(
                    Diagnostic::error(
                        V15,
                        format!("GPU entry uses undeclared capability '{capability}'"),
                    )
                    .with_primary_span(entry.item.span),
                );
            }
        }
    }
    diagnostics
}

fn function_capabilities(
    source: &str,
    header: &[Token],
    body: &FunctionBody,
    signature: Option<&TextFunctionSignature>,
    entry_stage: &str,
    policies: &TypePolicies,
    layouts: &mut LayoutEngine,
) -> HashSet<&'static str> {
    let mut required = HashSet::new();
    if has_call(header, "fp", "ieee_denormals") {
        required.insert("float32.denorm_preserve");
    }
    if has_call(header, "subgroup_size", "") {
        required.insert("subgroup_size_control");
    }
    if entry_stage == "tile" {
        required.insert("tile");
    }
    if entry_stage == "node" {
        required.insert("work_graphs");
        if has_atom(header, "mesh") {
            required.insert("work_graphs.mesh");
        }
    }
    if let Some(signature) = signature {
        for parameter in &signature.parameters {
            add_type_capabilities(&parameter.ty, policies, &mut required);
        }
        add_type_capabilities(&signature.result, policies, &mut required);
        if let Some(failure) = &signature.failure {
            add_type_capabilities(failure, policies, &mut required);
        }
    }
    for parameter in body.blocks.iter().flat_map(|block| &block.parameters) {
        add_type_capabilities(&parameter.ty, policies, &mut required);
    }
    for ty in body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .flat_map(|instruction| super::explicit_types(source, instruction))
    {
        add_type_capabilities(&ty, policies, &mut required);
    }
    let value_types = crate::text_types::inferred_value_types(source, body, layouts, policies);
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        let opcode = instruction.opcode.as_str();
        let tokens = &instruction.tokens;
        if matches!(opcode, "frag.imageblock.addr" | "tile.imageblock.addr") {
            required.insert("tile");
        }
        if matches!(opcode, "gpu.barrier" | "gpu.memfence") && has_atom(tokens, "tile") {
            required.insert("tile");
        }
        if opcode == "frag.load_attachment" {
            required.insert("framebuffer_fetch");
        }
        if matches!(opcode, "frag.raster_order.begin" | "frag.raster_order.end") {
            required.insert("raster_order");
        }
        if opcode == "tex.feedback" {
            required.insert("sampler_feedback");
        }
        if opcode == "gpu.printf" {
            required.insert("printf");
        }
        if opcode == "coop.reduce" {
            required.insert("coop.reduce");
        }
        if entry_stage == "compute"
            && matches!(
                opcode,
                "tex.sample"
                    | "tex.sample_bias"
                    | "tex.sample_compare"
                    | "tex.gather"
                    | "tex.gather_compare"
                    | "tex.lod"
                    | "tex.feedback"
            )
        {
            required.insert("compute_derivatives");
        }
        if opcode.starts_with("node.") {
            required.insert("work_graphs");
            if has_atom(tokens, "mesh") {
                required.insert("work_graphs.mesh");
            }
        }
        if opcode.starts_with("tex.") && has_atom(tokens, "sparse") {
            required.insert("sparse_residency");
        }
        if matches!(opcode, "rt.get" | "rt.hitobject.get")
            && has_atom(tokens, "triangle_vertex_positions")
        {
            required.insert("triangle_vertex_positions");
        }
        if opcode == "gpu.builtin" && has_atom(tokens, "shading_rate") {
            required.insert("shading_rate");
        }
        if opcode.starts_with("atomic.") {
            if let Some(ty) = atomic_type(source, instruction, &value_types) {
                if matches!(ty, Type::Float(FloatType::F16)) {
                    required.insert("float16.atomics");
                }
                if matches!(layouts.layout(&ty), Ok(layout) if layout.size == 16) {
                    required.insert("atomic128");
                }
            }
        }
        if opcode.starts_with("coop.") {
            if has_atom(tokens, "e8m0") {
                required.insert("coop.blockscaled");
            }
        }
    }
    required
}

fn atomic_type(
    source: &str,
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Option<Type> {
    if instruction.opcode == "atomic.load" {
        return crate::text_types::explicit_type(source, instruction);
    }
    let values = crate::ssa::text_value_uses(instruction);
    let index = if instruction.opcode == "atomic.store" {
        0
    } else {
        values.len().checked_sub(1)?
    };
    let (name, _) = values.get(index)?;
    value_types.get(name).cloned()
}

fn add_type_capabilities(ty: &Type, policies: &TypePolicies, required: &mut HashSet<&'static str>) {
    if policies.any_nested_type(ty, |nested| matches!(nested, Type::Float(FloatType::F64))) {
        required.insert("float64");
    }
    if policies.any_nested_type(ty, |nested| {
        matches!(
            nested,
            Type::Pointer(AddressSpace::Tile)
                | Type::Address {
                    space: Some(AddressSpace::Tile),
                    ..
                }
        )
    }) {
        required.insert("tile");
    }
    if policies.any_nested_type(ty, |nested| {
        matches!(
            nested,
            Type::Pointer(AddressSpace::NodeRecord)
                | Type::Address {
                    space: Some(AddressSpace::NodeRecord),
                    ..
                }
        )
    }) {
        required.insert("work_graphs");
    }
    if policies.any_nested_type(ty, |nested| {
        matches!(
            nested,
            Type::CooperativeMatrix {
                element,
                ..
            } if matches!(element.as_ref(), Type::Float(FloatType::F8E4M3 | FloatType::F8E5M2))
        )
    }) {
        required.insert("coop.fp8");
    }
    if policies.any_nested_type(ty, |nested| {
        matches!(
            nested,
            Type::CooperativeMatrix {
                element,
                ..
            } if matches!(
                element.as_ref(),
                Type::Float(FloatType::F4E2M1 | FloatType::F6E2M3 | FloatType::F6E3M2)
            )
        )
    }) {
        required.insert("coop.blockscaled");
    }
}

fn declared_capabilities(header: &[Token]) -> HashSet<String> {
    let header = significant_tokens(header);
    let mut capabilities = HashSet::new();
    for (index, token) in header.iter().enumerate() {
        if atom(token) != Some("caps") || !punctuation_at(&header, index + 1, '(') {
            continue;
        }
        let mut cursor = index + 2;
        while cursor < header.len() && !punctuation_at(&header, cursor, ')') {
            if let Some(name) = atom_at(&header, cursor) {
                let mut name = name.to_owned();
                cursor += 1;
                while punctuation_at(&header, cursor, '.') {
                    if let Some(component) = atom_at(&header, cursor + 1) {
                        name.push('.');
                        name.push_str(component);
                        cursor += 2;
                    } else {
                        break;
                    }
                }
                capabilities.insert(name);
                continue;
            }
            cursor += 1;
        }
    }
    capabilities
}

fn has_call(tokens: &[Token], name: &str, argument: &str) -> bool {
    significant_tokens(tokens).windows(4).any(|window| {
        atom(&window[0]) == Some(name)
            && punctuation(&window[1], '(')
            && (argument.is_empty() || atom(&window[2]) == Some(argument))
            && punctuation(&window[3], ')')
    })
}

fn significant_tokens(tokens: &[Token]) -> Vec<Token> {
    tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .cloned()
        .collect()
}

fn header_end(tokens: &[Token]) -> usize {
    tokens
        .iter()
        .position(|token| punctuation(token, '{'))
        .unwrap_or(tokens.len())
}

fn entry_stage(header: &[Token]) -> Option<&str> {
    header
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .nth(1)
        .and_then(atom)
}

fn has_atom(tokens: &[Token], expected: &str) -> bool {
    tokens.iter().any(|token| atom(token) == Some(expected))
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    tokens.get(index).and_then(atom)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn punctuation_at(tokens: &[Token], index: usize, character: char) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| punctuation(token, character))
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}
