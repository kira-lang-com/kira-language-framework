//! Source-level V2 checks for the C nonlocal-jump entry point.
use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::FunctionBody;
use tir_format::text::{TokenKind, lex};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(source: &str, body: &FunctionBody) -> Vec<Diagnostic> {
    let setjmps = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| instruction.opcode == "setjmp");
    let has_returns_twice = lex(source).ok().is_some_and(|tokens| {
        tokens
            .iter()
            .take_while(|token| !matches!(token.kind, TokenKind::Punctuation('{')))
            .any(|token| matches!(&token.kind, TokenKind::Atom(name) if name == "returns_twice"))
    });
    if has_returns_twice {
        return Vec::new();
    }
    setjmps
        .map(|instruction| {
            Diagnostic::error(
                V2,
                "setjmp requires the containing function to be marked returns_twice",
            )
            .with_primary_span(instruction.span)
        })
        .collect()
}
