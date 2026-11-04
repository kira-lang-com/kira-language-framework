use std::collections::{HashMap, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::TokenKind;
use tir_format::{FunctionBody, TextInstruction};
use tir_types::{AddressSpace, FloatType, Type};
use tvisa_format::{
    ParameterItem, ResultItem, TextCallingConvention, TextImage, TextItemKind, TextSignature,
};

const VFY21: Code = Code::new(Dialect::Tvisa, Area::Verifier, 21);

mod instructions;
use instructions::verify_instruction;
const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

struct GpuFunction {
    name: String,
    callees: Vec<String>,
    span: Span,
}

pub(super) fn verify_call_graph(image: &TextImage) -> Vec<Diagnostic> {
    let mut functions = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let Some(signature_name) = function_signature_name(source) else {
            continue;
        };
        let Some(signature) = image
            .signatures
            .iter()
            .find(|signature| signature.name == signature_name)
        else {
            continue;
        };
        if !matches!(
            signature.convention,
            TextCallingConvention::Gpu | TextCallingConvention::Entry
        ) {
            continue;
        }
        let Some(name) = function_symbol(source) else {
            continue;
        };
        let Some(body) = image
            .function_body_for_item(*item)
            .and_then(|parsed| parsed.body.as_ref())
        else {
            continue;
        };
        let callees = body
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter())
            .filter_map(direct_call_target)
            .collect();
        functions.push(GpuFunction {
            name,
            callees,
            span: item.span,
        });
    }

    let indices: HashMap<_, _> = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.name.as_str(), index))
        .collect();
    let mut edges = vec![Vec::new(); functions.len()];
    let mut incoming = vec![0usize; functions.len()];
    for (caller, function) in functions.iter().enumerate() {
        for callee in &function.callees {
            let Some(target) = indices.get(callee.as_str()).copied() else {
                continue;
            };
            edges[caller].push(target);
            incoming[target] += 1;
        }
    }

    let mut ready = VecDeque::new();
    for (index, count) in incoming.iter().enumerate() {
        if *count == 0 {
            ready.push_back(index);
        }
    }
    let mut visited = 0;
    while let Some(caller) = ready.pop_front() {
        visited += 1;
        for callee in &edges[caller] {
            incoming[*callee] -= 1;
            if incoming[*callee] == 0 {
                ready.push_back(*callee);
            }
        }
    }
    if visited == functions.len() {
        return Vec::new();
    }
    let cycle_function = incoming
        .iter()
        .position(|count| *count != 0)
        .unwrap_or_default();
    vec![
        Diagnostic::error(VFY21, "GPU direct-call graph contains a cycle")
            .with_primary_span(functions[cycle_function].span),
    ]
}

