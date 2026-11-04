use std::collections::HashMap;

use tir_diag::{Diagnostic, Span};
use tir_format::parse_type;
use tir_format::text::{Token, TokenKind, lex};
use tir_types::{FunctionType, Type};

use super::{CallConventions, call_result_shape, generic_calls};

mod witness_signatures;

pub(super) fn parse_trait(source: &str, module_path: &str, callables: &mut CallConventions) {
    let Ok(tokens) = lex(source) else {
        return;
    };
    let Some(start) = tokens.iter().position(|token| atom(token) == Some("trait")) else {
        return;
    };
    let Some((path, path_end)) = super::read_path(&tokens, start + 1, true) else {
        return;
    };
    let trait_path = super::qualify_path(&path, module_path);
    let generic_parameters = generic_calls::parameter_names(&tokens, path_end);
    callables
        .trait_generic_parameters
        .insert(trait_path.clone(), generic_parameters);
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(path_end)
        .find_map(|(index, token)| is_punctuation(token, '{').then_some(index))
    else {
        return;
    };
    let Some(close) = super::matching_brace(&tokens, open) else {
        return;
    };
    let mut index = open + 1;
    while index < close {
        if atom(&tokens[index]) != Some("method") {
            index += 1;
            continue;
        }
        let (Some(name), Some(_colon)) = (
            tokens.get(index + 1).and_then(atom),
            tokens
                .get(index + 2)
                .filter(|token| is_punctuation(token, ':')),
        ) else {
            index += 1;
            continue;
        };
        let end = crate::signature_types::type_item_end(&tokens, index + 3).min(close);
        if let Some(function) = function_type(source, &tokens, index + 3, end) {
            let key = (trait_path.clone(), name.to_owned());
            callables.trait_methods.insert(
                key.clone(),
                function
                    .parameters
                    .iter()
                    .map(|parameter| parameter.convention)
                    .collect(),
            );
            callables.trait_parameter_types.insert(
                key.clone(),
                function
                    .parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
            );
            callables.trait_results.insert(
                key.clone(),
                call_result_shape(*function.result.clone(), function.asynchronous, None),
            );
            callables
                .trait_failures
                .insert(key.clone(), function.failure.is_some());
            callables.trait_signatures.insert(key, function);
        }
        index = end.max(index + 3);
    }
}

pub(super) fn parse_conformance(
    source: &str,
    module_path: &str,
    item_span: Span,
    callables: &mut CallConventions,
) {
    let Ok(tokens) = lex(source) else {
        return;
    };
    let Some(start) = tokens
        .iter()
        .position(|token| atom(token) == Some("conformance"))
    else {
        return;
    };
    let Some(colon) = super::top_level_colon(&tokens, start + 1) else {
        return;
    };
    let (Some(first), Some(last)) = (tokens.get(start + 1), tokens.get(colon.wrapping_sub(1)))
    else {
        return;
    };
    let Some(type_text) = source.get(first.span.start..last.span.end) else {
        return;
    };
    let Ok(ty) = parse_type(type_text.trim()) else {
        return;
    };
    let Ok(ty) = ty.canonical_text_in(module_path, &std::collections::HashMap::new()) else {
        return;
    };
    let Some((trait_name, trait_end)) = super::read_path(&tokens, colon + 1, true) else {
        return;
    };
    let trait_path = super::qualify_path(&trait_name, module_path);
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(trait_end)
        .find_map(|(index, token)| is_punctuation(token, '{').then_some(index))
    else {
        return;
    };
    let Some(close) = super::matching_brace(&tokens, open) else {
        return;
    };
    let generic_parameters = callables
        .trait_generic_parameters
        .get(&trait_path)
        .cloned()
        .unwrap_or_default();
    let mut inferred_substitutions = HashMap::new();
    let mut index = open + 1;
    while index + 2 < close {
        let Some(method) = atom(&tokens[index]) else {
            index += 1;
            continue;
        };
        if !is_punctuation(&tokens[index + 1], '=') || !is_punctuation(&tokens[index + 2], '@') {
            index += 1;
            continue;
        }
        let Some((target, next, _)) = crate::variant_text::read_symbol(&tokens, index + 2) else {
            index += 1;
            continue;
        };
        let trait_method_key = (trait_path.clone(), method.to_owned());
        if let (Some(expected), Some(actual)) = (
            callables.trait_signatures.get(&trait_method_key),
            callables.direct_signatures.get(&target),
        ) {
            let mut candidate_substitutions = inferred_substitutions.clone();
            if witness_signatures::specialize(
                expected,
                actual,
                &generic_parameters,
                &mut candidate_substitutions,
            )
            .is_none()
            {
                let expected =
                    Type::Function(expected.clone()).substitute(&candidate_substitutions);
                callables.validation_diagnostics.push(
                    Diagnostic::error(
                        super::super::V2,
                        format!(
                            "witness function @{target} does not match trait requirement {trait_path}.{method} after substituting trait parameters: expected {expected}, found {actual}"
                        ),
                    )
                    .with_primary_span(item_span),
                );
            } else {
                inferred_substitutions = candidate_substitutions;
            }
        }
        let conventions = callables
            .direct
            .get(&target)
            .or_else(|| callables.trait_methods.get(&trait_method_key))
            .cloned();
        if let Some(conventions) = conventions {
            let key = (ty.clone(), trait_path.clone(), method.to_owned());
            let parameter_types = callables
                .direct_parameter_types
                .get(&target)
                .or_else(|| callables.trait_parameter_types.get(&trait_method_key))
                .cloned();
            let result = callables
                .direct_results
                .get(&target)
                .or_else(|| callables.trait_results.get(&trait_method_key))
                .cloned();
            let fails = callables
                .direct_failures
                .get(&target)
                .or_else(|| callables.trait_failures.get(&trait_method_key))
                .copied()
                .unwrap_or(false);
            callables.witnesses.insert(key.clone(), conventions);
            if let Some(parameter_types) = parameter_types {
                callables
                    .witness_parameter_types
                    .insert(key.clone(), parameter_types);
            }
            if let Some(result) = result {
                callables.witness_results.insert(key.clone(), result);
            }
            callables.witness_failures.insert(key, fails);
        }
        index = next;
    }
}

fn function_type(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<FunctionType> {
    let (first, last) = (tokens.get(start)?, tokens.get(end.checked_sub(1)?)?);
    let source = source.get(first.span.start..last.span.end)?;
    match parse_type(source).ok()? {
        Type::Function(function) => Some(function),
        _ => None,
    }
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
