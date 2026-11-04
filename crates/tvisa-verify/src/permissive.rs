use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind, lex};
use tir_format::{FunctionBody, TextInstruction};

const VFY15: Code = Code::new(Dialect::Tvisa, Area::Verifier, 15);
const VFY17: Code = Code::new(Dialect::Tvisa, Area::Verifier, 17);

pub(super) fn verify_function(source: &str, body: &FunctionBody) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let defined = is_defined_function(source, body);
    for block in &body.blocks {
        for instruction in &block.instructions {
            if defined && is_permissive_only(instruction) {
                diagnostics.push(
                    Diagnostic::error(
                        VFY15,
                        format!(
                            "{} is only valid in permissive functions",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(instruction.span),
                );
            }
            if matches!(instruction.opcode.as_str(), "trap" | "trap.if")
                && !has_valid_trap_code(instruction)
            {
                diagnostics.push(
                    Diagnostic::error(VFY17, "trap code must be a 16-bit integer immediate")
                        .with_primary_span(instruction.span),
                );
            }
        }
    }
    diagnostics
}

fn is_defined_function(source: &str, body: &FunctionBody) -> bool {
    let Ok(tokens) = lex(source) else {
        return true;
    };
    let header_end = tokens
        .iter()
        .position(|token| is_punctuation(token, '{') && token.span.end == body.span.start)
        .unwrap_or(tokens.len());
    !tokens[..header_end]
        .windows(2)
        .any(|pair| is_atom(&pair[0], "semantics") && is_atom(&pair[1], "permissive"))
}

fn is_permissive_only(instruction: &TextInstruction) -> bool {
    match instruction.opcode.as_str() {
        "const.poison" | "assume" | "freeze" => true,
        "unreachable" => has_bracket_flag(&instruction.tokens, "ub"),
        "add" | "sub" | "mul" | "shl" | "neg" | "abs" | "ctlz" | "cttz" => {
            has_bracket_flag(&instruction.tokens, "poison")
        }
        "or" => has_bracket_flag(&instruction.tokens, "disjoint"),
        "sdiv" | "udiv" | "lshr" | "ashr" => has_bracket_flag(&instruction.tokens, "exact"),
        "fptosi" | "fptoui" => has_bracket_flag(&instruction.tokens, "poison"),
        "ptr.offset" => has_bracket_flag(&instruction.tokens, "inbounds"),
        "mem.copy" => has_bracket_flag(&instruction.tokens, "nooverlap"),
        "fadd" | "fsub" | "fmul" | "fdiv" | "frem" | "fneg" | "fabs" | "fma" | "fmuladd"
        | "fmin" | "fmax" | "fminnum" | "fmaxnum" | "fcmp" => {
            has_bracket_flag(&instruction.tokens, "nnan")
                || has_bracket_flag(&instruction.tokens, "ninf")
                || has_bracket_flag(&instruction.tokens, "fast")
        }
        opcode if opcode.starts_with("math.") => {
            has_bracket_flag(&instruction.tokens, "nnan")
                || has_bracket_flag(&instruction.tokens, "ninf")
                || has_bracket_flag(&instruction.tokens, "fast")
        }
        _ => false,
    }
}

fn has_bracket_flag(tokens: &[Token], expected: &str) -> bool {
    let mut inside = false;
    for token in tokens {
        match &token.kind {
            TokenKind::Punctuation('[') => inside = true,
            TokenKind::Punctuation(']') => inside = false,
            TokenKind::Atom(value) if inside && value == expected => return true,
            _ => {}
        }
    }
    false
}

fn has_valid_trap_code(instruction: &TextInstruction) -> bool {
    let start = instruction_operands_start(&instruction.tokens);
    let tokens = &instruction.tokens[start..];
    let code = if instruction.opcode == "trap.if" {
        let Some(comma) = tokens.iter().position(|token| is_punctuation(token, ',')) else {
            return false;
        };
        tokens.get(comma + 1)
    } else {
        tokens.first()
    };
    code.and_then(integer_literal)
        .is_some_and(|value| value <= u16::MAX as u64)
}

fn integer_literal(token: &Token) -> Option<u64> {
    let TokenKind::Atom(value) = &token.kind else {
        return None;
    };
    u64::try_from(tir_format::numeric_literals::parse_unsigned_integer(value)?).ok()
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

fn is_atom(token: &Token, expected: &str) -> bool {
    matches!(&token.kind, TokenKind::Atom(value) if value == expected)
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
