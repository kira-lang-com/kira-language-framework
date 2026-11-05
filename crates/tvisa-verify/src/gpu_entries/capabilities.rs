use std::collections::{HashMap, HashSet};

use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction, parse_tvisa_function_body, parse_tvisa_type};
use tir_types::{AddressSpace, FloatType, Type};
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind, TextSignature};

use super::super::gpu::{direct_call_target, function_signature_name, function_symbol, opcode_end};

struct Function {
    name: String,
    signature: TextSignature,
    body: FunctionBody,
    source: String,
}

pub(super) struct EntryUse {
    pub(super) capabilities: HashSet<String>,
    pub(super) specialization_constants: HashMap<u64, SpecializationConstant>,
    pub(super) conflicting_specialization_constants: HashSet<u64>,
}

pub(super) struct SpecializationConstant {
    pub(super) ty: String,
    pub(super) default: String,
}

pub(super) fn analyze(image: &TextImage, root: &str, stage: &str) -> EntryUse {
    let mut functions = Vec::new();
    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let Some(signature_name) = function_signature_name(source) else {
            continue;
        };
        let Some(signature) = image.signatures.iter().find(|signature| {
            signature.name == signature_name
                && matches!(
                    signature.convention,
                    TextCallingConvention::Gpu | TextCallingConvention::Entry
                )
        }) else {
            continue;
        };
        let Some(name) = function_symbol(source) else {
            continue;
        };
        let Ok(Some(body)) = parse_tvisa_function_body(source) else {
            continue;
        };
        functions.push(Function {
            name,
            signature: signature.clone(),
            body,
            source: source.to_owned(),
        });
    }
    let indices: HashMap<_, _> = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.name.as_str(), index))
        .collect();
    let signatures: HashMap<_, _> = functions
        .iter()
        .map(|function| (function.name.as_str(), &function.signature))
        .collect();
    let Some(root) = indices.get(root).copied() else {
        return EntryUse {
            capabilities: HashSet::new(),
            specialization_constants: HashMap::new(),
            conflicting_specialization_constants: HashSet::new(),
        };
    };
    let mut result = EntryUse {
        capabilities: HashSet::new(),
        specialization_constants: HashMap::new(),
        conflicting_specialization_constants: HashSet::new(),
    };
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    while let Some(index) = pending.pop() {
        if !visited.insert(index) {
            continue;
        }
        let function = &functions[index];
        let value_types = inferred_value_types(function, &signatures);
        let mut types = signature_types(&function.signature);
        types.extend(
            function
                .body
                .blocks
                .iter()
                .flat_map(|block| &block.parameters)
                .map(|parameter| parameter.ty.clone()),
        );
        types.extend(value_types.values().cloned());
        types.extend(
            function
                .body
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .filter_map(|instruction| {
                    crate::instruction_types::explicit_type(&function.source, instruction)
                }),
        );
        collect_type_capabilities(&types, &mut result.capabilities);
        for instruction in function
            .body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
        {
            collect_instruction_capabilities(
                &function.source,
                instruction,
                stage,
                &value_types,
                &mut result.capabilities,
            );
            if instruction.opcode == "gpu.spec_const"
                && let Some((id, constant)) = specialization_constant(&function.source, instruction)
            {
                if let Some(previous) = result.specialization_constants.get(&id) {
                    if previous.ty != constant.ty || previous.default != constant.default {
                        result.conflicting_specialization_constants.insert(id);
                    }
                } else {
                    result.specialization_constants.insert(id, constant);
                }
            }
            if let Some(callee) = direct_call_target(instruction)
                && let Some(index) = indices.get(callee.as_str())
            {
                pending.push(*index);
            }
        }
    }
    result
}

fn signature_types(signature: &TextSignature) -> Vec<Type> {
    let mut types =
        crate::call_types::signature_parameter_types(signature, signature.parameters.len());
    types.extend(crate::call_types::signature_result_types(signature));
    types
}

