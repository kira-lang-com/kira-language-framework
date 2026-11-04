use std::collections::HashMap;

use tir_format::text::{Token, TokenKind};
use tir_format::{TextInstruction, parse_type};
use tir_types::{FunctionType, Type};

pub(super) fn parameter_names(tokens: &[Token], start: usize) -> Vec<String> {
    if !is_punctuation(tokens.get(start), '<') {
        return Vec::new();
    }
    let Some(end) = matching_angle(tokens, start) else {
        return Vec::new();
    };
    split_ranges(tokens, start + 1, end)
        .into_iter()
        .filter_map(|(first, last)| {
            (first..last).find_map(|index| {
                is_punctuation(tokens.get(index), '$')
                    .then(|| atom(tokens.get(index + 1)).map(str::to_owned))
                    .flatten()
            })
        })
        .collect()
}

pub(super) fn call_bindings(
    source: &str,
    instruction: &TextInstruction,
    parameters: &[String],
) -> Option<HashMap<String, Type>> {
    if parameters.is_empty() {
        return None;
    }
    let tokens = &instruction.tokens;
    let symbol_end = tokens.iter().enumerate().find_map(|(index, token)| {
        is_punctuation(Some(token), '@')
            .then(|| crate::variant_text::read_symbol(tokens, index).map(|(_, end, _)| end))
            .flatten()
    })?;
    if !is_punctuation(tokens.get(symbol_end), '<') {
        return None;
    }
    let end = matching_angle(tokens, symbol_end)?;
    let arguments = split_ranges(tokens, symbol_end + 1, end);
    if arguments.len() != parameters.len() {
        return None;
    }
    arguments
        .into_iter()
        .zip(parameters)
        .map(|((first, last), parameter)| {
            let first = tokens.get(first)?;
            let last = tokens.get(last.checked_sub(1)?)?;
            let text = source.get(first.span.start..last.span.end)?;
            Some((parameter.clone(), parse_type(text).ok()?))
        })
        .collect()
}

pub(super) fn infer_call_bindings(
    instruction: &TextInstruction,
    parameter_types: &[Type],
    parameters: &[String],
    value_types: &HashMap<String, Type>,
) -> Option<HashMap<String, Type>> {
    if parameters.is_empty() {
        return Some(HashMap::new());
    }
    let tokens = &instruction.tokens;
    let symbol_end = tokens.iter().enumerate().find_map(|(index, token)| {
        is_punctuation(Some(token), '@')
            .then(|| crate::variant_text::read_symbol(tokens, index).map(|(_, end, _)| end))
            .flatten()
    })?;
    let open_start = if is_punctuation(tokens.get(symbol_end), '<') {
        matching_angle(tokens, symbol_end)?.checked_add(1)?
    } else {
        symbol_end
    };
    let open = (open_start..tokens.len()).find(|index| is_punctuation(tokens.get(*index), '('))?;
    let arguments = super::argument_names_at(tokens, open);
    let mut bindings = HashMap::new();
    for (pattern, argument) in parameter_types.iter().zip(arguments) {
        let actual = value_types.get(&argument)?;
        if !unify_type(pattern, actual, parameters, &mut bindings) {
            return None;
        }
    }
    parameters
        .iter()
        .all(|parameter| bindings.contains_key(parameter))
        .then_some(bindings)
}

