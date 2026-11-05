use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{
    FunctionBody, TextFunctionSignature, TextItem, TextItemKind, TextModule, parse_function_body,
    parse_function_signature, parse_type,
};
use tir_layout::LayoutEngine;
use tir_types::{Abi, AddressSpace, FloatType, FunctionType, ParameterType, Type};

use crate::variant_text::read_symbol;

mod advanced_types;
mod capabilities;
mod graph;
pub(crate) mod instructions;
pub(super) mod policy;
mod ray_types;
mod register_only;
mod sync;
mod texture_types;
mod vft_types;
mod witness;

use graph::{GpuFunction, global_properties, has_capability, referenced_symbol, verify_call_graph};
use policy::TypePolicies;

pub(super) fn verify_sync_instruction(
    instruction: &tir_format::TextInstruction,
) -> Vec<Diagnostic> {
    sync::verify(instruction)
}

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

pub(super) fn verify_module(module: &TextModule, layouts: &mut LayoutEngine) -> Vec<Diagnostic> {
    let globals = global_properties(module);
    let policies = TypePolicies::from_module(module);
    let mut diagnostics = Vec::new();
    diagnostics.extend(tir_uniformity::verify_gpu_module(module));
    let mut functions = Vec::new();

    for item in &module.items {
        if !matches!(item.kind, TextItemKind::Function | TextItemKind::Entry) {
            continue;
        }
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        if !is_gpu_callable(item.kind, &tokens) {
            continue;
        }
        let Some((symbol, _, symbol_span)) =
            tokens.iter().enumerate().find_map(|(index, token)| {
                matches!(token.kind, TokenKind::Punctuation('@'))
                    .then(|| read_symbol(&tokens, index))
                    .flatten()
            })
        else {
            continue;
        };
        let Ok(Some(body)) = parse_function_body(source) else {
            continue;
        };
        let signature = parse_function_signature(source).ok().flatten();
        let header = &tokens[..header_end(&tokens)];
        let float64_capability = has_capability(header, "float64");
        let uses_float64 = function_uses_float64(source, &body, signature.as_ref(), &policies);
        diagnostics.extend(
            verify_function(
                source,
                *item,
                &tokens,
                &body,
                signature.as_ref(),
                module,
                &globals,
                &policies,
                layouts,
            )
            .into_iter()
            .map(|mut diagnostic| {
                shift(&mut diagnostic, item.span.start);
                diagnostic
            }),
        );
        functions.push(GpuFunction {
            item: *item,
            symbol,
            symbol_span,
            body,
            entry: item.kind == TextItemKind::Entry,
            exported: item.kind == TextItemKind::Function
                && has_linkage(header, "exported")
                && has_gpu_only_target(header),
            function_type: signature
                .as_ref()
                .and_then(|signature| declared_function_type(source, header, signature)),
            float64_capability,
            uses_float64,
        });
    }

    diagnostics.extend(verify_call_graph(&functions, module, &policies, layouts));
    diagnostics
}

