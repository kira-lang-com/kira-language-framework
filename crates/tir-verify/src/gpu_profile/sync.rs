use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_format::text::{Token, TokenKind};

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);
const BARRIER_MEMORY: &[&str] = &["device", "threadgroup", "texture", "tile", "object_payload"];
const FENCE_MEMORY: &[&str] = &[
    "device",
    "constant",
    "threadgroup",
    "thread",
    "texture",
    "tile",
    "object_payload",
    "ray_payload",
    "hit_attribute",
    "node_record",
];
const FENCE_SCOPES: &[&str] = &["thread", "subgroup", "threadgroup", "device", "system"];

pub(super) fn verify(instruction: &TextInstruction) -> Vec<Diagnostic> {
    let opcode = instruction.opcode.as_str();
    if !matches!(opcode, "gpu.barrier" | "gpu.memfence") {
        return Vec::new();
    }
    let mut diagnostics = Vec::new();
    let mut scope_seen = false;
    let mut memory_seen = false;
    let mut index = 0;
    while index < instruction.tokens.len() {
        if punctuation_at(&instruction.tokens, index, ']') {
            diagnostics.push(error(
                instruction,
                "unmatched closing GPU synchronization clause",
            ));
            index += 1;
            continue;
        }
        if !punctuation_at(&instruction.tokens, index, '[') {
            index += 1;
            continue;
        }
        let Some(close) = matching_bracket(&instruction.tokens, index) else {
            diagnostics.push(error(
                instruction,
                "unterminated GPU synchronization clause",
            ));
            break;
        };
        let clause = &instruction.tokens[index + 1..close];
        if clause.len() == 1 && atom_at(clause, 0) == Some("unchecked_uniformity") {
            index = close + 1;
            continue;
        }
        if atom_at(clause, 0) == Some("memory") {
            if memory_seen {
                diagnostics.push(error(
                    instruction,
                    "GPU synchronization instruction has multiple memory clauses",
                ));
            }
            memory_seen = true;
            verify_memory_list(instruction, clause, opcode, &mut diagnostics);
        } else {
            if scope_seen {
                diagnostics.push(error(
                    instruction,
                    "GPU synchronization instruction has multiple scope clauses",
                ));
            }
            scope_seen = true;
            verify_scope(instruction, clause, opcode, &mut diagnostics);
        }
        index = close + 1;
    }
    diagnostics
}

fn verify_scope(
    instruction: &TextInstruction,
    clause: &[Token],
    opcode: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let valid = if opcode == "gpu.barrier" {
        clause.len() == 1 && matches!(atom_at(clause, 0), Some("subgroup" | "threadgroup"))
    } else {
        clause.len() == 1 && atom_at(clause, 0).is_some_and(|scope| FENCE_SCOPES.contains(&scope))
    };
    if !valid {
        diagnostics.push(error(
            instruction,
            if opcode == "gpu.barrier" {
                "gpu.barrier scope must be subgroup or threadgroup"
            } else {
                "gpu.memfence scope must be thread, subgroup, threadgroup, device, or system"
            },
        ));
    }
}

fn verify_memory_list(
    instruction: &TextInstruction,
    clause: &[Token],
    opcode: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !punctuation_at(clause, 1, ':') {
        diagnostics.push(error(instruction, "memory clause must use 'memory: ...'"));
        return;
    }
    let allowed = if opcode == "gpu.barrier" {
        BARRIER_MEMORY
    } else {
        FENCE_MEMORY
    };
    let values = &clause[2..];
    let mut expect_value = true;
    let mut seen = Vec::new();
    for token in values {
        if expect_value {
            let Some(value) = atom(token) else {
                diagnostics.push(error(instruction, "memory clause expects a memory class"));
                return;
            };
            if !allowed.contains(&value) {
                diagnostics.push(error(
                    instruction,
                    format!("{value} is not valid in a {opcode} memory clause"),
                ));
            }
            if seen.contains(&value) {
                diagnostics.push(error(
                    instruction,
                    format!("memory class '{value}' appears more than once"),
                ));
            }
            seen.push(value);
        } else if !punctuation(token, ',') {
            diagnostics.push(error(instruction, "memory classes must be comma-separated"));
            return;
        }
        expect_value = !expect_value;
    }
    if values.is_empty() || expect_value {
        diagnostics.push(error(
            instruction,
            "memory clause requires at least one memory class",
        ));
    }
}

fn matching_bracket(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::Punctuation('[') => depth += 1,
            TokenKind::Punctuation(']') => {
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

fn punctuation_at(tokens: &[Token], index: usize, character: char) -> bool {
    tokens
        .get(index)
        .is_some_and(|token| punctuation(token, character))
}

fn punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

fn atom_at(tokens: &[Token], index: usize) -> Option<&str> {
    tokens.get(index).and_then(atom)
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(atom) => Some(atom),
        _ => None,
    }
}

fn error(instruction: &TextInstruction, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(V15, message).with_primary_span(instruction.span)
}
