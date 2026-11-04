use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction};
use tir_types::Type;

use crate::calls::opcode_end;

const VFY1: Code = Code::new(Dialect::Tvisa, Area::Verifier, 1);

struct EdgeValue {
    name: String,
    span: Span,
}

pub(super) fn verify_function(
    body: &FunctionBody,
    value_types: &HashMap<String, Type>,
) -> Vec<Diagnostic> {
    let blocks: HashMap<_, _> = body
        .blocks
        .iter()
        .map(|block| (block.label.as_str(), block))
        .collect();
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if !is_edge_terminator(&instruction.opcode) || instruction.opcode == "invoke" {
            continue;
        }
        for index in opcode_end(instruction)..instruction.tokens.len() {
            let Some(label) = atom(&instruction.tokens[index]) else {
                continue;
            };
            if is_symbol_or_value_name(instruction, index) {
                continue;
            }
            let Some(target) = blocks.get(label) else {
                continue;
            };
            let Some((arguments, valid)) = edge_arguments(&instruction.tokens, index + 1) else {
                continue;
            };
            if !valid {
                diagnostics.push(
                    Diagnostic::error(VFY1, "block edge arguments must be SSA values")
                        .with_primary_span(instruction.span),
                );
                continue;
            }
            if arguments.len() != target.parameters.len() {
                continue;
            }
            for (argument, parameter) in arguments.iter().zip(&target.parameters) {
                if let Some(actual) = value_types.get(&argument.name)
                    && actual != &parameter.ty
                {
                    diagnostics.push(
                        Diagnostic::error(
                            VFY1,
                            format!(
                                "edge argument %{} has type {actual}; block {} requires {}",
                                argument.name, target.label, parameter.ty
                            ),
                        )
                        .with_primary_span(argument.span),
                    );
                }
            }
        }
    }
    diagnostics
}

fn edge_arguments(tokens: &[Token], after_target: usize) -> Option<(Vec<EdgeValue>, bool)> {
    if !tokens
        .get(after_target)
        .is_some_and(|token| is_punctuation(token, '('))
    {
        return Some((Vec::new(), true));
    }
    let close = matching_paren(tokens, after_target)?;
    let mut arguments = Vec::new();
    let mut valid = true;
    let mut start = after_target + 1;
    for index in after_target + 1..close {
        if is_punctuation(&tokens[index], ',') {
            valid &= push_value(&mut arguments, &tokens[start..index]);
            start = index + 1;
        }
    }
    if start < close {
        valid &= push_value(&mut arguments, &tokens[start..close]);
    } else if start != after_target + 1 {
        valid = false;
    }
    Some((arguments, valid))
}

fn push_value(values: &mut Vec<EdgeValue>, tokens: &[Token]) -> bool {
    if tokens.len() != 2 || !is_punctuation(&tokens[0], '%') {
        return false;
    }
    let Some(name) = atom(&tokens[1]) else {
        return false;
    };
    values.push(EdgeValue {
        name: name.to_owned(),
        span: Span::new(tokens[0].span.start, tokens[1].span.end),
    });
    true
}

fn matching_paren(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            TokenKind::Punctuation('(') => depth += 1,
            TokenKind::Punctuation(')') => {
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

fn is_edge_terminator(opcode: &str) -> bool {
    matches!(
        opcode,
        "br" | "cond.br" | "switch.int" | "switch.enum" | "br.indirect" | "invoke" | "asm.goto"
    )
}

fn is_symbol_or_value_name(instruction: &TextInstruction, index: usize) -> bool {
    index.checked_sub(1).is_some_and(|previous| {
        matches!(
            instruction.tokens[previous].kind,
            TokenKind::Punctuation('%' | '@' | '$' | '.')
        )
    })
}

fn atom(token: &Token) -> Option<&str> {
    if let TokenKind::Atom(value) = &token.kind {
        Some(value)
    } else {
        None
    }
}

fn is_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(value) if value == expected)
}
