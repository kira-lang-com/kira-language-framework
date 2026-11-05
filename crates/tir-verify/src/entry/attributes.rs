use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::parse_type;
use tir_format::text::{Token, TokenKind};
use tir_layout::LayoutEngine;
use tir_types::Type;

use crate::gpu_profile::policy::TypePolicies;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);
const ENTRY_ATTRIBUTES: &[&str] = &[
    "threads",
    "mesh",
    "payload_size",
    "attributes",
    "caps",
    "subgroup_size",
    "fp",
    "node",
];

pub(super) fn verify(
    source: &str,
    stage: &str,
    header: &[Token],
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let attribute_start = attribute_start(source, header, &mut diagnostics);
    let attributes = &header[attribute_start..];
    verify_attribute_syntax(source, attributes, &mut diagnostics);
    let thread_calls = calls(attributes, "threads");
    let needs_threads = matches!(stage, "compute" | "object" | "mesh" | "tile");
    if needs_threads && thread_calls.len() != 1 {
        diagnostics.push(error(format!(
            "{stage} entry requires exactly one threads attribute"
        )));
    }
    if !needs_threads && !thread_calls.is_empty() {
        diagnostics.push(error(format!("threads is not valid for {stage} entries")));
    }
    for (start, end) in &thread_calls {
        let args = &attributes[start + 2..*end];
        let valid = if atom_at(args, 0) == Some("dynamic") {
            stage == "compute" && args.len() == 1
        } else {
            integer_triplet(args).is_some_and(|values| {
                values.iter().all(|value| *value > 0) && (stage != "tile" || values[2] == 1)
            })
        };
        if !valid {
            diagnostics.push(
                error(format!("invalid threads attribute for {stage} entry"))
                    .with_primary_span(attributes[*start].span),
            );
        }
    }
    let payload_sizes = calls(attributes, "payload_size");
    if stage != "object" && !payload_sizes.is_empty() {
        diagnostics.push(error(format!(
            "payload_size is not valid for {stage} entries"
        )));
    }
    if stage == "object" && payload_sizes.len() != 1 {
        diagnostics.push(error(
            "object entry requires exactly one payload_size attribute",
        ));
    }
    for (start, end) in payload_sizes {
        let size = attributes.get(start + 2).and_then(atom).and_then(parse_u64);
        if size.is_none_or(|size| size > 16_384) || end != start + 3 {
            diagnostics.push(
                error("payload_size must be an integer no greater than 16384")
                    .with_primary_span(attributes[start].span),
            );
        }
    }
    let mesh = calls(attributes, "mesh");
    if stage != "mesh" && !mesh.is_empty() {
        diagnostics.push(error(format!("mesh is not valid for {stage} entries")));
    }
    if stage == "mesh" && mesh.len() != 1 {
        diagnostics.push(error("mesh entry requires exactly one mesh attribute"));
    }
    for (start, end) in mesh {
        let args = &attributes[start + 2..end];
        if !valid_mesh_syntax(args) {
            continue;
        }
        let vertices = named_integer(args, "max_vertices");
        let primitives = named_integer(args, "max_primitives");
        let topology = named_value(args, "topology");
        if named_count(args, "max_vertices") != 1
            || named_count(args, "max_primitives") != 1
            || named_count(args, "topology") != 1
            || !vertices.is_some_and(|value| value <= 256)
            || !primitives.is_some_and(|value| value <= 512)
            || !topology.is_some_and(|value| matches!(value, "points" | "lines" | "triangles"))
        {
            diagnostics.push(
                error("mesh limits or topology are invalid")
                    .with_primary_span(attributes[start].span),
            );
        }
    }
    if stage == "intersection" {
        let attrs = calls(attributes, "attributes");
        if attrs.len() != 1 {
            diagnostics.push(error(
                "intersection entry requires exactly one attributes type",
            ));
        }
        for (start, end) in attrs {
            let parsed = type_from_tokens(source, attributes, start + 2, end);
            let Some(ty) = parsed else {
                diagnostics.push(
                    error("attributes requires a valid struct type")
                        .with_primary_span(attributes[start].span),
                );
                continue;
            };
            require_struct(
                &ty,
                "intersection attributes",
                attributes[start].span,
                policies,
                &mut diagnostics,
            );
            match layouts.layout(&ty) {
                Ok(layout) if layout.size > 32 => diagnostics.push(type_error(
                    "intersection attributes exceed the 32-byte limit",
                    attributes[start].span,
                )),
                Err(layout_error) => diagnostics.push(type_error(
                    format!("intersection attributes have no layout: {layout_error}"),
                    attributes[start].span,
                )),
                _ => {}
            }
        }
    } else if !calls(attributes, "attributes").is_empty() {
        diagnostics.push(error(format!(
            "attributes is not valid for {stage} entries"
        )));
    }
    verify_node_attribute(stage, attributes, &mut diagnostics);
    let early_tests = attributes
        .iter()
        .filter(|token| atom(token) == Some("early_fragment_tests"))
        .count();
    if early_tests > 1 {
        diagnostics.push(error("entry has multiple early_fragment_tests attributes"));
    }
    if stage != "fragment" && early_tests > 0 {
        diagnostics.push(error(
            "early_fragment_tests is only valid for fragment entries",
        ));
    }
    verify_fp_attribute(attributes, &mut diagnostics);
    verify_subgroup_size(attributes, &mut diagnostics);
    diagnostics
}