fn unify_type(
    pattern: &Type,
    actual: &Type,
    parameters: &[String],
    bindings: &mut HashMap<String, Type>,
) -> bool {
    if let Type::Generic(name) = pattern
        && parameters.iter().any(|parameter| parameter == name)
    {
        return match bindings.get(name) {
            Some(bound) => bound == actual,
            None => {
                bindings.insert(name.clone(), actual.clone());
                true
            }
        };
    }

    match (pattern, actual) {
        (
            Type::FixedVector {
                lanes: pattern_lanes,
                element: pattern_element,
            },
            Type::FixedVector {
                lanes: actual_lanes,
                element: actual_element,
            },
        ) => {
            pattern_lanes == actual_lanes
                && unify_type(pattern_element, actual_element, parameters, bindings)
        }
        (Type::ScalableVector(pattern), Type::ScalableVector(actual))
        | (Type::Reference(pattern), Type::Reference(actual))
        | (Type::Unique(pattern), Type::Unique(actual))
        | (Type::Weak(pattern), Type::Weak(actual)) => {
            unify_type(pattern, actual, parameters, bindings)
        }
        (Type::Tuple(patterns), Type::Tuple(actuals)) => {
            unify_types(patterns, actuals, parameters, bindings)
        }
        (
            Type::Array {
                count: pattern_count,
                element: pattern_element,
            },
            Type::Array {
                count: actual_count,
                element: actual_element,
            },
        ) => {
            pattern_count == actual_count
                && unify_type(pattern_element, actual_element, parameters, bindings)
        }
        (
            Type::Nominal {
                path: pattern_path,
                arguments: pattern_arguments,
            },
            Type::Nominal {
                path: actual_path,
                arguments: actual_arguments,
            },
        ) => {
            pattern_path == actual_path
                && unify_types(pattern_arguments, actual_arguments, parameters, bindings)
        }
        (
            Type::Address {
                pointee: pattern,
                space: pattern_space,
            },
            Type::Address {
                pointee: actual,
                space: actual_space,
            },
        ) => pattern_space == actual_space && unify_type(pattern, actual, parameters, bindings),
        (Type::Function(pattern), Type::Function(actual)) => {
            unify_function(pattern, actual, parameters, bindings)
        }
        (Type::Closure(pattern), Type::Closure(actual)) => {
            pattern.borrowed == actual.borrowed
                && unify_function(&pattern.signature, &actual.signature, parameters, bindings)
        }
        (
            Type::Coroutine {
                yield_type: pattern_yield,
                resume_type: pattern_resume,
                result_type: pattern_result,
            },
            Type::Coroutine {
                yield_type: actual_yield,
                resume_type: actual_resume,
                result_type: actual_result,
            },
        ) => {
            unify_type(pattern_yield, actual_yield, parameters, bindings)
                && unify_type(pattern_resume, actual_resume, parameters, bindings)
                && unify_type(pattern_result, actual_result, parameters, bindings)
        }
        (
            Type::Texture {
                dimension: pattern_dimension,
                component: pattern_component,
                access: pattern_access,
                multisampled: pattern_multisampled,
            },
            Type::Texture {
                dimension: actual_dimension,
                component: actual_component,
                access: actual_access,
                multisampled: actual_multisampled,
            },
        ) => {
            pattern_dimension == actual_dimension
                && pattern_access == actual_access
                && pattern_multisampled == actual_multisampled
                && unify_type(pattern_component, actual_component, parameters, bindings)
        }
        (Type::VisibleFunctionTable(pattern), Type::VisibleFunctionTable(actual)) => {
            unify_function(pattern, actual, parameters, bindings)
        }
        (
            Type::Tensor {
                count: pattern_count,
                element: pattern_element,
            },
            Type::Tensor {
                count: actual_count,
                element: actual_element,
            },
        ) => {
            pattern_count == actual_count
                && unify_type(pattern_element, actual_element, parameters, bindings)
        }
        (
            Type::CooperativeMatrix {
                scope: pattern_scope,
                rows: pattern_rows,
                columns: pattern_columns,
                element: pattern_element,
                use_kind: pattern_use,
            },
            Type::CooperativeMatrix {
                scope: actual_scope,
                rows: actual_rows,
                columns: actual_columns,
                element: actual_element,
                use_kind: actual_use,
            },
        ) => {
            pattern_scope == actual_scope
                && pattern_rows == actual_rows
                && pattern_columns == actual_columns
                && pattern_use == actual_use
                && unify_type(pattern_element, actual_element, parameters, bindings)
        }
        (
            Type::Entry {
                kind: pattern_kind,
                root: pattern_root,
                immediate: pattern_immediate,
            },
            Type::Entry {
                kind: actual_kind,
                root: actual_root,
                immediate: actual_immediate,
            },
        ) => {
            pattern_kind == actual_kind
                && unify_type(pattern_root, actual_root, parameters, bindings)
                && unify_type(pattern_immediate, actual_immediate, parameters, bindings)
        }
        _ => pattern == actual,
    }
}

