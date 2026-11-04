use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind, decode_kira_string_literal_contents};

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

#[derive(Clone, Copy, Eq, PartialEq)]
enum OutputKind {
    Node,
    Mesh,
}

pub(super) fn valid_syntax(source: &str, arguments: &[Token]) -> bool {
    parse(source, arguments).is_some()
}

pub(super) fn verify(stage: &str, header: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    let nodes = calls(header, "node");
    if stage == "node" && nodes.len() != 1 {
        diagnostics.push(Diagnostic::error(
            V15,
            "node entry requires exactly one node attribute",
        ));
    }
    if stage != "node" && !nodes.is_empty() {
        diagnostics.push(Diagnostic::error(
            V15,
            format!("node is not valid for {stage} entries"),
        ));
    }
}

fn parse(source: &str, arguments: &[Token]) -> Option<bool> {
    let fields = split(arguments, ',')?;
    if fields.len() != 4 || arguments.last().is_some_and(|token| punct(token, ',')) {
        return None;
    }
    if fields
        .iter()
        .map(|field| atom(&field[0]))
        .collect::<Option<Vec<_>>>()?
        != [
            "launch",
            "max_dispatch_grid",
            "input_record_size",
            "outputs",
        ]
    {
        return None;
    }
    let mut launch = false;
    let mut grid = None;
    let mut input_size = None;
    let mut outputs = None;
    let mut seen = HashSet::new();
    for field in fields {
        if field.len() < 3 || !punct(&field[1], ':') {
            return None;
        }
        let name = atom(&field[0])?;
        if !seen.insert(name) {
            return None;
        }
        let value = &field[2..];
        match name {
            "launch" if value.len() == 1 => {
                let mode = atom(&value[0])?;
                if !matches!(mode, "broadcast" | "coalescing" | "thread") {
                    return None;
                }
                launch = true;
            }
            "max_dispatch_grid" => {
                let dimensions = array_items(value)?;
                if dimensions.len() != 3
                    || dimensions
                        .iter()
                        .any(|dimension| positive_integer(source, dimension).is_none())
                {
                    return None;
                }
                grid = Some(());
            }
            "input_record_size" => {
                if unsigned_integer(source, value).is_none() {
                    return None;
                }
                input_size = Some(());
            }
            "outputs" => outputs = Some(parse_outputs(source, value)?),
            _ => return None,
        }
    }
    if !launch {
        return None;
    }
    grid?;
    input_size?;
    outputs
}

fn parse_outputs(source: &str, value: &[Token]) -> Option<bool> {
    let items = array_items(value)?;
    let mut has_mesh = false;
    let mut names = HashSet::new();
    for item in items {
        let fields = object_fields(item)?;
        if fields.len() != 4 {
            return None;
        }
        if fields
            .iter()
            .map(|field| atom(&field[0]))
            .collect::<Option<Vec<_>>>()?
            != ["name", "kind", "record_size", "max_records"]
        {
            return None;
        }
        let mut name = None;
        let mut kind = None;
        let mut size = None;
        let mut maximum = None;
        let mut seen = HashSet::new();
        for field in fields {
            if field.len() < 3 || !punct(&field[1], ':') {
                return None;
            }
            let field_name = atom(&field[0])?;
            if !seen.insert(field_name) {
                return None;
            }
            let value = &field[2..];
            match field_name {
                "name" if value.len() == 1 => {
                    let TokenKind::String(raw) = &value[0].kind else {
                        return None;
                    };
                    let decoded = decode_kira_string_literal_contents(raw).ok()?;
                    if decoded.is_empty() || !names.insert(decoded.clone()) {
                        return None;
                    }
                    name = Some(decoded);
                }
                "kind" if value.len() == 1 => {
                    kind = Some(match atom(&value[0])? {
                        "node" => OutputKind::Node,
                        "mesh" => OutputKind::Mesh,
                        _ => return None,
                    });
                }
                "record_size" => {
                    if positive_integer(source, value).is_none() {
                        return None;
                    }
                    size = Some(());
                }
                "max_records" => {
                    if positive_integer(source, value).is_none() {
                        return None;
                    }
                    maximum = Some(());
                }
                _ => return None,
            }
        }
        let _ = name?;
        has_mesh |= kind? == OutputKind::Mesh;
        size?;
        maximum?;
    }
    Some(has_mesh)
}

fn array_items(tokens: &[Token]) -> Option<Vec<&[Token]>> {
    if !tokens.first().is_some_and(|token| punct(token, '['))
        || !tokens.last().is_some_and(|token| punct(token, ']'))
    {
        return None;
    }
    split(&tokens[1..tokens.len() - 1], ',')
}

fn object_fields(tokens: &[Token]) -> Option<Vec<&[Token]>> {
    if !tokens.first().is_some_and(|token| punct(token, '{'))
        || !tokens.last().is_some_and(|token| punct(token, '}'))
    {
        return None;
    }
    split(&tokens[1..tokens.len() - 1], ',')
}

fn split(tokens: &[Token], delimiter: char) -> Option<Vec<&[Token]>> {
    if tokens.is_empty() {
        return Some(Vec::new());
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' | '[' | '{' => stack.push(character),
                ')' | ']' | '}' => {
                    let expected = match character {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    if stack.pop() != Some(expected) {
                        return None;
                    }
                }
                _ => {}
            }
            if character == delimiter && stack.is_empty() {
                if start == index {
                    return None;
                }
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
        }
    }
    if !stack.is_empty() || start == tokens.len() {
        return None;
    }
    parts.push(&tokens[start..]);
    Some(parts)
}

fn unsigned_integer(source: &str, tokens: &[Token]) -> Option<u64> {
    if tokens.len() != 1 {
        return None;
    }
    let token = &tokens[0];
    let text = source.get(token.span.start..token.span.end)?;
    u64::try_from(tir_format::numeric_literals::parse_unsigned_integer(text)?).ok()
}

fn positive_integer(source: &str, tokens: &[Token]) -> Option<u64> {
    unsigned_integer(source, tokens).filter(|value| *value > 0)
}

fn calls(tokens: &[Token], name: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    for index in 0..tokens.len().saturating_sub(1) {
        if atom(&tokens[index]) != Some(name) || !punct(&tokens[index + 1], '(') {
            continue;
        }
        let mut depth = 0usize;
        for close in index + 1..tokens.len() {
            if punct(&tokens[close], '(') {
                depth += 1;
            } else if punct(&tokens[close], ')') {
                let Some(next_depth) = depth.checked_sub(1) else {
                    break;
                };
                depth = next_depth;
                if depth == 0 {
                    found.push((index, close));
                    break;
                }
            }
        }
    }
    found
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn punct(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
