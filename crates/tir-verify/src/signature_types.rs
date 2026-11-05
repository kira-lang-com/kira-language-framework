use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{
    FunctionBody, TextFunctionSignature, TextItemKind, TextModule, parse_function_body,
    parse_function_signature, parse_type,
};
use tir_layout::LayoutEngine;
use tir_types::{FunctionType, Type};

use crate::gpu_profile::policy::TypePolicies;

mod call_conventions;
mod call_types;
pub(crate) use call_conventions::{CallConventions, callable_conventions};

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);
const V10: Code = Code::new(Dialect::Tir, Area::Verifier, 10);
const V19: Code = Code::new(Dialect::Tir, Area::Verifier, 19);

pub(super) fn verify_module_calls(
    module: &TextModule,
    layouts: &LayoutEngine,
    policies: &TypePolicies,
    call_conventions: &CallConventions,
) -> Vec<Diagnostic> {
    let callables = callable_failures(module);
    let mut diagnostics = Vec::new();
    for item in module
        .items
        .iter()
        .filter(|item| item.kind == TextItemKind::Function)
    {
        let source = module.item_text(*item);
        let Ok(Some(body)) = parse_function_body(source) else {
            continue;
        };
        let value_types = crate::text_types::inferred_value_types_with_calls(
            source,
            &body,
            layouts,
            policies,
            call_conventions,
        );
        let Ok(tokens) = lex(source) else {
            continue;
        };
        let header_end = tokens
            .iter()
            .position(|token| is_punctuation(token, '{'))
            .unwrap_or(tokens.len());
        let nounwind = tokens[..header_end]
            .iter()
            .any(|token| atom(token) == Some("nounwind"));
        diagnostics.extend(call_types::verify_function(
            source,
            &body,
            &value_types,
            call_conventions,
            item.span.start,
        ));
        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if instruction.opcode == "call.indirect"
                && let Some((declared, _)) = indirect_call_type(source, instruction)
                && let Some((callee, _)) = crate::ssa::text_value_uses(instruction).first()
                && let Some(actual) = value_types.get(callee)
                && actual != &Type::Function(declared)
            {
                diagnostics.push(
                    Diagnostic::error(
                        V19,
                        "indirect call site's function type does not match the callee value type",
                    )
                    .with_primary_span(shift_span(instruction.span, item.span.start)),
                );
            }
            if instruction.opcode == "invoke" && nounwind {
                diagnostics.push(
                    Diagnostic::error(V10, "invoke is not permitted in a nounwind function")
                        .with_primary_span(shift_span(instruction.span, item.span.start)),
                );
            }
            if !matches!(
                instruction.opcode.as_str(),
                "call"
                    | "call.indirect"
                    | "call.closure"
                    | "call.witness"
                    | "call.vtable"
                    | "call.foreign"
                    | "call.variadic"
                    | "try.call"
                    | "invoke"
            ) {
                continue;
            }
            let has_failure = if instruction.opcode == "call.indirect"
                || (instruction.opcode == "try.call"
                    && indirect_call_type(source, instruction).is_some())
            {
                indirect_call_failure(source, instruction)
            } else if matches!(instruction.opcode.as_str(), "call.witness" | "call.vtable") {
                call_conventions.special_call_failure(instruction, source)
            } else if instruction.opcode == "call.closure" {
                crate::ssa::text_value_uses(instruction)
                    .first()
                    .and_then(|(name, _)| value_types.get(name))
                    .and_then(|ty| match ty {
                        Type::Closure(closure) => Some(closure.signature.failure.is_some()),
                        _ => None,
                    })
            } else {
                instruction
                    .tokens
                    .iter()
                    .enumerate()
                    .find_map(|(index, token)| {
                        is_punctuation(token, '@')
                            .then(|| crate::variant_text::read_symbol(&instruction.tokens, index))
                            .flatten()
                    })
                    .and_then(|(callee, _, _)| callables.get(&callee).copied())
            };
            let Some(has_failure) = has_failure else {
                continue;
            };
            let message = if instruction.opcode == "try.call" && !has_failure {
                Some("try.call requires a callee function type with fails")
            } else if instruction.opcode != "try.call" && has_failure {
                Some("a callee with fails must use try.call")
            } else {
                None
            };
            if let Some(message) = message {
                diagnostics.push(
                    Diagnostic::error(V10, message)
                        .with_primary_span(shift_span(instruction.span, item.span.start)),
                );
            }
        }
    }
    diagnostics
}

fn indirect_call_failure(source: &str, instruction: &tir_format::TextInstruction) -> Option<bool> {
    indirect_call_type(source, instruction).map(|(function, _)| function.failure.is_some())
}

