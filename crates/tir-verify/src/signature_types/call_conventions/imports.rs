use super::*;

pub(super) fn parse_per_cpu_global(tokens: &[Token], globals: &mut HashSet<String>) {
    let per_cpu = tokens.iter().enumerate().any(|(index, token)| {
        let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
        atom(token) == Some("percpu")
            && !previous
                .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('.' | '%' | '@')))
            && !tokens
                .get(index + 1)
                .is_some_and(|token| is_punctuation(token, '.'))
    });
    if !per_cpu {
        return;
    }
    let Some(symbol) = tokens.iter().enumerate().find_map(|(index, token)| {
        is_punctuation(token, '@')
            .then(|| crate::variant_text::read_symbol(tokens, index))
            .flatten()
            .map(|(symbol, _, _)| symbol)
    }) else {
        return;
    };
    globals.insert(symbol);
}

pub(super) fn parse_platform_import_symbols(
    source: &str,
    tokens: &[Token],
    imports: &mut HashMap<String, PlatformImport>,
) {
    if tokens.get(1).and_then(atom) != Some("platform") {
        return;
    }
    for (index, token) in tokens.iter().enumerate() {
        let Some(kind) = atom(token) else {
            continue;
        };
        if !matches!(kind, "func" | "data") {
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
        let Some(type_text) = source.get(first.span.start..last.span.end) else {
            continue;
        };
        let Ok(ty) = parse_type(type_text) else {
            continue;
        };
        let import = match (kind, ty) {
            ("func", Type::Function(function)) => PlatformImport::Function(function),
            ("func", _) => continue,
            ("data", _) => PlatformImport::Data,
            _ => continue,
        };
        imports.insert(symbol, import);
    }
}

pub(super) fn parse_import_call_conventions(
    source: &str,
    tokens: &[Token],
    callables: &mut HashMap<String, Vec<Option<Convention>>>,
    parameter_types: &mut HashMap<String, Vec<Type>>,
    failures: &mut HashMap<String, bool>,
    results: &mut HashMap<String, DirectResult>,
    c_abi: &mut HashMap<String, bool>,
    imported_functions: &mut HashSet<String>,
    function_references: &mut HashMap<String, FunctionType>,
    generic_function_parameters: &mut HashMap<String, Vec<String>>,
) {
    let platform_imports = tokens.get(1).and_then(atom) == Some("platform");
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
            if !platform_imports {
                function_references.insert(symbol.clone(), function.clone());
                let parameters = generic_parameter_order(&function.to_string());
                if !parameters.is_empty() {
                    generic_function_parameters.insert(symbol.clone(), parameters);
                }
            }
            c_abi.insert(symbol.clone(), function.abi == Abi::C);
            imported_functions.insert(symbol.clone());
            failures.insert(symbol.clone(), function.failure.is_some());
            results.insert(
                symbol.clone(),
                DirectResult {
                    result: function.result.as_ref().clone(),
                    asynchronous: function.asynchronous,
                },
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

fn generic_parameter_order(function_type: &str) -> Vec<String> {
    let Ok(tokens) = lex(function_type) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if !is_punctuation(token, '$') {
            continue;
        }
        let Some(name) = tokens.get(index + 1).and_then(atom) else {
            continue;
        };
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names
}