fn inferred_value_types(
    function: &Function,
    signatures: &HashMap<&str, &TextSignature>,
) -> HashMap<String, Type> {
    let mut value_types = HashMap::new();
    for parameter in function
        .body
        .blocks
        .iter()
        .flat_map(|block| &block.parameters)
    {
        value_types.insert(parameter.name.clone(), parameter.ty.clone());
    }
    let slot_types = HashMap::new();
    for instruction in function
        .body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
    {
        if let Some(callee) = direct_call_target(instruction)
            && let Some(signature) = signatures.get(callee.as_str())
        {
            value_types.extend(
                instruction
                    .results
                    .iter()
                    .cloned()
                    .zip(crate::call_types::signature_result_types(signature)),
            );
        }
        value_types.extend(crate::call_types::infer_instruction_results(
            &function.source,
            instruction,
            &value_types,
            &slot_types,
            true,
        ));
    }
    value_types
}

fn collect_type_capabilities(types: &[Type], required: &mut HashSet<String>) {
    for ty in types {
        visit_type(ty, &mut |nested| match nested {
            Type::Float(FloatType::F64) => add(required, "float64"),
            Type::Pointer(AddressSpace::Tile)
            | Type::Address {
                space: Some(AddressSpace::Tile),
                ..
            } => add(required, "tile"),
            Type::Pointer(AddressSpace::NodeRecord)
            | Type::Address {
                space: Some(AddressSpace::NodeRecord),
                ..
            } => add(required, "work_graphs"),
            Type::CooperativeMatrix { element, .. } => match element.as_ref() {
                Type::Float(FloatType::F8E4M3 | FloatType::F8E5M2) => add(required, "coop.fp8"),
                Type::Float(FloatType::F4E2M1 | FloatType::F6E2M3 | FloatType::F6E3M2) => {
                    add(required, "coop.blockscaled")
                }
                _ => {}
            },
            _ => {}
        });
    }
}

fn visit_type(ty: &Type, visit: &mut impl FnMut(&Type)) {
    visit(ty);
    match ty {
        Type::FixedVector { element, .. }
        | Type::ScalableVector(element)
        | Type::Reference(element)
        | Type::Unique(element)
        | Type::Weak(element)
        | Type::Tensor { element, .. } => visit_type(element, visit),
        Type::Tuple(elements)
        | Type::Nominal {
            arguments: elements,
            ..
        } => {
            for element in elements {
                visit_type(element, visit);
            }
        }
        Type::Array { element, .. } => visit_type(element, visit),
        Type::Address { pointee, .. } => visit_type(pointee, visit),
        Type::Function(signature) => visit_function_type(signature, visit),
        Type::VisibleFunctionTable(signature) => visit_function_type(signature, visit),
        Type::Closure(closure) => visit_type(&Type::Function(closure.signature.clone()), visit),
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            visit_type(yield_type, visit);
            visit_type(resume_type, visit);
            visit_type(result_type, visit);
        }
        Type::Texture { component, .. } => visit_type(component, visit),
        Type::CooperativeMatrix { element, .. } => visit_type(element, visit),
        Type::Entry {
            root, immediate, ..
        } => {
            visit_type(root, visit);
            visit_type(immediate, visit);
        }
        Type::Integer(_)
        | Type::Float(_)
        | Type::Pointer(_)
        | Type::Code
        | Type::Unit
        | Type::Never
        | Type::Generic(_)
        | Type::Frame(_)
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::RayQuery
        | Type::HitObject => {}
    }
}

fn visit_function_type(signature: &tir_types::FunctionType, visit: &mut impl FnMut(&Type)) {
    for parameter in &signature.parameters {
        visit_type(&parameter.ty, visit);
    }
    visit_type(&signature.result, visit);
    if let Some(failure) = &signature.failure {
        visit_type(failure, visit);
    }
}

fn specialization_constant(
    source: &str,
    instruction: &TextInstruction,
) -> Option<(u64, SpecializationConstant)> {
    let tokens = &instruction.tokens;
    let start = opcode_end(instruction);
    let commas = top_level_commas(tokens, start);
    let (&first, &second) = (commas.first()?, commas.get(1)?);
    let id = tir_format::numeric_literals::parse_unsigned_integer(atom(tokens.get(start)?)?)?;
    let id = u64::from(u32::try_from(id).ok()?);
    let ty = text_for_range(source, tokens, first + 1, second)?;
    let ty = parse_tvisa_type(&ty).ok()?.to_string();
    let default = text_for_range(source, tokens, second + 1, tokens.len())?;
    Some((id, SpecializationConstant { ty, default }))
}

