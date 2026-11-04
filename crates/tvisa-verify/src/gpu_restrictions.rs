use std::collections::{HashMap, HashSet};

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind, lex};
use tvisa_format::{DataKind, TextCallingConvention, TextImage, TextImportPayload, TextItemKind};

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
        let Some(body) = image
            .function_body_for_item(*item)
            .and_then(|parsed| parsed.body.as_ref())
        else {
            continue;
        };

        for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
            if instruction.opcode == "global.addr"
                && let Some((name, _)) = super::symbols::instruction_symbol_operand(instruction)
                && let Some(kind) = data_kinds.get(&name)
                && *kind != Some(DataKind::GpuConstant)
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

fn data_kinds(image: &TextImage) -> HashMap<String, Option<DataKind>> {
    let mut kinds = HashMap::new();
    for item in &image.items {
        match item.kind {
            TextItemKind::Data => {
                if let Some(data) = image.data_for_item(*item) {
                    kinds.insert(data.symbol.clone(), Some(data.kind));
                }
            }
            TextItemKind::Import => {
                if let Some(import) = image.import_for_item(*item)
                    && matches!(&import.payload, TextImportPayload::Data { .. })
                {
                    kinds.insert(import.symbol.clone(), None);
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
            TextItemKind::Function => {
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
            TextItemKind::Import => {
                let Some(import) = image.import_for_item(*item) else {
                    continue;
                };
                let TextImportPayload::Function { signature, .. } = &import.payload else {
                    continue;
                };
                if let Some(signature) = image
                    .signatures
                    .iter()
                    .find(|signature_item| signature_item.name == *signature)
                {
                    functions.insert(import.symbol.clone(), signature.convention);
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

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}

fn shift(span: Span, base: usize) -> Span {
    Span::new(
        span.start.saturating_add(base),
        span.end.saturating_add(base),
    )
}