fn attribute_start(source: &str, header: &[Token], diagnostics: &mut Vec<Diagnostic>) -> usize {
    let Some(open) = header.iter().position(|token| punctuation(token, '(')) else {
        return header.len();
    };
    let Some(close) = matching(header, open, '(', ')') else {
        return header.len();
    };
    let mut start = close + 1;
    if !header
        .get(start)
        .is_some_and(|token| matches!(token.kind, TokenKind::Arrow))
    {
        return start;
    }
    start += 1;
    for end in start + 1..=header.len() {
        let Some(first) = header.get(start) else {
            break;
        };
        let Some(last) = header.get(end - 1) else {
            continue;
        };
        if source
            .get(first.span.start..last.span.end)
            .is_some_and(|text| parse_type(text).is_ok())
        {
            return end;
        }
    }
    diagnostics
        .push(error("entry result must be a valid type").with_primary_span(header[close + 1].span));
    header.len()
}

fn verify_attribute_syntax(source: &str, attributes: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    let mut index = 0;
    while index < attributes.len() {
        let token = &attributes[index];
        let Some(name) = atom(token) else {
            diagnostics.push(error("expected a GPU entry attribute").with_primary_span(token.span));
            index += 1;
            continue;
        };
        if name == "early_fragment_tests" {
            index += 1;
            continue;
        }
        if !ENTRY_ATTRIBUTES.contains(&name) {
            diagnostics.push(
                error(format!("unknown GPU entry attribute '{name}'"))
                    .with_primary_span(token.span),
            );
            index = attributes
                .get(index + 1)
                .filter(|token| punctuation(token, '('))
                .and_then(|_| matching(attributes, index + 1, '(', ')'))
                .map_or(index + 1, |close| close + 1);
            continue;
        }
        if !attributes
            .get(index + 1)
            .is_some_and(|token| punctuation(token, '('))
        {
            diagnostics.push(
                error(format!("GPU entry attribute '{name}' requires parentheses"))
                    .with_primary_span(token.span),
            );
            index += 1;
            continue;
        }
        let Some(close) = matching(attributes, index + 1, '(', ')') else {
            diagnostics.push(
                error(format!(
                    "GPU entry attribute '{name}' has no closing parenthesis"
                ))
                .with_primary_span(token.span),
            );
            break;
        };
        let arguments = &attributes[index + 2..close];
        let valid = match name {
            "mesh" => valid_mesh_syntax(arguments),
            "caps" => valid_capability_syntax(arguments),
            "node" => valid_node_syntax(source, arguments),
            _ => true,
        };
        if !valid {
            diagnostics.push(
                error(format!("GPU entry attribute '{name}' has invalid syntax"))
                    .with_primary_span(token.span),
            );
        }
        index = close + 1;
    }
}