fn top_level_commas(tokens: &[Token], start: usize) -> Vec<usize> {
    let mut commas = Vec::new();
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if let TokenKind::Punctuation(punctuation) = token.kind {
            match punctuation {
                '<' | '[' | '(' => depth += 1,
                '>' | ']' | ')' => depth -= 1,
                ',' if depth == 0 => commas.push(index),
                _ => {}
            }
        }
    }
    commas
}

fn text_for_range(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<String> {
    let first = tokens.get(start)?;
    let last = tokens.get(end.checked_sub(1)?)?;
    super::fields::normalize_text(source.get(first.span.start..last.span.end)?)
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(atom) = &token.kind {
        Some(atom)
    } else {
        None
    }
}

fn collect_instruction_capabilities(
    source: &str,
    instruction: &TextInstruction,
    stage: &str,
    value_types: &HashMap<String, Type>,
    required: &mut HashSet<String>,
) {
    let opcode = instruction.opcode.as_str();
    match opcode {
        "gpu.printf" => add(required, "printf"),
        "tex.feedback" => add(required, "sampler_feedback"),
        "frag.load_attachment" => add(required, "framebuffer_fetch"),
        "frag.raster_order.begin" | "frag.raster_order.end" => add(required, "raster_order"),
        "tile.imageblock.addr" | "frag.imageblock.addr" => add(required, "tile"),
        "node.input.addr"
        | "node.input.count"
        | "node.output.allocate"
        | "node.output.addr"
        | "node.output.commit"
        | "node.finished_cross_group" => add(required, "work_graphs"),
        "coop.reduce" => add(required, "coop.reduce"),
        "gpu.ddx" | "gpu.ddy" | "gpu.fwidth" | "tex.sample" | "tex.sample_bias"
        | "tex.sample_compare" | "tex.gather" | "tex.gather_compare" | "tex.lod"
            if stage == "compute" =>
        {
            add(required, "compute_derivatives")
        }
        _ => {}
    }
    if opcode == "tex.feedback" && stage == "compute" {
        add(required, "compute_derivatives");
    }
    if opcode.starts_with("tex.") && has_atom(instruction, "sparse") {
        add(required, "sparse_residency");
    }
    if opcode.starts_with("coop.") && has_atom(instruction, "e8m0") {
        add(required, "coop.blockscaled");
    }
    if opcode.starts_with("atomic.")
        && let Some(ty) = atomic_type(source, instruction, value_types)
    {
        if contains_float_type(&ty, FloatType::F16) {
            add(required, "float16.atomics");
        }
        if crate::conversion_types::static_size(&ty) == Some(16) {
            add(required, "atomic128");
        }
    }
    if opcode == "gpu.builtin"
        && instruction.tokens.get(opcode_end(instruction)).is_some_and(
            |token| matches!(&token.kind, TokenKind::Atom(atom) if atom == "shading_rate"),
        )
    {
        add(required, "shading_rate");
    }
    if opcode == "rt.get"
        && instruction.tokens[opcode_end(instruction)..]
            .iter()
            .any(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == "triangle_vertex_positions"))
    {
        add(required, "triangle_vertex_positions");
    }
}

fn atomic_type(
    source: &str,
    instruction: &TextInstruction,
    value_types: &HashMap<String, Type>,
) -> Option<Type> {
    if instruction.opcode == "atomic.load" {
        return crate::instruction_types::explicit_type(source, instruction);
    }
    let values = instruction.tokens[opcode_end(instruction)..]
        .windows(2)
        .filter_map(|window| {
            if matches!(window[0].kind, TokenKind::Punctuation('%')) {
                atom(&window[1]).and_then(|name| value_types.get(name))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    match instruction.opcode.as_str() {
        "atomic.store" => values.first(),
        "atomic.rmw" | "atomic.cmpxchg" => values.last(),
        _ => None,
    }
    .copied()
    .cloned()
}

fn contains_float_type(ty: &Type, expected: FloatType) -> bool {
    let mut found = false;
    visit_type(ty, &mut |nested| {
        found |= matches!(nested, Type::Float(kind) if *kind == expected);
    });
    found
}

fn has_atom(instruction: &TextInstruction, expected: &str) -> bool {
    instruction
        .tokens
        .iter()
        .any(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == expected))
}

fn add(required: &mut HashSet<String>, capability: &str) {
    required.insert(capability.to_owned());
}
