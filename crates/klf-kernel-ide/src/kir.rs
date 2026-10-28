//! `.kir` files: parse and verifier diagnostics, an outline of functions and
//! blocks, and navigation for `%values`, blocks and `@functions`.

use crate::Builder;
use klf_core::FileId;
use klf_facts::{Entity, ReferenceRole, Symbol, SymbolKind, TokenClass, TokenModifier};
use klf_kir::{Function, parse_module};
use std::collections::HashMap;
use std::path::Path;

pub(crate) fn file(builder: &mut Builder<'_>, path: &Path) {
    let Some(text) = builder.text(path) else {
        return;
    };
    let file = builder.file(path);
    let module = match parse_module(&text) {
        Ok(module) => module,
        Err(error) => {
            let start = u32::try_from(error.offset).unwrap_or(u32::MAX);
            let end = text
                .get(error.offset..)
                .and_then(|rest| rest.find('\n'))
                .map_or(u32::try_from(text.len()).unwrap_or(u32::MAX), |length| {
                    start + u32::try_from(length).unwrap_or(0)
                });
            builder.error(file, start, end, "KLF-KIR-PARSE", error.message);
            return;
        }
    };
    for diagnostic in klf_kir_verify::verify_module(&module) {
        builder.report(&diagnostic, &|_| Some(file));
    }
    let mut functions: HashMap<&str, Entity> = HashMap::new();
    for function in &module.functions {
        let extent = extent(&text, function);
        if let Some((start, end)) = find(&text, extent.0, extent.1, &format!("@{}", function.name))
        {
            let entity = entity(file, start);
            functions.insert(function.name.as_str(), entity);
            let signature = line(&text, function.span.start);
            builder.facts.declare_symbol(
                entity,
                Symbol {
                    name: function.name.as_str().to_owned(),
                    kind: Some(SymbolKind::Function),
                    file: Some(file),
                    start: extent.0,
                    end: extent.1,
                    selection_start: start,
                    selection_end: end,
                    signature: signature.trim_end_matches('{').trim_end().to_owned(),
                    parameters: function
                        .parameters
                        .iter()
                        .map(|parameter| format!("%{}: {}", parameter.name, parameter.ty))
                        .collect(),
                    ..Symbol::default()
                },
            );
            builder.facts.classify(
                file,
                start,
                end,
                TokenClass::Function,
                TokenModifier::Declaration.bit(),
            );
        }
    }
    for function in &module.functions {
        body(builder, file, &text, function, &functions);
    }
}

/// Declares the blocks and values of `function` and records every use of
/// them and of other functions.
fn body(
    builder: &mut Builder<'_>,
    file: FileId,
    text: &str,
    function: &Function,
    functions: &HashMap<&str, Entity>,
) {
    let owner = functions.get(function.name.as_str()).copied();
    let mut locals: HashMap<String, Entity> = HashMap::new();
    let mut declare =
        |builder: &mut Builder<'_>, name: String, kind: SymbolKind, span: (u32, u32)| {
            let Some((start, end)) = find(text, span.0, span.1, &name) else {
                return;
            };
            let entity = entity(file, start);
            builder.facts.declare_symbol(
                entity,
                Symbol {
                    name: name.clone(),
                    kind: Some(kind),
                    file: Some(file),
                    start: span.0,
                    end: span.1,
                    selection_start: start,
                    selection_end: end,
                    container: owner,
                    signature: line(text, span.0).to_owned(),
                    local: kind != SymbolKind::Label,
                    ..Symbol::default()
                },
            );
            let class = if kind == SymbolKind::Label {
                TokenClass::Label
            } else {
                TokenClass::Variable
            };
            builder
                .facts
                .classify(file, start, end, class, TokenModifier::Declaration.bit());
            locals.insert(name, entity);
        };
    for parameter in &function.parameters {
        declare(
            builder,
            format!("%{}", parameter.name),
            SymbolKind::Variable,
            (parameter.span.start, parameter.span.end),
        );
    }
    for block in &function.blocks {
        declare(
            builder,
            block.name.as_str().to_owned(),
            SymbolKind::Label,
            (block.span.start, block.span.end),
        );
        for parameter in &block.parameters {
            declare(
                builder,
                format!("%{}", parameter.name),
                SymbolKind::Variable,
                (parameter.span.start, parameter.span.end),
            );
        }
        for instruction in &block.instructions {
            for result in &instruction.results {
                declare(
                    builder,
                    format!("%{result}"),
                    SymbolKind::Variable,
                    (instruction.span.start, instruction.span.end),
                );
            }
        }
    }
    let (from, to) = extent(text, function);
    for (start, end, word) in words(text, from, to) {
        let (target, role) = if let Some(name) = word.strip_prefix('@') {
            (functions.get(name).copied(), ReferenceRole::Call)
        } else {
            (locals.get(word).copied(), ReferenceRole::Read)
        };
        let Some(target) = target else {
            continue;
        };
        if target == entity(file, start) {
            continue;
        }
        builder.facts.reference(file, start, end, target, role);
    }
}

/// The span of `function` through its closing brace; the parser's span
/// covers the header line only.
fn extent(text: &str, function: &Function) -> (u32, u32) {
    let start = function.span.start;
    let end = function
        .blocks
        .iter()
        .flat_map(|block| {
            std::iter::once(block.span.end).chain(
                block
                    .instructions
                    .iter()
                    .map(|instruction| instruction.span.end),
            )
        })
        .fold(function.span.end, u32::max);
    let close = text
        .get(end as usize..)
        .and_then(|rest| rest.find('}'))
        .map_or(end, |at| end + u32::try_from(at + 1).unwrap_or(0));
    (start, close)
}

pub(crate) fn entity(file: FileId, start: u32) -> Entity {
    Entity(u64::from(file.as_u32()) << 32 | u64::from(start))
}

/// The line of `text` starting at `start`, trimmed.
fn line(text: &str, start: u32) -> &str {
    let rest = text.get(start as usize..).unwrap_or_default();
    rest.lines().next().unwrap_or_default().trim()
}

/// The span of the first occurrence of the name `word` in `text[start..end]`.
fn find(text: &str, start: u32, end: u32, word: &str) -> Option<(u32, u32)> {
    words(text, start, end)
        .into_iter()
        .find(|(_, _, found)| *found == word)
        .map(|(start, end, _)| (start, end))
}

/// The names in `text[start..end]` outside comments and strings: `%value`,
/// `@global` and bare words, each with its span.
fn words(text: &str, start: u32, end: u32) -> Vec<(u32, u32, &str)> {
    let bytes = text.as_bytes();
    let end = (end as usize).min(bytes.len());
    let name = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$');
    let mut found = Vec::new();
    let mut index = start as usize;
    while index < end {
        let byte = bytes[index];
        if byte == b'"' {
            index += 1;
            while index < end && bytes[index] != b'"' {
                index += 1 + usize::from(bytes[index] == b'\\');
            }
            index += 1;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            while index < end && bytes[index] != b'\n' {
                index += 1;
            }
        } else if byte == b'%' || byte == b'@' || byte.is_ascii_alphabetic() || byte == b'_' {
            let first = index;
            index += 1;
            while index < end && name(bytes[index]) {
                index += 1;
            }
            let word = &text[first..index];
            if word.len() > 1 || name(byte) {
                let offset = |at: usize| u32::try_from(at).unwrap_or(u32::MAX);
                found.push((offset(first), offset(index), word));
            }
        } else {
            index += 1;
        }
    }
    found
}