fn valid_mesh_syntax(arguments: &[Token]) -> bool {
    arguments.len() == 11
        && atom_at(arguments, 0) == Some("max_vertices")
        && punctuation(&arguments[1], ':')
        && atom_at(arguments, 2).and_then(parse_u64).is_some()
        && punctuation(&arguments[3], ',')
        && atom_at(arguments, 4) == Some("max_primitives")
        && punctuation(&arguments[5], ':')
        && atom_at(arguments, 6).and_then(parse_u64).is_some()
        && punctuation(&arguments[7], ',')
        && atom_at(arguments, 8) == Some("topology")
        && punctuation(&arguments[9], ':')
        && atom_at(arguments, 10).is_some()
}

fn valid_capability_syntax(arguments: &[Token]) -> bool {
    let mut index = 0;
    let mut count = 0;
    while index < arguments.len() {
        if atom_at(arguments, index).is_none() {
            return false;
        }
        index += 1;
        while index < arguments.len() && punctuation(&arguments[index], '.') {
            if atom_at(arguments, index + 1).is_none() {
                return false;
            }
            index += 2;
        }
        count += 1;
        if index == arguments.len() {
            break;
        }
        if !punctuation(&arguments[index], ',') || index + 1 == arguments.len() {
            return false;
        }
        index += 1;
    }
    count > 0
}

fn valid_node_syntax(source: &str, arguments: &[Token]) -> bool {
    if arguments
        .last()
        .is_some_and(|token| punctuation(token, ','))
    {
        return false;
    }
    let fields = split_arguments(arguments);
    if !fields.first().is_some_and(|field| {
        field.len() == 3
            && atom_at(field, 0) == Some("launch")
            && punctuation(&field[1], ':')
            && atom_at(field, 2)
                .is_some_and(|value| matches!(value, "broadcast" | "coalescing" | "thread"))
    }) {
        return false;
    }
    fields.iter().skip(1).all(|field| {
        field.len() >= 3
            && atom_at(field, 0).is_some()
            && punctuation(&field[1], ':')
            && valid_literal(source, &field[2..])
    })
}

fn split_arguments(tokens: &[Token]) -> Vec<&[Token]> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Punctuation('(') => depth += 1,
            TokenKind::Punctuation(')') => depth -= 1,
            TokenKind::Punctuation(',') if depth == 0 => {
                fields.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < tokens.len() {
        fields.push(&tokens[start..]);
    }
    fields
}

fn valid_literal(source: &str, tokens: &[Token]) -> bool {
    if tokens.len() == 1 {
        if matches!(tokens[0].kind, TokenKind::String(_)) {
            return true;
        }
        if let Some(value) = atom(&tokens[0]) {
            return matches!(value, "true" | "false" | "null" | "inf")
                || numeric_literal(source, tokens);
        }
    }
    if tokens.len() == 2
        && atom_at(tokens, 0) == Some("b")
        && matches!(tokens[1].kind, TokenKind::String(_))
    {
        return true;
    }
    if tokens.len() >= 4
        && atom_at(tokens, 0) == Some("nan")
        && punctuation(&tokens[1], '(')
        && tokens.last().is_some_and(|token| punctuation(token, ')'))
        && integer_literal(source, &tokens[2..tokens.len() - 1])
    {
        return true;
    }
    numeric_literal(source, tokens)
}

fn numeric_literal(source: &str, tokens: &[Token]) -> bool {
    let Some(text) = literal_text(source, tokens) else {
        return false;
    };
    let cleaned = text.replace('_', "");
    parse_integer_text(text).is_some() || cleaned.parse::<f64>().is_ok()
}

fn integer_literal(source: &str, tokens: &[Token]) -> bool {
    literal_text(source, tokens)
        .and_then(parse_integer_text)
        .is_some()
}

fn literal_text<'a>(source: &'a str, tokens: &[Token]) -> Option<&'a str> {
    let Some(first) = tokens.first() else {
        return None;
    };
    let Some(last) = tokens.last() else {
        return None;
    };
    source.get(first.span.start..last.span.end)
}

fn parse_integer_text(text: &str) -> Option<i128> {
    tir_format::numeric_literals::parse_signed_integer(text)
}

fn verify_subgroup_size(header: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    let sizes = calls(header, "subgroup_size");
    if sizes.len() > 1 {
        diagnostics.push(error("entry has multiple subgroup_size attributes"));
    }
    for (start, end) in sizes {
        let size = header.get(start + 2).and_then(atom).and_then(parse_u64);
        if !size.is_some_and(|value| (4..=128).contains(&value) && value.is_power_of_two())
            || end != start + 3
        {
            diagnostics.push(
                error("subgroup_size must be a power of two from 4 through 128")
                    .with_primary_span(header[start].span),
            );
        }
    }
}