pub(super) fn verify_builtin_stages(image: &TextImage) -> Vec<Diagnostic> {
    let mut functions = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let Some(signature_name) = function_signature_name(source) else {
            continue;
        };
        if !image.signatures.iter().any(|signature| {
            signature.name == signature_name
                && matches!(
                    signature.convention,
                    TextCallingConvention::Gpu | TextCallingConvention::Entry
                )
        }) {
            continue;
        }
        let Some(name) = function_symbol(source) else {
            continue;
        };
        let Some(body) = image
            .function_body_for_item(*item)
            .and_then(|parsed| parsed.body.as_ref())
        else {
            continue;
        };
        functions.push((name, body.clone(), item.span.start));
    }
    let indices: HashMap<_, _> = functions
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| (name.as_str(), index))
        .collect();
    let mut diagnostics = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Entry {
            continue;
        }
        let Some(entry) = image.gpu_entry_for_item(*item) else {
            continue;
        };
        let (Some(stage), Some(target)) = (entry.kind.as_deref(), entry.function.as_deref()) else {
            continue;
        };
        if !entry.header_valid {
            continue;
        }
        let Some(root) = indices.get(target).copied() else {
            continue;
        };
        let mut pending = vec![root];
        let mut visited = HashSet::new();
        while let Some(index) = pending.pop() {
            if !visited.insert(index) {
                continue;
            }
            let (_, body, base) = &functions[index];
            for block in &body.blocks {
                for instruction in &block.instructions {
                    let required_stage = match instruction.opcode.as_str() {
                        "frag.demote"
                        | "frag.load_attachment"
                        | "frag.raster_order.begin"
                        | "frag.raster_order.end"
                        | "frag.imageblock.addr" => Some("fragment"),
                        "rt.report_hit" => Some("intersection"),
                        "rt.ignore_hit" | "rt.accept_and_end" => Some("anyhit"),
                        "object.dispatch_mesh" => Some("object"),
                        "mesh.set_counts"
                        | "mesh.write.vertex"
                        | "mesh.write.primitive"
                        | "mesh.write.indices" => Some("mesh"),
                        "tile.imageblock.addr" => Some("tile"),
                        opcode if opcode.starts_with("node.") => Some("node"),
                        _ => None,
                    };
                    if let Some(required) = required_stage
                        && required != stage
                    {
                        let span = Span::new(
                            instruction.span.start.saturating_add(*base),
                            instruction.span.end.saturating_add(*base),
                        );
                        diagnostics.push(
                            Diagnostic::error(
                                VFY3,
                                format!(
                                    "{} is only available in {required} entries",
                                    instruction.opcode
                                ),
                            )
                            .with_primary_span(span),
                        );
                    }
                    if matches!(
                        instruction.opcode.as_str(),
                        "tex.sample"
                            | "tex.sample_bias"
                            | "tex.sample_compare"
                            | "tex.gather"
                            | "tex.gather_compare"
                            | "tex.lod"
                            | "tex.feedback"
                    ) && !matches!(stage, "fragment" | "compute")
                    {
                        let span = Span::new(
                            instruction.span.start.saturating_add(*base),
                            instruction.span.end.saturating_add(*base),
                        );
                        diagnostics.push(
                            Diagnostic::error(
                                VFY3,
                                format!(
                                    "{} with implicit derivatives requires the fragment or compute stage",
                                    instruction.opcode
                                ),
                            )
                            .with_primary_span(span),
                        );
                    }
                    if instruction.opcode == "gpu.call.vft"
                        && !matches!(
                            stage,
                            "compute"
                                | "raygen"
                                | "intersection"
                                | "anyhit"
                                | "closesthit"
                                | "miss"
                                | "callable"
                        )
                    {
                        let span = Span::new(
                            instruction.span.start.saturating_add(*base),
                            instruction.span.end.saturating_add(*base),
                        );
                        diagnostics.push(
                            Diagnostic::error(
                                VFY3,
                                format!("gpu.call.vft is not available in {stage} entries"),
                            )
                            .with_primary_span(span),
                        );
                    }
                    if instruction.opcode == "gpu.builtin"
                        && let Some(name) = instruction
                            .tokens
                            .get(opcode_end(instruction))
                            .and_then(atom)
                        && let Some(stages) = builtin_stages(name)
                        && !stages.contains(&stage)
                    {
                        let span = Span::new(
                            instruction.span.start.saturating_add(*base),
                            instruction.span.end.saturating_add(*base),
                        );
                        diagnostics.push(
                            Diagnostic::error(
                                VFY3,
                                format!("GPU builtin {name} is not available in {stage} entries"),
                            )
                            .with_primary_span(span),
                        );
                    }
                    if let Some(callee) = direct_call_target(instruction)
                        && let Some(callee_index) = indices.get(callee.as_str())
                    {
                        pending.push(*callee_index);
                    }
                }
            }
        }
    }
    diagnostics
}

pub(super) fn builtin_stages(name: &str) -> Option<&'static [&'static str]> {
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

pub(super) fn function_signature_name(source: &str) -> Option<String> {
    let tokens = tir_format::text::lex(source).ok()?;
    tokens.windows(3).find_map(|window| {
        if matches!(&window[0].kind, TokenKind::Atom(atom) if atom == "sig")
            && matches!(window[1].kind, TokenKind::Punctuation('$'))
        {
            if let TokenKind::Atom(name) = &window[2].kind {
                return Some(name.clone());
            }
        }
        None
    })
}

pub(super) fn function_symbol(source: &str) -> Option<String> {
    let tokens = tir_format::text::lex(source).ok()?;
    let at = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('@')))?;
    let start = tokens.get(at + 1)?.span.start;
    let signature = tokens
        .iter()
        .enumerate()
        .skip(at + 1)
        .find_map(|(index, token)| {
            matches!(&token.kind, TokenKind::Atom(atom) if atom == "sig").then_some(index)
        })?;
    let end = tokens.get(signature.checked_sub(1)?)?.span.end;
    source.get(start..end).map(|name| name.trim().to_owned())
}

pub(super) fn direct_call_target(instruction: &TextInstruction) -> Option<String> {
    if !matches!(instruction.opcode.as_str(), "call" | "tail.call" | "invoke") {
        return None;
    }
    let tokens = &instruction.tokens;
    let opcode_end = opcode_end(instruction);
    let at = tokens
        .iter()
        .enumerate()
        .skip(opcode_end)
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('@')).then_some(index)
        })?;
    let open = tokens
        .iter()
        .enumerate()
        .skip(at + 1)
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('(')).then_some(index)
        })?;
    let mut name = String::new();
    for token in &tokens[at + 1..open] {
        match &token.kind {
            TokenKind::Atom(component) => name.push_str(component),
            TokenKind::Punctuation('.') => name.push('.'),
            _ => return None,
        }
    }
    (!name.is_empty()).then_some(name)
}

