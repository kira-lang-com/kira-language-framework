use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, TextFunctionSignature, TextItem, TextItemKind, TextModule};
use tir_layout::LayoutEngine;
use tir_types::{AddressSpace, Type};

use super::policy::TypePolicies;
use crate::ssa::text_value_uses;
use crate::text_types::inferred_value_types;
use crate::variant_text::read_symbol;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

pub(super) fn verify(
    source: &str,
    item: TextItem,
    body: &FunctionBody,
    signature: Option<&TextFunctionSignature>,
    module: &TextModule,
    policies: &TypePolicies,
    layouts: &LayoutEngine,
) -> Vec<Diagnostic> {
    let types = inferred_value_types(source, body, layouts, policies);
    let current_is_inlineable =
        item.kind == TextItemKind::Function && has_module_gpu_linkage(source);
    let mut diagnostics = Vec::new();

    if let Some(signature) = signature {
        for parameter in &signature.parameters {
            if is_register_only(&parameter.ty, policies) && !current_is_inlineable {
                diagnostics.push(
                    Diagnostic::error(
                        V15,
                        "register-only GPU values cannot cross a function boundary",
                    )
                    .with_primary_span(parameter.span),
                );
            }
        }
        if is_register_only(&signature.result, policies) && !current_is_inlineable {
            diagnostics.push(
                Diagnostic::error(
                    V15,
                    "register-only GPU values cannot cross a function boundary",
                )
                .with_primary_span(
                    body.blocks
                        .first()
                        .map_or(Span::new(0, 0), |block| block.span),
                ),
            );
        }
    }

    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if instruction.opcode == "stack.alloc"
            && let Some(result) = instruction.results.first()
            && let Some(Type::Address { pointee, space }) = types.get(result)
            && is_register_only(pointee, policies)
            && *space != Some(AddressSpace::Thread)
        {
            error(
                &mut diagnostics,
                instruction.span,
                "register-only GPU values may only use thread-address-space stack slots",
            );
        }

        if matches!(instruction.opcode.as_str(), "store" | "raw.store") {
            let uses = text_value_uses(instruction);
            if let (Some((pointer, _)), Some((value, _))) = (uses.first(), uses.get(1))
                && types
                    .get(value)
                    .is_some_and(|ty| is_register_only(ty, policies))
                && !is_thread_address(types.get(pointer))
            {
                error(
                    &mut diagnostics,
                    instruction.span,
                    "register-only GPU values may only be stored in thread-address-space slots",
                );
            }
        }

        if matches!(
            instruction.opcode.as_str(),
            "call" | "tail.call" | "gpu.call.vft"
        ) {
            let carries_register_only = text_value_uses(instruction).iter().any(|(name, _)| {
                types
                    .get(name)
                    .is_some_and(|ty| is_register_only(ty, policies))
            });
            if carries_register_only
                && direct_callee(instruction)
                    .is_none_or(|symbol| !is_inlineable_callee(module, &symbol))
            {
                error(
                    &mut diagnostics,
                    instruction.span,
                    "register-only GPU values may only cross calls to module-linkage GPU functions",
                );
            }
        }

        if instruction.opcode == "return"
            && text_value_uses(instruction)
                .first()
                .and_then(|(name, _)| types.get(name))
                .is_some_and(|ty| is_register_only(ty, policies))
            && !current_is_inlineable
        {
            error(
                &mut diagnostics,
                instruction.span,
                "register-only GPU values cannot be returned across a function boundary",
            );
        }
    }
    diagnostics
}

fn is_register_only(ty: &Type, policies: &TypePolicies) -> bool {
    policies.any_nested_type(ty, |nested| {
        matches!(
            nested,
            Type::RayQuery | Type::HitObject | Type::CooperativeMatrix { .. }
        )
    })
}

fn is_thread_address(ty: Option<&Type>) -> bool {
    matches!(
        ty,
        Some(Type::Address {
            space: Some(AddressSpace::Thread),
            ..
        }) | Some(Type::Pointer(AddressSpace::Thread))
    )
}

fn direct_callee(instruction: &tir_format::TextInstruction) -> Option<String> {
    instruction
        .tokens
        .iter()
        .enumerate()
        .find_map(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('@'))
                .then(|| read_symbol(&instruction.tokens, index).map(|(symbol, _, _)| symbol))
                .flatten()
        })
}

fn is_inlineable_callee(module: &TextModule, symbol: &str) -> bool {
    module.items.iter().any(|item| {
        if item.kind != TextItemKind::Function {
            return false;
        }
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            return false;
        };
        let matches_symbol = tokens.iter().enumerate().any(|(index, token)| {
            matches!(token.kind, TokenKind::Punctuation('@'))
                && read_symbol(&tokens, index).is_some_and(|(name, _, _)| name == symbol)
        });
        matches_symbol && has_module_gpu_linkage_tokens(&tokens)
    })
}

fn has_module_gpu_linkage(source: &str) -> bool {
    lex(source)
        .ok()
        .is_some_and(|tokens| has_module_gpu_linkage_tokens(&tokens))
}

fn has_module_gpu_linkage_tokens(tokens: &[Token]) -> bool {
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    let module_linkage = header
        .iter()
        .any(|token| matches!(&token.kind, TokenKind::Atom(name) if name == "module"));
    let mut in_target = false;
    let gpu_target = header.iter().any(|token| match &token.kind {
        TokenKind::Atom(name) if name == "target" => {
            in_target = true;
            false
        }
        TokenKind::Punctuation(')') => {
            in_target = false;
            false
        }
        TokenKind::Atom(name) if in_target && name == "gpu" => true,
        _ => false,
    });
    module_linkage && gpu_target
}

fn error(diagnostics: &mut Vec<Diagnostic>, span: Span, message: &'static str) {
    diagnostics.push(Diagnostic::error(V15, message).with_primary_span(span));
}
