use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction};
use tir_verify::text_successors;

const VFY16: Code = Code::new(Dialect::Tvisa, Area::Verifier, 16);

pub(super) fn verify_function(body: &FunctionBody) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut labels = HashMap::with_capacity(body.blocks.len());
    for (index, block) in body.blocks.iter().enumerate() {
        labels.entry(block.label.as_str()).or_insert(index);
    }
    let mut landingpads = vec![false; body.blocks.len()];
    for (block_index, block) in body.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if instruction.opcode == "landingpad" {
                if instruction_index != 0 {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY16,
                            "landingpad must be the first instruction in its block",
                        )
                        .with_primary_span(instruction.span),
                    );
                } else {
                    landingpads[block_index] = true;
                    if block_index == 0 {
                        diagnostics.push(
                            Diagnostic::error(VFY16, "function entry block cannot be a landingpad")
                                .with_primary_span(instruction.span),
                        );
                    }
                }
                diagnostics.extend(verify_clauses(instruction));
            }
        }
    }

    let successors = text_successors(body);
    let mut incoming_unwind = vec![0usize; body.blocks.len()];
    let mut incoming_normal = vec![0usize; body.blocks.len()];
    for (source_index, block) in body.blocks.iter().enumerate() {
        let mut unwind_targets = Vec::new();
        for instruction in &block.instructions {
            if instruction.opcode != "invoke" {
                continue;
            }
            if let Some((target, span)) = invoke_unwind_target(instruction) {
                if let Some(target_index) = labels.get(target.as_str()).copied() {
                    unwind_targets.push((target_index, instruction.span));
                    if !landingpads[target_index] {
                        diagnostics.push(
                            Diagnostic::error(
                                VFY16,
                                format!("invoke unwind target {target} must begin with landingpad"),
                            )
                            .with_primary_span(span),
                        );
                    }
                }
            }
        }
        let mut remaining_unwind: Vec<_> =
            unwind_targets.iter().map(|(target, _)| *target).collect();
        for &target in successors.get(source_index).map_or(&[][..], Vec::as_slice) {
            if let Some(position) = remaining_unwind
                .iter()
                .position(|candidate| *candidate == target)
            {
                remaining_unwind.swap_remove(position);
                incoming_unwind[target] = incoming_unwind[target].saturating_add(1);
            } else {
                incoming_normal[target] = incoming_normal[target].saturating_add(1);
            }
        }
        for (target, span) in unwind_targets {
            if remaining_unwind.contains(&target) {
                diagnostics.push(
                    Diagnostic::error(VFY16, "invoke unwind target is not a valid CFG edge")
                        .with_primary_span(span),
                );
            }
        }
    }

    for (index, block) in body.blocks.iter().enumerate() {
        if landingpads[index] && incoming_normal[index] != 0 {
            diagnostics.push(
                Diagnostic::error(
                    VFY16,
                    "landingpad block is reachable through a non-unwind edge",
                )
                .with_primary_span(block.span),
            );
        }
        if landingpads[index]
            && index != 0
            && incoming_unwind[index] == 0
            && incoming_normal[index] == 0
        {
            diagnostics.push(
                Diagnostic::error(VFY16, "landingpad block has no unwind predecessor")
                    .with_primary_span(block.span),
            );
        }
    }
    diagnostics
}

