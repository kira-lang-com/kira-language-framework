use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_layout::LayoutEngine;

use crate::gpu_profile::policy::TypePolicies;

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(
    source: &str,
    instruction: &tir_format::TextInstruction,
    layouts: &mut LayoutEngine,
    policies: &TypePolicies,
) -> Vec<Diagnostic> {
    let error =
        |message: String| Diagnostic::error(V2, message).with_primary_span(instruction.span);
    let Some(ty) = super::explicit_type(source, instruction) else {
        return vec![error("const.bytes has an invalid result type".to_owned())];
    };
    if !policies.is_trivial(&ty) {
        return vec![error(
            "const.bytes requires a type with trivial copy and destroy policies".to_owned(),
        )];
    }
    let bytes = match byte_string(instruction) {
        Ok(bytes) => bytes,
        Err(message) => return vec![error(message)],
    };
    let layout = match layouts.layout(&ty) {
        Ok(layout) => layout,
        Err(layout_error) => {
            return vec![error(format!(
                "const.bytes result type has no computable layout: {layout_error}"
            ))];
        }
    };
    if layout.size != bytes.len() as u64 {
        return vec![error(format!(
            "const.bytes contains {} bytes; result layout requires {}",
            bytes.len(),
            layout.size
        ))];
    }
    Vec::new()
}

fn byte_string(instruction: &tir_format::TextInstruction) -> Result<Vec<u8>, String> {
    let tokens = &instruction.tokens;
    let start = crate::ssa::text_instruction_operands_start(tokens);
    let comma = tokens
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, token)| punctuation(token, ',').then_some(index))
        .ok_or_else(|| "const.bytes requires a b-prefixed string literal".to_owned())?;
    if atom_at(tokens, comma + 1) != Some("b") {
        return Err("const.bytes requires a b-prefixed string literal".to_owned());
    }
    let Some(TokenKind::String(raw)) = tokens.get(comma + 2).map(|token| &token.kind) else {
        return Err("const.bytes requires a b-prefixed string literal".to_owned());
    };
    tir_format::text::decode_kira_byte_string(raw)
        .map_err(|escape_error| format!("const.bytes: {escape_error}"))
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    match &tokens.get(index)?.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}
