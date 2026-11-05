use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::parse_tvisa_function_body;
use tir_format::text::{Token, TokenKind, lex};
use tvisa_format::{TextCallingConvention, TextImage, TextItemKind};

const VFY21: Code = Code::new(Dialect::Tvisa, Area::Verifier, 21);

pub(super) fn verify_image(image: &TextImage) -> Vec<Diagnostic> {
    let data_kinds = data_kinds(image);
    let (function_conventions, indirect_sets) = callable_symbols(image);
    let mut diagnostics = Vec::new();

    for item in &image.items {
        if item.kind != TextItemKind::Function {
            continue;
        }
        let source = image.item_text(*item);
        let Some(signature_name) = super::gpu::function_signature_name(source) else {
            continue;
        };
        let Some(signature) = image
            .signatures
            .iter()
            .find(|signature| signature.name == signature_name)
        else {
            continue;
        };
        if !matches!(
            signature.convention,
            TextCallingConvention::Gpu | TextCallingConvention::Entry
        ) {
            continue;
        }
        let Ok(Some(body)) = parse_tvisa_function_body(source) else {
            continue;
        };

        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if instruction.opcode == "global.addr"
                && let Some((name, _)) = super::symbols::instruction_symbol_operand(instruction)
                && let Some(kind) = data_kinds.get(&name)
                && kind.as_deref() != Some("gpu_constant")
            {
                diagnostics.push(
                    Diagnostic::error(
                        VFY21,
                        format!(
                            "GPU global.addr @{name} must reference initialized GPU constant data"
                        ),
                    )
                    .with_primary_span(shift(instruction.span, item.span.start)),
                );
            }

            if let Some(callee) = super::gpu::direct_call_target(instruction) {
                if let Some(convention) = function_conventions.get(&callee)
                    && *convention != TextCallingConvention::Gpu
                {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY21,
                            format!(
                                "GPU direct call @{callee} must target a gpu-convention function"
                            ),
                        )
                        .with_primary_span(shift(instruction.span, item.span.start)),
                    );
                } else if indirect_sets.contains(&callee) {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY21,
                            format!(
                                "GPU direct call @{callee} cannot target a variant or dispatch set"
                            ),
                        )
                        .with_primary_span(shift(instruction.span, item.span.start)),
                    );
                }
            }
        }
    }

    diagnostics
}

fn data_kinds(image: &TextImage) -> HashMap<String, Option<String>> {
    let mut kinds = HashMap::new();
    for item in &image.items {
        let source = image.item_text(*item);
        match item.kind {
            TextItemKind::Data => {
                if let Some((name, kind, _per_cpu)) = data_record(source) {
                    kinds.insert(name, Some(kind));
                }
            }
            TextItemKind::Import => {
                if let Some(name) = imported_data_name(source) {
                    kinds.insert(name, None);
                }
            }
            _ => {}
        }
    }
    kinds
}

pub(super) fn callable_symbols(
    image: &TextImage,
) -> (HashMap<String, TextCallingConvention>, HashSet<String>) {
    let mut functions = HashMap::new();
    let mut indirect_sets = HashSet::new();
    for item in &image.items {
        let source = image.item_text(*item);
        match item.kind {
            TextItemKind::Function | TextItemKind::Import => {
                let (Some(name), Some(signature_name)) = (
                    item_symbol(source),
                    super::gpu::function_signature_name(source),
                ) else {
                    continue;
                };
                if let Some(signature) = image
                    .signatures
                    .iter()
                    .find(|signature| signature.name == signature_name)
                {
                    functions.insert(name, signature.convention);
                }
            }
            TextItemKind::Variants | TextItemKind::Dispatch => {
                if let Some(name) = item_symbol(source) {
                    indirect_sets.insert(name);
                }
            }
            _ => {}
        }
    }
    (functions, indirect_sets)
}

pub(super) fn data_record(source: &str) -> Option<(String, String, bool)> {
    let tokens = significant_tokens(source)?;
    if atom(tokens.first()?)? != "data" {
        return None;
    }
    let at = tokens.iter().position(|token| punctuation(token, '@'))?;
    let references: Vec<_> = tokens.iter().collect();
    let (name, next, _) = super::symbols::read_symbol_refs(&references, at)?;
    let kind_index = if atom(tokens.get(next)?).is_some_and(is_linkage) {
        next + 1
    } else {
        next
    };
    let per_cpu = tokens
        .get(kind_index + 1)
        .is_some_and(|token| atom(token) == Some("percpu"));
    Some((name, atom(tokens.get(kind_index)?)?.to_owned(), per_cpu))
}

pub(super) fn imported_data_name(source: &str) -> Option<String> {
    let tokens = significant_tokens(source)?;
    if atom(tokens.first()?)? != "import" || atom(tokens.get(1)?)? != "data" {
        return None;
    }
    let at = tokens.iter().position(|token| punctuation(token, '@'))?;
    let references: Vec<_> = tokens.iter().collect();
    super::symbols::read_symbol_refs(&references, at).map(|(name, _, _)| name)
}

pub(super) fn item_symbol(source: &str) -> Option<String> {
    let tokens = significant_tokens(source)?;
    let at = tokens.iter().position(|token| punctuation(token, '@'))?;
    let references: Vec<_> = tokens.iter().collect();
    super::symbols::read_symbol_refs(&references, at).map(|(name, _, _)| name)
}

fn significant_tokens(source: &str) -> Option<Vec<Token>> {
    Some(
        lex(source)
            .ok()?
            .into_iter()
            .filter(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End))
            .collect(),
    )
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn is_linkage(value: &str) -> bool {
    matches!(value, "local" | "image" | "exported")
}

fn shift(span: Span, base: usize) -> Span {
    Span::new(
        span.start.saturating_add(base),
        span.end.saturating_add(base),
    )
}
