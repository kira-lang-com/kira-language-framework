use std::collections::{HashMap, HashSet};

use tir_format::text::{Token, TokenKind, lex};
use tir_format::{
    TextItemKind, TextModule, function_uses_c_abi, parse_function_signature, parse_type,
};
use tir_types::{Abi, Convention, FunctionType, ParameterType, Type};

mod calls;
mod generic_calls;
mod imports;

use imports::{parse_import_call_conventions, parse_per_cpu_global, parse_platform_import_symbols};

#[derive(Default)]
pub(crate) struct CallConventions {
    module_path: String,
    direct: HashMap<String, Vec<Option<Convention>>>,
    direct_parameter_types: HashMap<String, Vec<Type>>,
    direct_results: HashMap<String, DirectResult>,
    direct_failures: HashMap<String, bool>,
    direct_c_abi: HashMap<String, bool>,
    imported_functions: HashSet<String>,
    defined_functions: HashMap<String, FunctionType>,
    function_references: HashMap<String, FunctionType>,
    generic_function_parameters: HashMap<String, Vec<String>>,
    platform_imports: HashMap<String, PlatformImport>,
    per_cpu_globals: HashSet<String>,
    trait_methods: HashMap<(String, String), Vec<Option<Convention>>>,
    trait_parameter_types: HashMap<(String, String), Vec<Type>>,
    trait_results: HashMap<(String, String), DirectResult>,
    trait_failures: HashMap<(String, String), bool>,
    witnesses: HashMap<(String, String, String), Vec<Option<Convention>>>,
    witness_parameter_types: HashMap<(String, String, String), Vec<Type>>,
    witness_results: HashMap<(String, String, String), DirectResult>,
    witness_failures: HashMap<(String, String, String), bool>,
    vtables: HashMap<(String, u64), Vec<Option<Convention>>>,
    vtable_parameter_types: HashMap<(String, u64), Vec<Type>>,
    vtable_results: HashMap<(String, u64), DirectResult>,
    vtable_failures: HashMap<(String, u64), bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct DirectResult {
    pub result: Type,
    pub asynchronous: bool,
}

#[derive(Clone, Debug)]
enum PlatformImport {
    Function(FunctionType),
    Data,
}

pub(crate) fn callable_conventions(module: &TextModule) -> CallConventions {
    let mut callables = CallConventions {
        module_path: module.header.path.clone(),
        ..CallConventions::default()
    };
    for item in &module.items {
        let source = module.item_text(*item);
        let Ok(tokens) = lex(source) else {
            continue;
        };
        match item.kind {
            TextItemKind::Function | TextItemKind::Entry => {
                let Some((symbol, symbol_end, _)) =
                    tokens.iter().enumerate().find_map(|(index, token)| {
                        is_punctuation(token, '@')
                            .then(|| crate::variant_text::read_symbol(&tokens, index))
                            .flatten()
                    })
                else {
                    continue;
                };
                if let Ok(Some(signature)) = parse_function_signature(source) {
                    let fails = signature.failure.is_some();
                    let header_end = tokens
                        .iter()
                        .position(|token| is_punctuation(token, '{'))
                        .unwrap_or(tokens.len());
                    let asynchronous = tokens[..header_end]
                        .iter()
                        .any(|token| atom(token) == Some("async"));
                    let function_type = FunctionType {
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
                        abi: if function_uses_c_abi(source) {
                            Abi::C
                        } else {
                            Abi::Ridl
                        },
                        foreign: false,
                    };
                    callables
                        .direct_c_abi
                        .insert(symbol.clone(), function_type.abi == Abi::C);
                    if item.kind == TextItemKind::Function {
                        callables
                            .defined_functions
                            .insert(symbol.clone(), function_type.clone());
                        callables
                            .function_references
                            .insert(symbol.clone(), function_type);
                        let generic_parameters =
                            generic_calls::parameter_names(&tokens, symbol_end);
                        if !generic_parameters.is_empty() {
                            callables
                                .generic_function_parameters
                                .insert(symbol.clone(), generic_parameters);
                        }
                    }
                    callables.direct_results.insert(
                        symbol.clone(),
                        DirectResult {
                            result: signature.result.clone(),
                            asynchronous,
                        },
                    );
                    let parameter_types = signature
                        .parameters
                        .iter()
                        .map(|parameter| parameter.ty.clone())
                        .collect();
                    callables.direct.insert(
                        symbol.clone(),
                        signature
                            .parameters
                            .into_iter()
                            .map(|parameter| parameter.convention)
                            .collect(),
                    );
                    callables
                        .direct_parameter_types
                        .insert(symbol.clone(), parameter_types);
                    callables.direct_failures.insert(symbol, fails);
                }
            }
            TextItemKind::Import => {
                parse_platform_import_symbols(source, &tokens, &mut callables.platform_imports);
                parse_import_call_conventions(
                    source,
                    &tokens,
                    &mut callables.direct,
                    &mut callables.direct_parameter_types,
                    &mut callables.direct_failures,
                    &mut callables.direct_results,
                    &mut callables.direct_c_abi,
                    &mut callables.imported_functions,
                    &mut callables.function_references,
                    &mut callables.generic_function_parameters,
                );
            }
            TextItemKind::Global => {
                parse_per_cpu_global(&tokens, &mut callables.per_cpu_globals);
            }
            _ => {}
        }
    }
    for item in &module.items {
        let source = module.item_text(*item);
        match item.kind {
            TextItemKind::Trait => parse_trait_call_conventions(
                source,
                &callables.module_path,
                &mut callables.trait_methods,
                &mut callables.trait_parameter_types,
                &mut callables.trait_results,
                &mut callables.trait_failures,
            ),
            TextItemKind::Conformance => parse_witness_call_conventions(
                source,
                &callables.module_path,
                &callables.direct,
                &callables.direct_parameter_types,
                &callables.direct_results,
                &callables.direct_failures,
                &callables.trait_methods,
                &callables.trait_parameter_types,
                &callables.trait_results,
                &callables.trait_failures,
                &mut callables.witnesses,
                &mut callables.witness_parameter_types,
                &mut callables.witness_results,
                &mut callables.witness_failures,
            ),
            TextItemKind::VTable => parse_vtable_call_conventions(
                source,
                &callables.direct,
                &callables.direct_parameter_types,
                &callables.direct_results,
                &callables.direct_failures,
                &mut callables.vtables,
                &mut callables.vtable_parameter_types,
                &mut callables.vtable_results,
                &mut callables.vtable_failures,
            ),
            _ => {}
        }
    }
    callables
}

fn parse_trait_call_conventions(
    source: &str,
    module_path: &str,
    traits: &mut HashMap<(String, String), Vec<Option<Convention>>>,
    parameter_types: &mut HashMap<(String, String), Vec<Type>>,
    results: &mut HashMap<(String, String), DirectResult>,
    failures: &mut HashMap<(String, String), bool>,
) {
    let Ok(tokens) = lex(source) else {
        return;
    };
    let Some(start) = tokens.iter().position(|token| atom(token) == Some("trait")) else {
        return;
    };
    let Some((path, path_end)) = read_path(&tokens, start + 1, true) else {
        return;
    };
    let trait_path = qualify_path(&path, module_path);
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(path_end)
        .find_map(|(index, token)| is_punctuation(token, '{').then_some(index))
    else {
        return;
    };
    let Some(close) = matching_brace(&tokens, open) else {
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
        if let Some((conventions, types, result, fails)) =
            type_call_info(source, &tokens, index + 3, end)
        {
            let key = (trait_path.clone(), name.to_owned());
            traits.insert(key.clone(), conventions);
            parameter_types.insert(key.clone(), types);
            results.insert(key.clone(), result);
            failures.insert(key, fails);
        }
        index = end.max(index + 3);
    }
}

fn parse_witness_call_conventions(
    source: &str,
    module_path: &str,
    direct: &HashMap<String, Vec<Option<Convention>>>,
    direct_parameter_types: &HashMap<String, Vec<Type>>,
    direct_results: &HashMap<String, DirectResult>,
    direct_failures: &HashMap<String, bool>,
    trait_methods: &HashMap<(String, String), Vec<Option<Convention>>>,
    trait_parameter_types: &HashMap<(String, String), Vec<Type>>,
    trait_results: &HashMap<(String, String), DirectResult>,
    trait_failures: &HashMap<(String, String), bool>,
    witnesses: &mut HashMap<(String, String, String), Vec<Option<Convention>>>,
    witness_parameter_types: &mut HashMap<(String, String, String), Vec<Type>>,
    witness_results: &mut HashMap<(String, String, String), DirectResult>,
    witness_failures: &mut HashMap<(String, String, String), bool>,
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
    let Some(colon) = top_level_colon(&tokens, start + 1) else {
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
    let Ok(ty) = ty.canonical_text_in(module_path, &HashMap::new()) else {
        return;
    };
    let Some((trait_name, trait_end)) = read_path(&tokens, colon + 1, true) else {
        return;
    };
    let trait_path = qualify_path(&trait_name, module_path);
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(trait_end)
        .find_map(|(index, token)| is_punctuation(token, '{').then_some(index))
    else {
        return;
    };
    let Some(close) = matching_brace(&tokens, open) else {
        return;
    };
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
        let conventions = direct
            .get(&target)
            .or_else(|| trait_methods.get(&(trait_path.clone(), method.to_owned())))
            .cloned();
        if let Some(conventions) = conventions {
            let key = (ty.clone(), trait_path.clone(), method.to_owned());
            let parameter_types = direct_parameter_types
                .get(&target)
                .or_else(|| trait_parameter_types.get(&(trait_path.clone(), method.to_owned())))
                .cloned();
            let result = direct_results
                .get(&target)
                .or_else(|| trait_results.get(&(trait_path.clone(), method.to_owned())))
                .cloned();
            let fails = direct_failures
                .get(&target)
                .or_else(|| trait_failures.get(&(trait_path.clone(), method.to_owned())))
                .copied()
                .unwrap_or(false);
            witnesses.insert(key.clone(), conventions);
            if let Some(parameter_types) = parameter_types {
                witness_parameter_types.insert(key.clone(), parameter_types);
            }
            if let Some(result) = result {
                witness_results.insert(key.clone(), result);
            }
            witness_failures.insert(key, fails);
        }
        index = next;
    }
}

fn parse_vtable_call_conventions(
    source: &str,
    direct: &HashMap<String, Vec<Option<Convention>>>,
    direct_parameter_types: &HashMap<String, Vec<Type>>,
    direct_results: &HashMap<String, DirectResult>,
    direct_failures: &HashMap<String, bool>,
    vtables: &mut HashMap<(String, u64), Vec<Option<Convention>>>,
    vtable_parameter_types: &mut HashMap<(String, u64), Vec<Type>>,
    vtable_results: &mut HashMap<(String, u64), DirectResult>,
    vtable_failures: &mut HashMap<(String, u64), bool>,
) {
    let Ok(tokens) = lex(source) else {
        return;
    };
    let Some(start) = tokens
        .iter()
        .position(|token| atom(token) == Some("vtable"))
    else {
        return;
    };
    let Some((name, name_end, _)) = crate::variant_text::read_symbol(&tokens, start + 1) else {
        return;
    };
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(name_end)
        .find_map(|(index, token)| is_punctuation(token, '{').then_some(index))
    else {
        return;
    };
    let Some(close) = matching_brace(&tokens, open) else {
        return;
    };
    let mut index = open + 1;
    while index + 2 < close {
        let Some(slot) = atom(&tokens[index]).and_then(parse_u64) else {
            index += 1;
            continue;
        };
        if !is_punctuation(&tokens[index + 1], ':') || !is_punctuation(&tokens[index + 2], '@') {
            index += 1;
            continue;
        }
        let Some((target, next, _)) = crate::variant_text::read_symbol(&tokens, index + 2) else {
            index += 1;
            continue;
        };
        if let Some(conventions) = direct.get(&target) {
            let key = (name.clone(), slot);
            vtables.insert(key.clone(), conventions.clone());
            if let Some(parameter_types) = direct_parameter_types.get(&target) {
                vtable_parameter_types.insert(key.clone(), parameter_types.clone());
            }
            if let Some(result) = direct_results.get(&target) {
                vtable_results.insert(key.clone(), result.clone());
            }
            vtable_failures.insert(key, direct_failures.get(&target).copied().unwrap_or(false));
        }
        index = next;
    }
}

fn type_call_info(
    source: &str,
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<(Vec<Option<Convention>>, Vec<Type>, DirectResult, bool)> {
    let (first, last) = (tokens.get(start)?, tokens.get(end.checked_sub(1)?)?);
    let source = source.get(first.span.start..last.span.end)?;
    let Type::Function(function) = parse_type(source).ok()? else {
        return None;
    };
    let fails = function.failure.is_some();
    let result = DirectResult {
        result: function.result.as_ref().clone(),
        asynchronous: function.asynchronous,
    };
    let parameter_types = function
        .parameters
        .iter()
        .map(|parameter| parameter.ty.clone())
        .collect();
    let conventions = function
        .parameters
        .into_iter()
        .map(|parameter| parameter.convention)
        .collect();
    Some((conventions, parameter_types, result, fails))
}

fn witness_call_operands(
    instruction: &tir_format::TextInstruction,
    source: &str,
    module_path: &str,
) -> Option<(Option<String>, String, String, usize)> {
    let tokens = &instruction.tokens;
    let start = crate::ssa::text_instruction_operands_start(tokens);
    let comma = top_level_comma(tokens, start)?;
    let type_text =
        source.get(tokens.get(start)?.span.start..tokens.get(comma.checked_sub(1)?)?.span.end)?;
    let ty = parse_type(type_text.trim())
        .ok()
        .and_then(|ty| ty.canonical_text_in(module_path, &HashMap::new()).ok());
    let (path, path_end) = read_path(tokens, comma + 1, true)?;
    if !is_punctuation(tokens.get(path_end)?, '(') {
        return None;
    }
    let (trait_name, method) = path.rsplit_once('.')?;
    let trait_path = qualify_path(trait_name, module_path);
    Some((ty, trait_path, method.to_owned(), path_end))
}

fn vtable_call_operands(
    instruction: &tir_format::TextInstruction,
) -> Option<(String, u64, String, usize)> {
    let tokens = &instruction.tokens;
    let start = crate::ssa::text_instruction_operands_start(tokens);
    let (symbol, symbol_end, _) =
        tokens
            .iter()
            .enumerate()
            .skip(start)
            .find_map(|(index, token)| {
                is_punctuation(token, '@')
                    .then(|| crate::variant_text::read_symbol(tokens, index))
                    .flatten()
            })?;
    let receiver = value_name_in(tokens, start, symbol_end)?;
    let mut cursor = symbol_end;
    if tokens
        .get(cursor)
        .is_some_and(|token| is_punctuation(token, ','))
    {
        cursor += 1;
    }
    let slot = atom(tokens.get(cursor)?).and_then(parse_u64)?;
    cursor += 1;
    let open = (cursor..tokens.len()).find(|index| is_punctuation(&tokens[*index], '('))?;
    Some((symbol, slot, receiver, open))
}

fn value_name_in(tokens: &[Token], start: usize, end: usize) -> Option<String> {
    (start..end.saturating_sub(1)).find_map(|index| {
        (is_punctuation(&tokens[index], '%'))
            .then(|| atom(tokens.get(index + 1)?).map(str::to_owned))
            .flatten()
    })
}

fn argument_names_at(tokens: &[Token], open: usize) -> Vec<String> {
    let mut depth = 0_i32;
    let Some(close) = (open..tokens.len()).find(|index| {
        if is_punctuation(&tokens[*index], '(') {
            depth += 1;
        } else if is_punctuation(&tokens[*index], ')') {
            depth -= 1;
            if depth == 0 {
                return true;
            }
        }
        false
    }) else {
        return Vec::new();
    };
    tokens[open + 1..close]
        .windows(2)
        .filter_map(|pair| {
            (is_punctuation(&pair[0], '%'))
                .then(|| atom(&pair[1]).map(str::to_owned))
                .flatten()
        })
        .collect()
}

fn top_level_comma(tokens: &[Token], start: usize) -> Option<usize> {
    let (mut angle, mut paren, mut bracket) = (0_i32, 0_i32, 0_i32);
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            TokenKind::Punctuation('<') => angle += 1,
            TokenKind::Punctuation('>') => angle -= 1,
            TokenKind::Punctuation('(') => paren += 1,
            TokenKind::Punctuation(')') => paren -= 1,
            TokenKind::Punctuation('[') => bracket += 1,
            TokenKind::Punctuation(']') => bracket -= 1,
            TokenKind::Punctuation(',') if angle == 0 && paren == 0 && bracket == 0 => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

fn top_level_colon(tokens: &[Token], start: usize) -> Option<usize> {
    let (mut angle, mut paren, mut bracket) = (0_i32, 0_i32, 0_i32);
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            TokenKind::Punctuation('<') => angle += 1,
            TokenKind::Punctuation('>') => angle -= 1,
            TokenKind::Punctuation('(') => paren += 1,
            TokenKind::Punctuation(')') => paren -= 1,
            TokenKind::Punctuation('[') => bracket += 1,
            TokenKind::Punctuation(']') => bracket -= 1,
            TokenKind::Punctuation(':') if angle == 0 && paren == 0 && bracket == 0 => {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

fn read_path(tokens: &[Token], start: usize, allow_percent: bool) -> Option<(String, usize)> {
    let mut index = start;
    if allow_percent
        && tokens
            .get(index)
            .is_some_and(|token| is_punctuation(token, '%'))
    {
        index += 1;
    }
    let mut path = atom(tokens.get(index)?)?.to_owned();
    index += 1;
    while tokens
        .get(index)
        .is_some_and(|token| is_punctuation(token, '.'))
    {
        path.push('.');
        path.push_str(atom(tokens.get(index + 1)?)?);
        index += 2;
    }
    Some((path, index))
}

fn matching_brace(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0_i32;
    (open..tokens.len()).find(|index| {
        if is_punctuation(&tokens[*index], '{') {
            depth += 1;
        } else if is_punctuation(&tokens[*index], '}') {
            depth -= 1;
            if depth == 0 {
                return true;
            }
        }
        false
    })
}

fn qualify_path(path: &str, module_path: &str) -> String {
    if path.contains('.') || module_path.is_empty() {
        path.to_owned()
    } else {
        format!("{module_path}.{path}")
    }
}

fn parse_u64(text: &str) -> Option<u64> {
    u64::try_from(tir_format::numeric_literals::parse_signed_integer(text)?).ok()
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
