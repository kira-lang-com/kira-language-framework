use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, SemanticsMode, TextItemKind};

const V14: Code = Code::new(Dialect::Tir, Area::Verifier, 14);

pub(super) fn verify_function(
    module_mode: Option<SemanticsMode>,
    source: &str,
    kind: TextItemKind,
    body: Option<&FunctionBody>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let default_mode = module_mode.unwrap_or(SemanticsMode::Defined);
    let mode = match function_mode(source, default_mode, kind, &mut diagnostics) {
        Some(mode) => mode,
        None => default_mode,
    };
    let Some(body) = body.filter(|_| mode == SemanticsMode::Defined) else {
        return diagnostics;
    };

    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if let Some(form) = permissive_form(instruction) {
            diagnostics.push(
                Diagnostic::error(
                    V14,
                    format!("{form} is only permitted in a permissive function"),
                )
                .with_primary_span(instruction.span),
            );
        }
    }
    diagnostics
}

fn function_mode(
    source: &str,
    default_mode: SemanticsMode,
    kind: TextItemKind,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SemanticsMode> {
    let Ok(tokens) = lex(source) else {
        return None;
    };
    let header_end = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('{')))
        .unwrap_or(tokens.len());
    let header = &tokens[..header_end];
    let mut selected = None;
    let mut saw_attribute = false;
    for index in 0..header.len() {
        if !matches!(&header[index].kind, TokenKind::Atom(atom) if atom == "semantics") {
            continue;
        }
        let parsed = match (
            header.get(index + 1).map(|token| &token.kind),
            header.get(index + 2).map(|token| &token.kind),
            header.get(index + 3).map(|token| &token.kind),
        ) {
            (
                Some(TokenKind::Punctuation('(')),
                Some(TokenKind::Atom(mode)),
                Some(TokenKind::Punctuation(')')),
            ) => match mode.as_str() {
                "defined" => Some(SemanticsMode::Defined),
                "permissive" => Some(SemanticsMode::Permissive),
                _ => {
                    diagnostics.push(
                        Diagnostic::error(V14, format!("unknown semantic mode '{mode}'"))
                            .with_primary_span(header[index + 2].span),
                    );
                    None
                }
            },
            _ => {
                diagnostics.push(
                    Diagnostic::error(V14, "malformed function semantics attribute")
                        .with_primary_span(header[index].span),
                );
                None
            }
        };
        if saw_attribute {
            diagnostics.push(
                Diagnostic::error(V14, "function declares semantics more than once")
                    .with_primary_span(header[index].span),
            );
        } else if let Some(mode) = parsed {
            selected = Some(mode);
        }
        saw_attribute = true;
    }
    if kind == TextItemKind::Entry && selected.is_some() {
        let span = header
            .iter()
            .find(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == "semantics"))
            .map_or_else(
                || {
                    header
                        .last()
                        .map_or(tir_diag::Span::new(0, 0), |token| token.span)
                },
                |token| token.span,
            );
        diagnostics.push(
            Diagnostic::error(
                V14,
                "entry records do not declare a function semantics override",
            )
            .with_primary_span(span),
        );
        return Some(default_mode);
    }
    selected.or(Some(default_mode))
}

fn permissive_form(instruction: &tir_format::TextInstruction) -> Option<&'static str> {
    let modifiers = leading_modifiers(&instruction.tokens);
    let opcode = instruction.opcode.as_str();
    if opcode == "const.poison" {
        return Some("const.poison");
    }
    if opcode == "assume" {
        return Some("assume");
    }
    if opcode == "unreachable" && modifiers.contains(&"ub") {
        return Some("unreachable [ub]");
    }
    if opcode == "or" && modifiers.contains(&"disjoint") {
        return Some("or [disjoint]");
    }
    if opcode == "ptr.offset" && modifiers.contains(&"inbounds") {
        return Some("ptr.offset [inbounds]");
    }
    if opcode == "mem.copy" && modifiers.contains(&"nooverlap") {
        return Some("mem.copy [nooverlap]");
    }
    if opcode == "heap.alloc.raw"
        && instruction
            .tokens
            .iter()
            .any(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == "uninit"))
    {
        return Some("heap.alloc.raw [uninit]");
    }
    if modifiers.contains(&"poison")
        && matches!(
            opcode,
            "add" | "sub" | "mul" | "shl" | "neg" | "ctlz" | "cttz" | "fptosi" | "fptoui"
        )
    {
        return Some("[poison]");
    }
    if modifiers.contains(&"exact") && matches!(opcode, "sdiv" | "udiv" | "lshr" | "ashr") {
        return Some("[exact]");
    }
    if modifiers
        .iter()
        .any(|flag| matches!(*flag, "nnan" | "ninf" | "fast"))
    {
        return Some("nnan/ninf/fast fast-math flags");
    }
    let metadata = metadata_names(&instruction.tokens);
    if let Some(name) = metadata
        .iter()
        .find(|name| matches!(**name, "noalias" | "alias.scope" | "tbaa"))
    {
        return Some(match *name {
            "noalias" => "!noalias metadata",
            "alias.scope" => "!alias.scope metadata",
            _ => "!tbaa metadata",
        });
    }
    if opcode == "load"
        && metadata
            .iter()
            .any(|name| matches!(*name, "range" | "nonnull" | "align"))
    {
        return Some("load metadata assertion");
    }
    None
}

fn leading_modifiers(tokens: &[Token]) -> Vec<&str> {
    let mut cursor = 0;
    if matches!(
        tokens.first().map(|token| &token.kind),
        Some(TokenKind::Punctuation('%'))
    ) {
        while cursor < tokens.len() && !matches!(tokens[cursor].kind, TokenKind::Punctuation('=')) {
            cursor += 1;
        }
        cursor = cursor.saturating_add(1);
    }
    cursor = cursor.saturating_add(1);
    while cursor + 1 < tokens.len() && matches!(tokens[cursor].kind, TokenKind::Punctuation('.')) {
        cursor += 2;
    }
    let mut modifiers = Vec::new();
    while matches!(
        tokens.get(cursor).map(|token| &token.kind),
        Some(TokenKind::Punctuation('['))
    ) {
        cursor += 1;
        while cursor < tokens.len() && !matches!(tokens[cursor].kind, TokenKind::Punctuation(']')) {
            if let TokenKind::Atom(atom) = &tokens[cursor].kind {
                modifiers.push(atom.as_str());
            }
            cursor += 1;
        }
        cursor = cursor.saturating_add(1);
    }
    modifiers
}

fn metadata_names(tokens: &[Token]) -> Vec<&str> {
    tokens
        .windows(2)
        .filter_map(|pair| match (&pair[0].kind, &pair[1].kind) {
            (TokenKind::Punctuation('!'), TokenKind::Atom(name)) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}
