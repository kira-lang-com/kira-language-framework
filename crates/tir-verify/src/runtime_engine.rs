use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{TokenKind, lex};
use tir_format::{FunctionBody, TextInstruction};

const V9: Code = Code::new(Dialect::Tir, Area::Verifier, 9);

pub(super) fn is_restricted(source: &str) -> bool {
    let Ok(tokens) = lex(source) else {
        return false;
    };
    let end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    let header = &tokens[..end];
    let mut restricted = false;
    for (index, token) in header.iter().enumerate() {
        if !matches!(&token.kind, TokenKind::Atom(name) if name == "engine") {
            continue;
        }
        let mode = match (
            header.get(index + 1).map(|token| &token.kind),
            header.get(index + 2).map(|token| &token.kind),
            header.get(index + 3).map(|token| &token.kind),
        ) {
            (
                Some(TokenKind::Punctuation('(')),
                Some(TokenKind::Atom(mode)),
                Some(TokenKind::Punctuation(')')),
            ) => mode.as_str(),
            _ => continue,
        };
        restricted |= matches!(mode, "runtime" | "any");
    }
    restricted
}

pub(super) fn verify_instructions(body: &FunctionBody, restricted: bool) -> Vec<Diagnostic> {
    if !restricted {
        return Vec::new();
    }
    body.blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(verify_instruction)
        .collect()
}

fn verify_instruction(instruction: &TextInstruction) -> Option<Diagnostic> {
    let form = match instruction.opcode.as_str() {
        "asm" => "asm",
        "asm.goto" => "asm.goto",
        "target.intrinsic" => "target.intrinsic",
        "mmio.load" => "mmio.load",
        "mmio.store" => "mmio.store",
        "fence.io" => "fence.io",
        _ => return None,
    };
    Some(
        Diagnostic::error(
            V9,
            format!("{form} is forbidden in a runtime or any engine function"),
        )
        .with_primary_span(instruction.span),
    )
}