fn calls(tokens: &[Token], name: &str) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    for index in 0..tokens.len().saturating_sub(1) {
        if atom(&tokens[index]) == Some(name) && punctuation(&tokens[index + 1], '(') {
            if let Some(close) = matching(tokens, index + 1, '(', ')') {
                result.push((index, close));
            }
        }
    }
    result
}

fn matching(tokens: &[Token], open: usize, pair: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::Punctuation(character) if character == pair => depth += 1,
            TokenKind::Punctuation(character) if character == close => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn integer_triplet(tokens: &[Token]) -> Option<[u64; 3]> {
    if tokens.len() != 5 || !punctuation(&tokens[1], ',') || !punctuation(&tokens[3], ',') {
        return None;
    }
    Some([
        parse_u64(atom(&tokens[0])?)?,
        parse_u64(atom(&tokens[2])?)?,
        parse_u64(atom(&tokens[4])?)?,
    ])
}

fn named_integer(tokens: &[Token], name: &str) -> Option<u64> {
    tokens.windows(3).find_map(|window| {
        (atom(&window[0]) == Some(name) && punctuation(&window[1], ':'))
            .then(|| atom(&window[2]).and_then(parse_u64))
            .flatten()
    })
}

fn named_value<'a>(tokens: &'a [Token], name: &str) -> Option<&'a str> {
    tokens.windows(3).find_map(|window| {
        (atom(&window[0]) == Some(name) && punctuation(&window[1], ':'))
            .then(|| atom(&window[2]))
            .flatten()
    })
}

fn named_count(tokens: &[Token], name: &str) -> usize {
    tokens
        .windows(2)
        .filter(|pair| atom(&pair[0]) == Some(name) && punctuation(&pair[1], ':'))
        .count()
}

fn verify_node_attribute(stage: &str, header: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    let nodes = calls(header, "node");
    if stage == "node" && nodes.len() != 1 {
        diagnostics.push(error("node entry requires exactly one node attribute"));
    }
    if stage != "node" && !nodes.is_empty() {
        diagnostics.push(error(format!("node is not valid for {stage} entries")));
    }
    for (start, end) in nodes {
        let args = &header[start + 2..end];
        let launch = named_value(args, "launch");
        if named_count(args, "launch") != 1
            || !launch.is_some_and(|value| matches!(value, "broadcast" | "coalescing" | "thread"))
        {
            diagnostics.push(
                error("node launch must be broadcast, coalescing, or thread")
                    .with_primary_span(header[start].span),
            );
        }
    }
}

fn verify_fp_attribute(header: &[Token], diagnostics: &mut Vec<Diagnostic>) {
    let modes = calls(header, "fp");
    if modes.len() > 1 {
        diagnostics.push(error("entry has multiple fp attributes"));
    }
    for (start, end) in modes {
        let args = &header[start + 2..end];
        if args.len() != 1
            || !matches!(
                atom_at(args, 0),
                Some("precise" | "fast" | "ieee_denormals")
            )
        {
            diagnostics.push(
                error("fp attribute must be precise, fast, or ieee_denormals")
                    .with_primary_span(header[start].span),
            );
        }
    }
}

fn type_from_tokens(source: &str, tokens: &[Token], start: usize, end: usize) -> Option<Type> {
    let first = tokens.get(start)?;
    let last = tokens.get(end.checked_sub(1)?)?;
    parse_type(source.get(first.span.start..last.span.end)?).ok()
}

fn require_struct(
    ty: &Type,
    name: &str,
    span: Span,
    policies: &TypePolicies,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !policies.is_struct(ty) {
        diagnostics.push(type_error(
            format!("{name} must be a defined struct type"),
            span,
        ));
    }
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    tokens.get(index).and_then(atom)
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn parse_u64(text: &str) -> Option<u64> {
    u64::try_from(tir_format::numeric_literals::parse_signed_integer(text)?).ok()
}

fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(V15, message)
}

fn type_error(message: impl Into<String>, span: Span) -> Diagnostic {
    error(message).with_primary_span(span)
}