fn verify_clauses(instruction: &TextInstruction) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let start = instruction_operands_start(&instruction.tokens);
    let operands = &instruction.tokens[start..];
    if operands.is_empty() {
        return diagnostics;
    }
    if !is_punctuation(&operands[0], '[') {
        diagnostics.push(
            Diagnostic::error(VFY16, "landingpad clauses must be enclosed in brackets")
                .with_primary_span(operands[0].span),
        );
        return diagnostics;
    }
    if !operands
        .last()
        .is_some_and(|token| is_punctuation(token, ']'))
    {
        diagnostics.push(
            Diagnostic::error(VFY16, "unterminated landingpad clause list")
                .with_primary_span(operands[0].span),
        );
        return diagnostics;
    }
    let clauses = &operands[1..operands.len() - 1];
    let mut cursor = 0;
    let mut saw_cleanup = false;
    while cursor < clauses.len() {
        let Some(clause) = clauses.get(cursor) else {
            break;
        };
        let TokenKind::Atom(word) = &clause.kind else {
            diagnostics.push(
                Diagnostic::error(VFY16, "expected a landingpad clause")
                    .with_primary_span(clause.span),
            );
            break;
        };
        let kind = if word == "catch" {
            if clauses
                .get(cursor + 1)
                .is_some_and(|token| is_punctuation(token, '.'))
            {
                let Some(TokenKind::Atom(suffix)) =
                    clauses.get(cursor + 2).map(|token| &token.kind)
                else {
                    diagnostics.push(
                        Diagnostic::error(VFY16, "malformed catch clause")
                            .with_primary_span(clause.span),
                    );
                    break;
                };
                cursor += 3;
                match suffix.as_str() {
                    "type" => "catch.type",
                    "filter" => "catch.filter",
                    "all" => "catch.all",
                    _ => {
                        diagnostics.push(
                            Diagnostic::error(
                                VFY16,
                                format!("unknown landingpad clause 'catch.{suffix}'"),
                            )
                            .with_primary_span(clause.span),
                        );
                        break;
                    }
                }
            } else {
                diagnostics.push(
                    Diagnostic::error(VFY16, "malformed catch clause")
                        .with_primary_span(clause.span),
                );
                break;
            }
        } else {
            cursor += 1;
            word.as_str()
        };
        match kind {
            "catch.type" => {
                let Some(typeid) = clauses.get(cursor) else {
                    diagnostics.push(
                        Diagnostic::error(VFY16, "catch.type requires a 64-bit type identifier")
                            .with_primary_span(clause.span),
                    );
                    break;
                };
                if integer_literal(typeid).is_none() {
                    diagnostics.push(
                        Diagnostic::error(VFY16, "catch.type requires a 64-bit type identifier")
                            .with_primary_span(typeid.span),
                    );
                }
                cursor += 1;
            }
            "catch.filter" => {
                let valid_symbol = clauses
                    .get(cursor)
                    .is_some_and(|token| is_punctuation(token, '@'))
                    && clauses
                        .get(cursor + 1)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Atom(_)));
                if !valid_symbol {
                    diagnostics.push(
                        Diagnostic::error(VFY16, "catch.filter requires a function symbol")
                            .with_primary_span(clause.span),
                    );
                    break;
                }
                cursor += 2;
            }
            "catch.all" => {}
            "cleanup" => {
                if saw_cleanup {
                    diagnostics.push(
                        Diagnostic::error(VFY16, "landingpad has more than one cleanup clause")
                            .with_primary_span(clause.span),
                    );
                }
                saw_cleanup = true;
            }
            other => {
                diagnostics.push(
                    Diagnostic::error(VFY16, format!("unknown landingpad clause '{other}'"))
                        .with_primary_span(clause.span),
                );
                break;
            }
        }
        if cursor < clauses.len() {
            if is_punctuation(&clauses[cursor], ',') {
                cursor += 1;
                if cursor == clauses.len() {
                    diagnostics.push(
                        Diagnostic::error(VFY16, "trailing comma in landingpad clauses")
                            .with_primary_span(clauses[cursor - 1].span),
                    );
                    break;
                }
            } else {
                diagnostics.push(
                    Diagnostic::error(VFY16, "expected ',' between landingpad clauses")
                        .with_primary_span(clauses[cursor].span),
                );
                break;
            }
        }
    }
    diagnostics
}

fn invoke_unwind_target(instruction: &TextInstruction) -> Option<(String, Span)> {
    let start = instruction_operands_start(&instruction.tokens);
    let tokens = &instruction.tokens[start..];
    let keyword = tokens.iter().enumerate().position(|(index, token)| {
        is_atom(token, "unwind")
            && !index.checked_sub(1).is_some_and(|previous| {
                matches!(
                    tokens[previous].kind,
                    TokenKind::Punctuation('%' | '@' | '$' | '.')
                )
            })
    })?;
    let target = tokens.get(keyword + 1)?;
    let TokenKind::Atom(name) = &target.kind else {
        return None;
    };
    Some((name.clone(), target.span))
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

fn integer_literal(token: &Token) -> Option<u64> {
    let TokenKind::Atom(value) = &token.kind else {
        return None;
    };
    u64::try_from(tir_format::numeric_literals::parse_unsigned_integer(value)?).ok()
}

fn is_atom(token: &Token, expected: &str) -> bool {
    matches!(&token.kind, TokenKind::Atom(value) if value == expected)
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
