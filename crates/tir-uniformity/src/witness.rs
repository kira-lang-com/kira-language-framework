use std::collections::HashMap;

use tir_format::text::{Token, TokenKind, lex};
use tir_format::{TextInstruction, TextItemKind, TextModule, parse_type};

struct Conformance {
    ty: String,
    trait_path: String,
    methods: Vec<(String, String)>,
}

pub(super) struct ConformanceIndex {
    records: Vec<Conformance>,
}

impl ConformanceIndex {
    pub(super) fn from_module(module: &TextModule) -> Self {
        let records = module
            .items
            .iter()
            .filter(|item| item.kind == TextItemKind::Conformance)
            .filter_map(|item| parse_conformance(module.item_text(*item), &module.header.path))
            .collect();
        Self { records }
    }
}

pub(super) fn resolve_target(
    instruction: &TextInstruction,
    function_source: &str,
    module_path: &str,
    conformances: &ConformanceIndex,
) -> Result<String, String> {
    let tokens = &instruction.tokens;
    let start = instruction_operands_start(tokens);
    let comma = top_level_token(tokens, start, ',')
        .ok_or_else(|| "GPU call.witness has no concrete type operand".to_owned())?;
    let Some(type_first) = tokens.get(start) else {
        return Err("GPU call.witness has no concrete type operand".to_owned());
    };
    let Some(type_last) = tokens.get(comma.wrapping_sub(1)) else {
        return Err("GPU call.witness has no concrete type operand".to_owned());
    };
    let type_text = function_source
        .get(type_first.span.start..type_last.span.end)
        .ok_or_else(|| "GPU call.witness has an invalid type operand".to_owned())?;
    if type_text.contains('$') {
        return Err("GPU witness calls must resolve to a concrete type".to_owned());
    }
    let ty = parse_type(type_text)
        .map_err(|_| "GPU call.witness has an invalid concrete type operand".to_owned())?
        .canonical_text_in(module_path, &HashMap::new())
        .map_err(|_| "GPU call.witness has an invalid concrete type operand".to_owned())?;

    let method_start = comma + 1;
    let method_end = tokens[method_start..]
        .iter()
        .position(|token| is_punctuation(token, '('))
        .map(|offset| method_start + offset)
        .ok_or_else(|| "GPU call.witness has no trait method operand".to_owned())?;
    let (method_path, consumed) = read_path(tokens, method_start, true)
        .ok_or_else(|| "GPU call.witness has an invalid trait method operand".to_owned())?;
    if consumed != method_end {
        return Err("GPU call.witness has an invalid trait method operand".to_owned());
    }
    let Some((trait_path, method_name)) = method_path.rsplit_once('.') else {
        return Err("GPU call.witness must name Trait.method".to_owned());
    };
    let trait_path = qualify_path(trait_path, module_path);

    let matching = conformances
        .records
        .iter()
        .filter(|record| record.ty == ty && record.trait_path == trait_path)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Err(format!(
            "GPU call.witness {trait_path}.{method_name} has no concrete conformance to resolve"
        ));
    }
    if matching.len() != 1 {
        return Err(format!(
            "GPU call.witness {trait_path}.{method_name} has ambiguous concrete conformances"
        ));
    }
    let targets = matching[0]
        .methods
        .iter()
        .filter(|(name, _)| name == method_name)
        .map(|(_, target)| target)
        .collect::<Vec<_>>();
    match targets.as_slice() {
        [target] => Ok((**target).clone()),
        [] => Err(format!(
            "GPU call.witness {trait_path}.{method_name} has no witness function"
        )),
        _ => Err(format!(
            "GPU call.witness {trait_path}.{method_name} has ambiguous witness functions"
        )),
    }
}

