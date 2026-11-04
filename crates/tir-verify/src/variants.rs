use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::TextItemKind;
use tir_format::text::TokenKind;
use tir_format::{TextModule, parse_function_body};
use tir_target::{Platform, known_features};

use crate::variant_text::{
    FunctionHeader, VariantArchitecture, VariantKey, VariantMember, parse_function_header,
    parse_variant_set,
};

const V18: Code = Code::new(Dialect::Tir, Area::Verifier, 18);
const V9: Code = Code::new(Dialect::Tir, Area::Verifier, 9);

#[derive(Clone, Debug)]
struct FunctionRecord {
    header: FunctionHeader,
    has_assembly: bool,
    has_body: bool,
    offset: usize,
}

pub(super) fn verify_text_module(module: &TextModule) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut functions = HashMap::new();
    let mut membership: HashMap<String, Vec<Span>> = HashMap::new();

    for item in &module.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = module.item_text(*item);
        let header = match parse_function_header(source) {
            Ok(header) => header,
            Err((span, message)) => {
                push(&mut diagnostics, item.span.start, span, message);
                continue;
            }
        };
        let (has_body, has_assembly) = match parse_function_body(source) {
            Ok(Some(body)) => {
                let mut has_assembly = false;
                for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                    if matches!(header.engine.as_deref(), Some("runtime" | "any"))
                        && matches!(
                            instruction.opcode.as_str(),
                            "asm"
                                | "asm.goto"
                                | "target.intrinsic"
                                | "mmio.load"
                                | "mmio.store"
                                | "fence.io"
                        )
                    {
                        diagnostics.push(
                            Diagnostic::error(
                                V9,
                                format!(
                                    "{} is forbidden in a runtime or any engine function",
                                    instruction.opcode
                                ),
                            )
                            .with_primary_span(Span::new(
                                instruction.span.start.saturating_add(item.span.start),
                                instruction.span.end.saturating_add(item.span.start),
                            )),
                        );
                    }
                    match instruction.opcode.as_str() {
                        "asm" | "asm.goto" => {
                            has_assembly = true;
                            if !has_arch_variant(header.variant.as_ref()) {
                                push(
                                    &mut diagnostics,
                                    item.span.start,
                                    instruction.span,
                                    "asm and asm.goto require an architecture-specific variant body",
                                );
                            }
                        }
                        "target.intrinsic" => {
                            if let Some(architecture) = intrinsic_architecture(instruction) {
                                if header.variant.as_ref().map(|variant| variant.architecture)
                                    != Some(architecture)
                                {
                                    push(
                                        &mut diagnostics,
                                        item.span.start,
                                        instruction.span,
                                        format!(
                                            "{} target intrinsic must be in a {} variant body",
                                            architecture.name(),
                                            architecture.name()
                                        ),
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
                (true, has_assembly)
            }
            Ok(None) | Err(_) => (false, false),
        };
        functions.insert(
            header.symbol.clone(),
            FunctionRecord {
                header,
                has_assembly,
                has_body,
                offset: item.span.start,
            },
        );
    }

    for item in &module.items {
        if item.kind != TextItemKind::Variants {
            continue;
        }
        let members = match parse_variant_set(module.item_text(*item)) {
            Ok(members) => members,
            Err((span, message)) => {
                push(&mut diagnostics, item.span.start, span, message);
                continue;
            }
        };
        verify_set(
            module,
            *item,
            &members,
            &functions,
            &mut membership,
            &mut diagnostics,
        );
    }

    for (name, function) in &functions {
        if function.header.variant.is_some() && !membership.contains_key(name) {
            push(
                &mut diagnostics,
                function.offset,
                function.header.symbol_span,
                format!("variant function @{name} is not in a variant set"),
            );
        } else if function.header.variant.is_some()
            && membership.get(name).is_some_and(|sets| sets.len() > 1)
        {
            push(
                &mut diagnostics,
                function.offset,
                function.header.symbol_span,
                format!("variant function @{name} belongs to more than one set"),
            );
        }
    }
    diagnostics
}

fn verify_set(
    module: &TextModule,
    item: tir_format::TextItem,
    members: &[VariantMember],
    functions: &HashMap<String, FunctionRecord>,
    membership: &mut HashMap<String, Vec<Span>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut signatures: Option<FunctionHeader> = None;
    let mut member_keys = HashSet::new();
    let mut member_functions = HashSet::new();
    let mut has_assembly = false;

    for member in members {
        if !member_keys.insert(member.key.clone()) {
            push(
                diagnostics,
                item.span.start,
                member.span,
                "duplicate architecture and feature variant in set",
            );
        }
        if !member_functions.insert(member.function.clone()) {
            push(
                diagnostics,
                item.span.start,
                member.span,
                format!(
                    "function @{} appears more than once in this set",
                    member.function
                ),
            );
        }
        membership
            .entry(member.function.clone())
            .or_default()
            .push(shift(member.span, item.span.start));
        validate_features(&member.key, member.span, item.span.start, diagnostics);

        let Some(function) = functions.get(&member.function) else {
            push(
                diagnostics,
                item.span.start,
                member.span,
                format!(
                    "variant member @{} must name a defined function",
                    member.function
                ),
            );
            continue;
        };
        has_assembly |= function.has_assembly;
        if !function.has_body {
            push(
                diagnostics,
                item.span.start,
                member.span,
                format!(
                    "variant member @{} must have a function body",
                    member.function
                ),
            );
        }
        if !same_variant(&member.key, function.header.variant.as_ref()) {
            push(
                diagnostics,
                item.span.start,
                member.span,
                format!(
                    "variant member @{} must have a matching variant attribute",
                    member.function
                ),
            );
        }
        if function.header.engine.as_deref() != Some("native") {
            push(
                diagnostics,
                item.span.start,
                member.span,
                format!(
                    "variant member @{} must use engine(native)",
                    member.function
                ),
            );
        }
        if !function.header.target_cpu || function.header.target_gpu {
            push(
                diagnostics,
                item.span.start,
                member.span,
                format!("variant member @{} must be CPU-only code", member.function),
            );
        }
        if let Some(reference) = &signatures {
            if reference.signature != function.header.signature
                || reference.abi != function.header.abi
            {
                push(
                    diagnostics,
                    item.span.start,
                    member.span,
                    format!(
                        "variant member @{} must have the same signature and ABI as its set",
                        member.function
                    ),
                );
            }
        } else {
            signatures = Some(function.header.clone());
        }
    }

    let has_portable = members
        .iter()
        .any(|member| member.key.architecture == VariantArchitecture::Portable);
    let has_wasm = members
        .iter()
        .any(|member| member.key.architecture == VariantArchitecture::Wasm);
    let has_three_bases = [
        VariantArchitecture::Aarch64,
        VariantArchitecture::X64,
        VariantArchitecture::Rv64,
    ]
    .into_iter()
    .all(|architecture| {
        members
            .iter()
            .any(|member| member.key.architecture == architecture && member.key.features.is_empty())
    });

    if has_assembly {
        if !has_three_bases {
            push(
                diagnostics,
                0,
                item.span,
                "a variant set containing assembly requires feature-free aarch64, x64, and rv64 members",
            );
        }
    } else if !has_portable && !has_three_bases {
        push(
            diagnostics,
            0,
            item.span,
            "a variant set without assembly requires a portable member or feature-free aarch64, x64, and rv64 members",
        );
    }

    if matches!(module.header.platform, Some(Platform::Web | Platform::Wasi)) {
        if has_assembly && !has_wasm {
            push(
                diagnostics,
                0,
                item.span,
                "a variant set with assembly in a web or wasi module requires a wasm member",
            );
        } else if !has_assembly && !has_portable && !has_wasm {
            push(
                diagnostics,
                0,
                item.span,
                "a variant set in a web or wasi module requires a portable or wasm member",
            );
        }
    }
}

fn validate_features(
    key: &VariantKey,
    span: Span,
    offset: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if key.architecture == VariantArchitecture::Portable && !key.features.is_empty() {
        push(
            diagnostics,
            offset,
            span,
            "portable variants cannot name architecture features",
        );
        return;
    }
    let Some(architecture) = key.architecture.target() else {
        return;
    };
    let known = known_features(architecture);
    let mut seen = HashSet::new();
    for feature in &key.features {
        if !seen.insert(feature) {
            push(
                diagnostics,
                offset,
                span,
                format!("duplicate feature {feature} in variant"),
            );
        } else if !known.contains(&feature.as_str()) {
            push(
                diagnostics,
                offset,
                span,
                format!(
                    "unknown {} feature {feature} in variant",
                    key.architecture.name()
                ),
            );
        }
    }
}

fn same_variant(expected: &VariantKey, actual: Option<&VariantKey>) -> bool {
    let Some(actual) = actual else {
        return expected.architecture == VariantArchitecture::Portable
            && expected.features.is_empty();
    };
    expected.architecture == actual.architecture && expected.features == actual.features
}

fn has_arch_variant(variant: Option<&VariantKey>) -> bool {
    variant.is_some_and(|variant| {
        matches!(
            variant.architecture,
            VariantArchitecture::Aarch64 | VariantArchitecture::X64 | VariantArchitecture::Rv64
        )
    })
}

fn intrinsic_architecture(
    instruction: &tir_format::TextInstruction,
) -> Option<VariantArchitecture> {
    let name = instruction
        .tokens
        .iter()
        .find_map(|token| match &token.kind {
            TokenKind::String(value) => Some(value.as_str()),
            _ => None,
        })?;
    if name.starts_with("llvm.aarch64.") {
        Some(VariantArchitecture::Aarch64)
    } else if name.starts_with("llvm.x86.") {
        Some(VariantArchitecture::X64)
    } else if name.starts_with("llvm.riscv.") {
        Some(VariantArchitecture::Rv64)
    } else if name.starts_with("llvm.wasm.") {
        Some(VariantArchitecture::Wasm)
    } else {
        None
    }
}

fn push(diagnostics: &mut Vec<Diagnostic>, offset: usize, span: Span, message: impl Into<String>) {
    diagnostics.push(Diagnostic::error(V18, message).with_primary_span(shift(span, offset)));
}

fn shift(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}
