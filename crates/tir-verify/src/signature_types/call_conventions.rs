use std::collections::HashMap;

use tir_diag::Diagnostic;
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{TextItemKind, TextModule, parse_function_signature, parse_type};
use tir_types::{Abi, Convention, FunctionType, ParameterType, Type};

mod calls;
mod generic_calls;
mod trait_witnesses;

#[derive(Default)]
pub(crate) struct CallConventions {
    module_path: String,
    direct: HashMap<String, Vec<Option<Convention>>>,
    direct_parameter_types: HashMap<String, Vec<Type>>,
    direct_signatures: HashMap<String, FunctionType>,
    direct_generic_parameters: HashMap<String, Vec<String>>,
    direct_results: HashMap<String, (usize, Option<Type>)>,
    direct_failures: HashMap<String, bool>,
    trait_methods: HashMap<(String, String), Vec<Option<Convention>>>,
    trait_parameter_types: HashMap<(String, String), Vec<Type>>,
    trait_signatures: HashMap<(String, String), FunctionType>,
    trait_generic_parameters: HashMap<String, Vec<String>>,
    trait_results: HashMap<(String, String), (usize, Option<Type>)>,
    trait_failures: HashMap<(String, String), bool>,
    witnesses: HashMap<(String, String, String), Vec<Option<Convention>>>,
    witness_parameter_types: HashMap<(String, String, String), Vec<Type>>,
    witness_results: HashMap<(String, String, String), (usize, Option<Type>)>,
    witness_failures: HashMap<(String, String, String), bool>,
    vtables: HashMap<(String, u64), Vec<Option<Convention>>>,
    vtable_parameter_types: HashMap<(String, u64), Vec<Type>>,
    vtable_results: HashMap<(String, u64), (usize, Option<Type>)>,
    vtable_failures: HashMap<(String, u64), bool>,
    validation_diagnostics: Vec<Diagnostic>,
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
                    let parameter_types = signature
                        .parameters
                        .iter()
                        .map(|parameter| parameter.ty.clone())
                        .collect();
                    callables.direct_signatures.insert(
                        symbol.clone(),
                        function_type_from_signature(&signature, &tokens),
                    );
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
                    callables.direct_results.insert(
                        symbol.clone(),
                        call_result_shape(
                            signature.result.clone(),
                            function_is_async(&tokens),
                            Some(&symbol),
                        ),
                    );
                    let generic_parameters = generic_calls::parameter_names(&tokens, symbol_end);
                    if !generic_parameters.is_empty() {
                        callables
                            .direct_generic_parameters
                            .insert(symbol.clone(), generic_parameters);
                    }
                    callables.direct_failures.insert(symbol, fails);
                }
            }
            TextItemKind::Import => parse_import_call_conventions(
                source,
                &tokens,
                &mut callables.direct,
                &mut callables.direct_parameter_types,
                &mut callables.direct_results,
                &mut callables.direct_failures,
                &mut callables.direct_signatures,
            ),
            _ => {}
        }
    }
    let module_path = callables.module_path.clone();
    for item in &module.items {
        let source = module.item_text(*item);
        if item.kind == TextItemKind::Trait {
            trait_witnesses::parse_trait(source, &module_path, &mut callables);
        }
    }
    for item in &module.items {
        let source = module.item_text(*item);
        match item.kind {
            TextItemKind::Conformance => {
                trait_witnesses::parse_conformance(source, &module_path, item.span, &mut callables)
            }
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

impl CallConventions {
    pub(crate) fn validation_diagnostics(&self) -> &[Diagnostic] {
        &self.validation_diagnostics
    }
}

fn parse_import_call_conventions(
    source: &str,
    tokens: &[Token],
    callables: &mut HashMap<String, Vec<Option<Convention>>>,
    parameter_types: &mut HashMap<String, Vec<Type>>,
    results: &mut HashMap<String, (usize, Option<Type>)>,
    failures: &mut HashMap<String, bool>,
    signatures: &mut HashMap<String, FunctionType>,
) {
    for (index, token) in tokens.iter().enumerate() {
        if atom(token) != Some("func") {
            continue;
        }
        let Some(symbol_index) = (index + 1..tokens.len()).find(|cursor| {
            is_punctuation(&tokens[*cursor], '@')
                || matches!(
                    tokens[*cursor].kind,
                    TokenKind::Punctuation('}' | ',') | TokenKind::Newline
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
        let end = crate::signature_types::type_item_end(tokens, colon + 1);
        let (Some(first), Some(last)) = (tokens.get(colon + 1), tokens.get(end.saturating_sub(1)))
        else {
            continue;
        };
        let Some(text) = source.get(first.span.start..last.span.end) else {
            continue;
        };
        if let Ok(Type::Function(function)) = parse_type(text) {
            signatures.insert(symbol.clone(), function.clone());
            failures.insert(symbol.clone(), function.failure.is_some());
            results.insert(
                symbol.clone(),
                call_result_shape(
                    *function.result.clone(),
                    function.asynchronous,
                    Some(&symbol),
                ),
            );
            parameter_types.insert(
                symbol.clone(),
                function
                    .parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
            );
            callables.insert(
                symbol,
                function
                    .parameters
                    .into_iter()
                    .map(|parameter| parameter.convention)
                    .collect(),
            );
        }
    }
}

fn parse_vtable_call_conventions(
    source: &str,
    direct: &HashMap<String, Vec<Option<Convention>>>,
    direct_parameter_types: &HashMap<String, Vec<Type>>,
    direct_results: &HashMap<String, (usize, Option<Type>)>,
    direct_failures: &HashMap<String, bool>,
    vtables: &mut HashMap<(String, u64), Vec<Option<Convention>>>,
    vtable_parameter_types: &mut HashMap<(String, u64), Vec<Type>>,
    vtable_results: &mut HashMap<(String, u64), (usize, Option<Type>)>,
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

fn call_result_shape(
    result: Type,
    asynchronous: bool,
    frame: Option<&str>,
) -> (usize, Option<Type>) {
    if asynchronous {
        return (1, frame.map(|symbol| Type::Frame(symbol.to_owned())));
    }
    let count = usize::from(!matches!(result, Type::Unit | Type::Never));
    (count, Some(result))
}

fn function_type_from_signature(
    signature: &tir_format::TextFunctionSignature,
    tokens: &[Token],
) -> FunctionType {
    let parameters = signature
        .parameters
        .iter()
        .map(|parameter| ParameterType {
            context: parameter.context,
            convention: parameter.convention,
            ty: parameter.ty.clone(),
        })
        .collect();
    FunctionType {
        parameters,
        result: Box::new(signature.result.clone()),
        failure: signature.failure.clone().map(Box::new),
        asynchronous: function_is_async(tokens),
        abi: if function_abi_is_c(tokens) {
            Abi::C
        } else {
            Abi::Ridl
        },
        foreign: false,
    }
}

fn function_abi_is_c(tokens: &[Token]) -> bool {
    let header_end = tokens
        .iter()
        .position(|token| is_punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let (mut angle, mut paren, mut bracket) = (0_i32, 0_i32, 0_i32);
    for (index, token) in tokens[..header_end].iter().enumerate() {
        if angle == 0 && paren == 0 && bracket == 0 && atom(token) == Some("abi") {
            return tokens
                .get(index + 1)
                .is_some_and(|token| is_punctuation(token, '('))
                && tokens.get(index + 2).and_then(atom) == Some("c")
                && tokens
                    .get(index + 3)
                    .is_some_and(|token| is_punctuation(token, ')'));
        }
        match token.kind {
            TokenKind::Punctuation('<') => angle += 1,
            TokenKind::Punctuation('>') => angle -= 1,
            TokenKind::Punctuation('(') => paren += 1,
            TokenKind::Punctuation(')') => paren -= 1,
            TokenKind::Punctuation('[') => bracket += 1,
            TokenKind::Punctuation(']') => bracket -= 1,
            _ => {}
        }
    }
    false
}

fn function_is_async(tokens: &[Token]) -> bool {
    let header_end = tokens
        .iter()
        .position(|token| is_punctuation(token, '{'))
        .unwrap_or(tokens.len());
    let (mut angle, mut paren, mut bracket) = (0_i32, 0_i32, 0_i32);
    for token in &tokens[..header_end] {
        if angle == 0 && paren == 0 && bracket == 0 && atom(token) == Some("async") {
            return true;
        }
        match token.kind {
            TokenKind::Punctuation('<') => angle += 1,
            TokenKind::Punctuation('>') => angle -= 1,
            TokenKind::Punctuation('(') => paren += 1,
            TokenKind::Punctuation(')') => paren -= 1,
            TokenKind::Punctuation('[') => bracket += 1,
            TokenKind::Punctuation(']') => bracket -= 1,
            _ => {}
        }
    }
    false
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