fn parse_conformance(source: &str, module_path: &str) -> Option<Conformance> {
    let tokens = lex(source).ok()?;
    let tokens = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
        .cloned()
        .collect::<Vec<_>>();
    if atom(tokens.first()?)? != "conformance" {
        return None;
    }
    let colon = top_level_token(&tokens, 1, ':')?;
    let type_first = tokens.get(1)?;
    let type_text = source
        .get(type_first.span.start..tokens[colon].span.start)?
        .trim();
    if type_text.contains('$') {
        return None;
    }
    let ty = parse_type(type_text)
        .ok()?
        .canonical_text_in(module_path, &HashMap::new())
        .ok()?;
    let (trait_path, trait_end) = read_path(&tokens, colon + 1, true)?;
    let trait_path = qualify_path(&trait_path, module_path);
    let open = tokens
        .iter()
        .enumerate()
        .skip(trait_end)
        .find_map(|(index, token)| is_punctuation(token, '{').then_some(index))?;
    let close = matching_brace(&tokens, open)?;
    let mut methods = Vec::new();
    let mut index = open + 1;
    while index + 2 < close {
        if atom(&tokens[index]).is_some()
            && is_punctuation(&tokens[index + 1], '=')
            && is_punctuation(&tokens[index + 2], '@')
            && let Some((target, next)) = read_symbol(&tokens, index + 2)
        {
            methods.push((atom(&tokens[index])?.to_owned(), target));
            index = next;
        } else {
            index += 1;
        }
    }
    Some(Conformance {
        ty,
        trait_path,
        methods,
    })
}

fn top_level_token(tokens: &[Token], start: usize, expected: char) -> Option<usize> {
    let (mut angle, mut paren, mut bracket) = (0_i32, 0_i32, 0_i32);
    for (index, token) in tokens.iter().enumerate().skip(start) {
        match token.kind {
            TokenKind::Punctuation('<') => angle += 1,
            TokenKind::Punctuation('>') => angle -= 1,
            TokenKind::Punctuation('(') => paren += 1,
            TokenKind::Punctuation(')') => paren -= 1,
            TokenKind::Punctuation('[') => bracket += 1,
            TokenKind::Punctuation(']') => bracket -= 1,
            TokenKind::Punctuation(character)
                if character == expected && angle == 0 && paren == 0 && bracket == 0 =>
            {
                return Some(index);
            }
            _ => {}
        }
    }
    None
}

fn instruction_operands_start(tokens: &[Token]) -> usize {
    let mut cursor = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        loop {
            if !matches!(
                tokens.get(cursor).map(|token| &token.kind),
                Some(TokenKind::Punctuation('%'))
            ) || !matches!(
                tokens.get(cursor + 1).map(|token| &token.kind),
                Some(TokenKind::Atom(_))
            ) {
                return 0;
            }
            cursor += 2;
            match tokens.get(cursor).map(|token| &token.kind) {
                Some(TokenKind::Punctuation('=')) => {
                    cursor += 1;
                    break;
                }
                Some(TokenKind::Punctuation(',')) => cursor += 1,
                _ => return 0,
            }
        }
    }
    cursor += 1;
    while matches!(
        tokens.get(cursor).map(|token| &token.kind),
        Some(TokenKind::Punctuation('.'))
    ) && matches!(
        tokens.get(cursor + 1).map(|token| &token.kind),
        Some(TokenKind::Atom(_))
    ) {
        cursor += 2;
    }
    cursor
}

fn read_path(tokens: &[Token], start: usize, allow_percent: bool) -> Option<(String, usize)> {
    let mut index = start;
    if allow_percent && is_punctuation(tokens.get(index)?, '%') {
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

fn read_symbol(tokens: &[Token], start: usize) -> Option<(String, usize)> {
    if !is_punctuation(tokens.get(start)?, '@') {
        return None;
    }
    let mut path = atom(tokens.get(start + 1)?)?.to_owned();
    let mut index = start + 2;
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
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if is_punctuation(token, '{') {
            depth += 1;
        } else if is_punctuation(token, '}') {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn qualify_path(path: &str, module_path: &str) -> String {
    if path.contains('.') || module_path.is_empty() {
        path.to_owned()
    } else {
        format!("{module_path}.{path}")
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