fn verify_function(
    source: &str,
    item: TextItem,
    tokens: &[Token],
    body: &FunctionBody,
    signature: Option<&TextFunctionSignature>,
    module: &TextModule,
    globals: &HashMap<String, (bool, bool)>,
    policies: &TypePolicies,
    layouts: &LayoutEngine,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let header = &tokens[..header_end(tokens)];
    let static_values = crate::profile::constant_integer_results(source, body);
    diagnostics.extend(register_only::verify(
        source, item, body, signature, module, policies, layouts,
    ));

    if has_atom(header, "async") || has_atom(header, "coro") {
        diagnostics.push(
            Diagnostic::error(
                V15,
                "async and coroutine functions are forbidden in GPU code",
            )
            .with_primary_span(header.first().map_or(Span::new(0, 0), |token| token.span)),
        );
    }

    if let Some(signature) = signature {
        for parameter in &signature.parameters {
            check_type(&parameter.ty, parameter.span, policies, &mut diagnostics);
        }
        check_type(
            &signature.result,
            header.first().map_or(Span::new(0, 0), |token| token.span),
            policies,
            &mut diagnostics,
        );
    }

    for (block_index, block) in body.blocks.iter().enumerate() {
        for parameter in &block.parameters {
            check_type(&parameter.ty, parameter.span, policies, &mut diagnostics);
        }
        for instruction in &block.instructions {
            let opcode = instruction.opcode.as_str();
            if forbidden_opcode(opcode) {
                diagnostics.push(
                    Diagnostic::error(V15, format!("{opcode} is forbidden in GPU code"))
                        .with_primary_span(instruction.span),
                );
            }
            if opcode == "intrinsic" && !allowed_gpu_intrinsic(&instruction.tokens) {
                diagnostics.push(
                    Diagnostic::error(
                        V15,
                        "GPU code permits only the ct.select and ct.eq portable intrinsics",
                    )
                    .with_primary_span(instruction.span),
                );
            }
            if opcode == "tls.addr" {
                diagnostics.push(
                    Diagnostic::error(V15, "GPU code cannot access thread-local globals")
                        .with_primary_span(instruction.span),
                );
            }
            if opcode == "global.addr"
                && !referenced_symbol(&instruction.tokens)
                    .and_then(|symbol| globals.get(&symbol))
                    .is_some_and(|(constant_space, initialized)| *constant_space && *initialized)
            {
                diagnostics.push(
                    Diagnostic::error(
                        V15,
                        "GPU code may access only initialized constant-space globals",
                    )
                    .with_primary_span(instruction.span),
                );
            }
            if opcode == "stack.alloc" {
                if crate::profile::has_dynamic_count(instruction, &static_values) {
                    diagnostics.push(
                        Diagnostic::error(V15, "GPU stack.alloc sizes must be static")
                            .with_primary_span(instruction.span),
                    );
                }
                if has_space_suffix(&instruction.tokens, "threadgroup")
                    && (!item.kind.eq(&TextItemKind::Entry) || block_index != 0)
                {
                    diagnostics.push(
                        Diagnostic::error(
                            V15,
                            "threadgroup storage must be allocated in an entry's entry block",
                        )
                        .with_primary_span(instruction.span),
                    );
                }
            }
            for ty in explicit_types(source, instruction) {
                check_type(&ty, instruction.span, policies, &mut diagnostics);
            }
        }
    }
    diagnostics
}

fn is_gpu_callable(kind: TextItemKind, tokens: &[Token]) -> bool {
    if kind == TextItemKind::Entry {
        return true;
    }
    let end = header_end(tokens);
    let header = &tokens[..end];
    let mut in_target = false;
    for token in header {
        match &token.kind {
            TokenKind::Atom(name) if name == "target" => in_target = true,
            TokenKind::Punctuation(')') => in_target = false,
            TokenKind::Atom(name) if in_target && name == "gpu" => return true,
            _ => {}
        }
    }
    false
}

fn forbidden_opcode(opcode: &str) -> bool {
    matches!(
        opcode,
        "heap.alloc.counted"
            | "heap.alloc.unique"
            | "heap.alloc.array"
            | "heap.alloc.raw"
            | "heap.realloc.raw"
            | "heap.free.raw"
            | "weak.new"
            | "weak.load"
            | "ref.addr"
            | "uniq.addr"
            | "ref.unowned"
            | "ref.isunique"
            | "invoke"
            | "landingpad"
            | "resume"
            | "throw"
            | "try.call"
            | "call.variadic"
            | "await"
            | "setjmp"
            | "longjmp"
            | "asm"
            | "asm.goto"
            | "target.intrinsic"
            | "call.foreign"
            | "closure.new"
            | "call.closure"
            | "call.indirect"
            | "call.vtable"
            | "atomic.wait"
            | "atomic.notify"
            | "fp.flags.read"
            | "fp.flags.clear"
    ) || opcode.starts_with("heap.")
        || opcode.starts_with("coro.")
        || opcode.starts_with("async.")
        || opcode.starts_with("fp.flags.")
        || opcode.starts_with("va.")
}

fn allowed_gpu_intrinsic(tokens: &[Token]) -> bool {
    let start = crate::ssa::text_instruction_operands_start(tokens);
    tokens
        .get(start)
        .and_then(atom)
        .is_some_and(|id| id == "15" || id == "16")
}

fn has_space_suffix(tokens: &[Token], space: &str) -> bool {
    tokens.windows(3).rev().find_map(|window| {
        (matches!(window[0].kind, TokenKind::Punctuation('<'))
            && matches!(&window[1].kind, TokenKind::Atom(atom) if atom == space)
            && matches!(window[2].kind, TokenKind::Punctuation('>')))
        .then_some(space)
    }) == Some(space)
}