pub(super) fn indirect_call_type(
    source: &str,
    instruction: &tir_format::TextInstruction,
) -> Option<(FunctionType, usize)> {
    let tokens = &instruction.tokens;
    let colon = tokens.iter().position(|token| is_punctuation(token, ':'))?;
    let first = tokens.get(colon + 1)?;
    for (argument_open, token) in tokens.iter().enumerate().skip(colon + 2) {
        if !is_punctuation(token, '(') {
            continue;
        }
        let last = tokens.get(argument_open.checked_sub(1)?)?;
        let text = source.get(first.span.start..last.span.end)?;
        if let Ok(Type::Function(function)) = parse_type(text) {
            return Some((function, argument_open));
        }
    }
    None
}

fn callable_failures(module: &TextModule) -> HashMap<String, bool> {
    let mut callables = HashMap::new();
    for item in &module.items {
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        match item.kind {
            TextItemKind::Function | TextItemKind::Entry => {
                let Some((symbol, _, _)) = tokens.iter().enumerate().find_map(|(index, token)| {
                    is_punctuation(token, '@')
                        .then(|| crate::variant_text::read_symbol(&tokens, index))
                        .flatten()
                }) else {
                    continue;
                };
                let failure = item.kind == TextItemKind::Function
                    && parse_function_signature(source)
                        .ok()
                        .flatten()
                        .is_some_and(|signature| signature.failure.is_some());
                callables.insert(symbol, failure);
            }
            TextItemKind::Import => parse_import_functions(source, &tokens, &mut callables),
            _ => {}
        }
    }
    callables
}

fn parse_import_functions(source: &str, tokens: &[Token], callables: &mut HashMap<String, bool>) {
    for (index, token) in tokens.iter().enumerate() {
        if atom(token) != Some("func") {
            continue;
        }
        let Some(symbol_index) = (index + 1..tokens.len()).find(|cursor| {
            is_punctuation(&tokens[*cursor], '@')
                || matches!(
                    tokens[*cursor].kind,
                    TokenKind::Punctuation('}') | TokenKind::Newline
                )
        }) else {
            continue;
        };
        if !is_punctuation(&tokens[symbol_index], '@') {
            continue;
        }
        let Some((symbol, symbol_end, _)) = crate::variant_text::read_symbol(tokens, symbol_index)
        else {
            continue;
        };
        let Some(colon) = (symbol_end..tokens.len()).find(|cursor| {
            matches!(tokens[*cursor].kind, TokenKind::Punctuation(':'))
                || matches!(tokens[*cursor].kind, TokenKind::Punctuation(',' | '}'))
        }) else {
            continue;
        };
        if !is_punctuation(&tokens[colon], ':') {
            continue;
        }
        let end = type_item_end(tokens, colon + 1);
        let Some(first) = tokens.get(colon + 1) else {
            continue;
        };
        let Some(last) = tokens.get(end.saturating_sub(1)) else {
            continue;
        };
        let Some(text) = source.get(first.span.start..last.span.end) else {
            continue;
        };
        if let Ok(Type::Function(function)) = parse_type(text) {
            callables.insert(symbol, function.failure.is_some());
        }
    }
}

pub(crate) fn type_item_end(tokens: &[Token], start: usize) -> usize {
    let mut depth = [0i32; 4];
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if depth == [0; 4]
            && matches!(
                token.kind,
                TokenKind::Punctuation(',' | '}' | '{') | TokenKind::Newline
            )
        {
            return index;
        }
        match token.kind {
            TokenKind::Punctuation('(') => depth[0] += 1,
            TokenKind::Punctuation(')') => depth[0] -= 1,
            TokenKind::Punctuation('[') => depth[1] += 1,
            TokenKind::Punctuation(']') => depth[1] -= 1,
            TokenKind::Punctuation('<') => depth[2] += 1,
            TokenKind::Punctuation('>') => depth[2] -= 1,
            TokenKind::Punctuation('{') => depth[3] += 1,
            TokenKind::Punctuation('}') => depth[3] -= 1,
            _ => {}
        }
    }
    tokens.len()
}

fn shift_span(span: Span, offset: usize) -> Span {
    Span::new(
        span.start.saturating_add(offset),
        span.end.saturating_add(offset),
    )
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

pub(super) fn verify_function(
    signature: &TextFunctionSignature,
    body: &FunctionBody,
) -> Vec<Diagnostic> {
    let Some(entry) = body.blocks.first() else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    if entry.parameters.len() != signature.parameters.len() {
        diagnostics.push(
            Diagnostic::error(
                V1,
                format!(
                    "entry block has {} parameters; function signature has {}",
                    entry.parameters.len(),
                    signature.parameters.len()
                ),
            )
            .with_primary_span(entry.span),
        );
        return diagnostics;
    }
    for (parameter, block_parameter) in signature.parameters.iter().zip(&entry.parameters) {
        if parameter.ty != block_parameter.ty {
            diagnostics.push(
                Diagnostic::error(
                    V1,
                    format!(
                        "entry parameter %{} has type {}; signature requires {}",
                        block_parameter.name, block_parameter.ty, parameter.ty
                    ),
                )
                .with_primary_span(block_parameter.span),
            );
        }
    }
    diagnostics
}