pub(super) fn verify_function(signature: &TextSignature, body: &FunctionBody) -> Vec<Diagnostic> {
    if !matches!(
        signature.convention,
        TextCallingConvention::Gpu | TextCallingConvention::Entry
    ) {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            verify_instruction(instruction, &mut diagnostics);
        }
    }
    diagnostics
}

pub(super) fn verify_value_type(ty: &Type, span: Span) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    visit_type(ty, span, &mut diagnostics);
    diagnostics
}

pub(super) fn verify_image_signatures(image: &TextImage) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for signature in &image.signatures {
        if !matches!(
            signature.convention,
            TextCallingConvention::Gpu | TextCallingConvention::Entry
        ) {
            continue;
        }
        if signature.fails || signature.variadic.is_some() {
            diagnostics.push(
                Diagnostic::error(VFY21, "GPU signatures cannot fail or be variadic")
                    .with_primary_span(signature.span),
            );
        }
        for parameter in &signature.parameters {
            visit_parameter(parameter, signature.span, &mut diagnostics);
        }
        for result in &signature.results {
            visit_result(result, signature.span, &mut diagnostics);
        }
    }
    diagnostics
}

fn visit_parameter(parameter: &ParameterItem, span: Span, diagnostics: &mut Vec<Diagnostic>) {
    match parameter {
        ParameterItem::Leaf(ty) => visit_type(ty, span, diagnostics),
        ParameterItem::Group(group) => {
            for member in &group.members {
                visit_type(&member.ty, span, diagnostics);
            }
        }
        ParameterItem::CAggregate(aggregate) => {
            for member in &aggregate.members {
                visit_type(&member.ty, span, diagnostics);
            }
        }
        ParameterItem::ByRef { .. } | ParameterItem::Sret { .. } | ParameterItem::Context => {}
    }
}

fn visit_result(result: &ResultItem, span: Span, diagnostics: &mut Vec<Diagnostic>) {
    match result {
        ResultItem::Leaf(ty) => visit_type(ty, span, diagnostics),
        ResultItem::Group(group) => {
            for member in &group.members {
                visit_type(&member.ty, span, diagnostics);
            }
        }
    }
}

fn visit_type(ty: &Type, span: Span, diagnostics: &mut Vec<Diagnostic>) {
    let violation = match ty {
        Type::Integer(bits) if *bits > 64 => {
            Some("GPU code does not support integers wider than 64 bits")
        }
        Type::Float(FloatType::F128) => Some("GPU code does not support f128"),
        Type::Pointer(AddressSpace::Generic) => {
            Some("GPU code must use an explicit pointer address space")
        }
        Type::Pointer(_)
        | Type::Integer(_)
        | Type::Float(_)
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::RayQuery
        | Type::HitObject => None,
        Type::Code => Some("GPU code cannot use CPU code pointers"),
        Type::FixedVector { lanes, element } => {
            visit_type(element, span, diagnostics);
            if !matches!(*lanes, 2 | 3 | 4) {
                Some("GPU fixed vectors must have 2, 3, or 4 lanes")
            } else {
                None
            }
        }
        Type::ScalableVector(element) => {
            visit_type(element, span, diagnostics);
            Some("GPU code cannot use scalable vectors")
        }
        Type::Texture { component, .. }
        | Type::Tensor {
            element: component, ..
        }
        | Type::CooperativeMatrix {
            element: component, ..
        } => {
            visit_type(component, span, diagnostics);
            None
        }
        Type::VisibleFunctionTable(function) => {
            for parameter in &function.parameters {
                visit_type(&parameter.ty, span, diagnostics);
            }
            visit_type(&function.result, span, diagnostics);
            if let Some(failure) = &function.failure {
                visit_type(failure, span, diagnostics);
            }
            None
        }
        Type::Tuple(_)
        | Type::Array { .. }
        | Type::Nominal { .. }
        | Type::Generic(_)
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address { .. }
        | Type::Function(_)
        | Type::Closure(_)
        | Type::Coroutine { .. }
        | Type::Frame(_)
        | Type::Unit
        | Type::Never
        | Type::Entry { .. } => Some("type is not permitted in the GPU value profile"),
    };
    if let Some(message) = violation {
        diagnostics.push(Diagnostic::error(VFY21, message).with_primary_span(span));
    }
}

pub(super) fn opcode_end(instruction: &TextInstruction) -> usize {
    let tokens = &instruction.tokens;
    let mut index = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        while index < tokens.len() {
            if matches!(tokens[index].kind, TokenKind::Punctuation('=')) {
                index += 1;
                break;
            }
            index += 1;
        }
    }
    index += 1;
    while matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(TokenKind::Punctuation('.'))
    ) && index + 1 < tokens.len()
    {
        index += 2;
    }
    index
}

fn atom(token: &tir_format::text::Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}