fn check_type(ty: &Type, span: Span, policies: &TypePolicies, diagnostics: &mut Vec<Diagnostic>) {
    let issue = type_issue(ty, policies);
    if let Some(message) = issue {
        diagnostics.push(Diagnostic::error(V15, message).with_primary_span(span));
    }
    if matches!(ty, Type::Nominal { .. })
        && policies.any_nested_type(ty, |nested| {
            nested != ty && type_issue(nested, policies).is_some()
        })
    {
        diagnostics.push(
            Diagnostic::error(
                V15,
                "GPU nominal type contains a field type forbidden by the GPU profile",
            )
            .with_primary_span(span),
        );
    }
    match ty {
        Type::FixedVector { element, .. }
        | Type::ScalableVector(element)
        | Type::Array { element, .. }
        | Type::Reference(element)
        | Type::Unique(element)
        | Type::Weak(element)
        | Type::Address {
            pointee: element, ..
        }
        | Type::Tensor { element, .. }
        | Type::CooperativeMatrix { element, .. } => {
            check_type(element, span, policies, diagnostics);
        }
        Type::Tuple(elements) => {
            for element in elements {
                check_type(element, span, policies, diagnostics);
            }
        }
        Type::Nominal { arguments, .. } => {
            for argument in arguments {
                check_type(argument, span, policies, diagnostics);
            }
        }
        Type::Function(signature) => {
            check_function_type(signature, span, policies, diagnostics);
        }
        Type::VisibleFunctionTable(signature) => {
            check_function_type(signature, span, policies, diagnostics);
        }
        Type::Closure(closure) => {
            check_function_type(&closure.signature, span, policies, diagnostics);
        }
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            check_type(yield_type, span, policies, diagnostics);
            check_type(resume_type, span, policies, diagnostics);
            check_type(result_type, span, policies, diagnostics);
        }
        Type::Texture { component, .. } => {
            check_type(component, span, policies, diagnostics);
        }
        Type::Entry {
            root, immediate, ..
        } => {
            check_type(root, span, policies, diagnostics);
            check_type(immediate, span, policies, diagnostics);
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

fn type_issue(ty: &Type, policies: &TypePolicies) -> Option<&'static str> {
    match ty {
        Type::Integer(bits) if *bits > 64 => Some("GPU integers cannot be wider than 64 bits"),
        Type::Float(FloatType::F128) => Some("f128 is forbidden in GPU code"),
        Type::ScalableVector(_) => Some("scalable vectors are forbidden in GPU code"),
        Type::FixedVector { lanes, .. } if !matches!(*lanes, 2 | 3 | 4) => {
            Some("GPU fixed vectors must have 2, 3, or 4 lanes")
        }
        Type::Pointer(AddressSpace::Generic) => {
            Some("GPU pointers must name a concrete address space")
        }
        Type::Reference(_) | Type::Unique(_) | Type::Weak(_) => {
            Some("reference-counted and unique reference types are forbidden in GPU code")
        }
        Type::Generic(_) => Some("GPU generics must be specialized before GPU lowering"),
        Type::Closure(_) => Some("closure types are forbidden in GPU code"),
        Type::Coroutine { .. } | Type::Frame(_) => {
            Some("coroutine and async frame types are forbidden in GPU code")
        }
        Type::Address { space, .. } if matches!(space, None | Some(AddressSpace::Generic)) => {
            Some("GPU addresses must name a concrete address space")
        }
        Type::Nominal { .. } if !policies.is_trivial(ty) => {
            Some("GPU value types must have trivial copy and destroy policies")
        }
        _ => None,
    }
}

fn check_function_type(
    signature: &FunctionType,
    span: Span,
    policies: &TypePolicies,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if signature.asynchronous {
        diagnostics.push(
            Diagnostic::error(V15, "async function types are forbidden in GPU code")
                .with_primary_span(span),
        );
    }
    for parameter in &signature.parameters {
        check_type(&parameter.ty, span, policies, diagnostics);
    }
    check_type(&signature.result, span, policies, diagnostics);
    if let Some(failure) = &signature.failure {
        check_type(failure, span, policies, diagnostics);
    }
}

pub(super) fn explicit_types(source: &str, instruction: &tir_format::TextInstruction) -> Vec<Type> {
    let start = crate::ssa::text_instruction_operands_start(&instruction.tokens);
    let mut types = HashSet::new();
    for start in start..instruction.tokens.len() {
        let first = &instruction.tokens[start];
        if matches!(
            first.kind,
            TokenKind::String(_) | TokenKind::Newline | TokenKind::End
        ) {
            continue;
        }
        for end in start + 1..=instruction.tokens.len() {
            let last = &instruction.tokens[end - 1];
            let Some(text) = source.get(first.span.start..last.span.end) else {
                break;
            };
            if let Ok(ty) = parse_type(text) {
                types.insert(ty);
            }
        }
    }
    types.into_iter().collect()
}

fn function_uses_float64(
    source: &str,
    body: &FunctionBody,
    signature: Option<&TextFunctionSignature>,
    policies: &TypePolicies,
) -> bool {
    if signature.is_some_and(|signature| {
        signature
            .parameters
            .iter()
            .any(|parameter| type_uses_float64(&parameter.ty, policies))
            || type_uses_float64(&signature.result, policies)
            || signature
                .failure
                .as_ref()
                .is_some_and(|failure| type_uses_float64(failure, policies))
    }) {
        return true;
    }
    for block in &body.blocks {
        if block
            .parameters
            .iter()
            .any(|parameter| type_uses_float64(&parameter.ty, policies))
        {
            return true;
        }
        if block.instructions.iter().any(|instruction| {
            explicit_types(source, instruction)
                .iter()
                .any(|ty| type_uses_float64(ty, policies))
        }) {
            return true;
        }
    }
    false
}

fn type_uses_float64(ty: &Type, policies: &TypePolicies) -> bool {
    policies.any_nested_type(ty, |ty| matches!(ty, Type::Float(FloatType::F64)))
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

fn has_linkage(header: &[Token], linkage: &str) -> bool {
    header
        .iter()
        .take_while(|token| !matches!(token.kind, TokenKind::Punctuation('@')))
        .any(|token| atom(token) == Some(linkage))
}

fn has_gpu_only_target(header: &[Token]) -> bool {
    let mut in_target = false;
    let mut has_cpu = false;
    let mut has_gpu = false;
    for token in header {
        match &token.kind {
            TokenKind::Atom(name) if name == "target" => {
                in_target = true;
                has_cpu = false;
                has_gpu = false;
            }
            TokenKind::Punctuation(')') if in_target => {
                let gpu_only = has_gpu && !has_cpu;
                in_target = false;
                if gpu_only {
                    return true;
                }
            }
            TokenKind::Atom(name) if in_target && name == "cpu" => has_cpu = true,
            TokenKind::Atom(name) if in_target && name == "gpu" => has_gpu = true,
            _ => {}
        }
    }
    false
}

fn declared_function_type(
    source: &str,
    header: &[Token],
    signature: &TextFunctionSignature,
) -> Option<FunctionType> {
    let arrow = header
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Arrow))?;
    let mut cursor = type_prefix_end(source, header, arrow + 1, &signature.result)?;
    if atom(header.get(cursor)?) == Some("fails") {
        cursor += 1;
        cursor = type_prefix_end(source, header, cursor, signature.failure.as_ref()?)?;
    } else if signature.failure.is_some() {
        return None;
    }

    let mut abi = Abi::Ridl;
    let mut asynchronous = false;
    let mut nesting = [0_i32; 3];
    for (index, token) in header.iter().enumerate().skip(cursor) {
        if nesting == [0; 3] {
            if atom(token) == Some("async") {
                asynchronous = true;
            }
            if atom(token) == Some("abi")
                && header
                    .get(index + 1)
                    .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('(')))
            {
                abi = match header.get(index + 2).and_then(atom) {
                    Some("c") => Abi::C,
                    Some("ridl") => Abi::Ridl,
                    _ => abi,
                };
            }
        }
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '[' => nesting[0] += 1,
                ']' => nesting[0] -= 1,
                '<' => nesting[1] += 1,
                '>' => nesting[1] -= 1,
                '(' => nesting[2] += 1,
                ')' => nesting[2] -= 1,
                _ => {}
            }
        }
    }
    Some(FunctionType {
        parameters: signature
            .parameters
            .iter()
            .map(|parameter| ParameterType {
                context: parameter.context,
                convention: parameter.convention,
                ty: parameter.ty.clone(),
            })
            .collect(),
        result: Box::new(signature.result.clone()),
        failure: signature.failure.clone().map(Box::new),
        asynchronous,
        abi,
        foreign: false,
    })
}

fn type_prefix_end(source: &str, tokens: &[Token], start: usize, expected: &Type) -> Option<usize> {
    (start + 1..=tokens.len()).rev().find(|end| {
        let Some(first) = tokens.get(start) else {
            return false;
        };
        let Some(last) = tokens.get(end - 1) else {
            return false;
        };
        source
            .get(first.span.start..last.span.end)
            .and_then(|text| parse_type(text).ok())
            .is_some_and(|ty| &ty == expected)
    })
}

fn header_end(tokens: &[Token]) -> usize {
    tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len())
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