fn unify_types(
    patterns: &[Type],
    actuals: &[Type],
    parameters: &[String],
    bindings: &mut HashMap<String, Type>,
) -> bool {
    patterns.len() == actuals.len()
        && patterns
            .iter()
            .zip(actuals)
            .all(|(pattern, actual)| unify_type(pattern, actual, parameters, bindings))
}

fn unify_function(
    pattern: &FunctionType,
    actual: &FunctionType,
    parameters: &[String],
    bindings: &mut HashMap<String, Type>,
) -> bool {
    if pattern.parameters.len() != actual.parameters.len()
        || pattern.asynchronous != actual.asynchronous
        || pattern.abi != actual.abi
        || pattern.foreign != actual.foreign
    {
        return false;
    }
    for (pattern, actual) in pattern.parameters.iter().zip(&actual.parameters) {
        if pattern.context != actual.context
            || pattern.convention != actual.convention
            || !unify_type(&pattern.ty, &actual.ty, parameters, bindings)
        {
            return false;
        }
    }
    let result_matches = unify_type(&pattern.result, &actual.result, parameters, bindings);
    match (&pattern.failure, &actual.failure) {
        (None, None) => result_matches,
        (Some(pattern), Some(actual)) => {
            result_matches && unify_type(pattern, actual, parameters, bindings)
        }
        _ => false,
    }
}

pub(super) fn explicit_type_argument_count(instruction: &TextInstruction) -> Option<usize> {
    let tokens = &instruction.tokens;
    let symbol_end = tokens.iter().enumerate().find_map(|(index, token)| {
        is_punctuation(Some(token), '@')
            .then(|| crate::variant_text::read_symbol(tokens, index).map(|(_, end, _)| end))
            .flatten()
    })?;
    if !is_punctuation(tokens.get(symbol_end), '<') {
        return None;
    }
    let end = matching_angle(tokens, symbol_end)?;
    Some(split_ranges(tokens, symbol_end + 1, end).len())
}

fn split_ranges(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut range_start = start;
    let (mut angle, mut paren, mut bracket) = (0_i32, 0_i32, 0_i32);
    for index in start..end {
        match tokens[index].kind {
            TokenKind::Punctuation('<') => angle += 1,
            TokenKind::Punctuation('>') => angle -= 1,
            TokenKind::Punctuation('(') => paren += 1,
            TokenKind::Punctuation(')') => paren -= 1,
            TokenKind::Punctuation('[') => bracket += 1,
            TokenKind::Punctuation(']') => bracket -= 1,
            TokenKind::Punctuation(',') if angle == 0 && paren == 0 && bracket == 0 => {
                ranges.push((range_start, index));
                range_start = index + 1;
            }
            _ => {}
        }
    }
    if range_start < end {
        ranges.push((range_start, end));
    }
    ranges
}

fn matching_angle(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    (open..tokens.len()).find(|index| {
        if is_punctuation(tokens.get(*index), '<') {
            depth += 1;
        } else if is_punctuation(tokens.get(*index), '>') {
            depth -= 1;
            if depth == 0 {
                return true;
            }
        }
        false
    })
}

fn atom(token: Option<&Token>) -> Option<&str> {
    match &token?.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn is_punctuation(token: Option<&Token>, expected: char) -> bool {
    matches!(token.map(|token| &token.kind), Some(TokenKind::Punctuation(character)) if *character == expected)
}
