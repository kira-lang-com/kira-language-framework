use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction};

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);

#[derive(Clone)]
pub(crate) struct Successor {
    pub(crate) label: String,
    pub(crate) argument_count: Option<usize>,
    pub(crate) arguments: Vec<String>,
    pub(crate) span: tir_diag::Span,
}

#[derive(Clone)]
pub(crate) struct TextEdge {
    pub(crate) target: usize,
    pub(crate) arguments: Vec<String>,
}

/// Returns the block indices reached by each block's control-flow terminators.
/// Unknown targets are omitted here and reported by the CFG verifier.
pub fn text_successors(body: &FunctionBody) -> Vec<Vec<usize>> {
    text_edges(body)
        .into_iter()
        .map(|edges| edges.into_iter().map(|edge| edge.target).collect())
        .collect()
}

pub(crate) fn conditional_successor_count(instruction: &TextInstruction) -> Option<usize> {
    matches!(instruction.opcode.as_str(), "cond.br" | "switch.int")
        .then(|| successors_of(instruction).len())
}

pub(crate) fn instruction_targets(
    body: &FunctionBody,
    instruction: &TextInstruction,
) -> Vec<usize> {
    let mut labels = HashMap::with_capacity(body.blocks.len());
    for (index, block) in body.blocks.iter().enumerate() {
        labels.entry(block.label.as_str()).or_insert(index);
    }
    successors_of(instruction)
        .into_iter()
        .filter_map(|successor| labels.get(successor.label.as_str()).copied())
        .collect()
}

pub(crate) fn text_edges(body: &FunctionBody) -> Vec<Vec<TextEdge>> {
    let mut labels = HashMap::with_capacity(body.blocks.len());
    for (index, block) in body.blocks.iter().enumerate() {
        labels.entry(block.label.as_str()).or_insert(index);
    }
    body.blocks
        .iter()
        .map(|block| {
            block
                .instructions
                .iter()
                .filter(|instruction| is_control_terminator(&instruction.opcode))
                .flat_map(successors_of)
                .filter_map(|successor| {
                    labels
                        .get(successor.label.as_str())
                        .copied()
                        .map(|target| TextEdge {
                            target,
                            arguments: successor.arguments,
                        })
                })
                .collect()
        })
        .collect()
}

pub(crate) fn verify_text_cfg(body: &FunctionBody, enforce_structure: bool) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut labels = HashMap::with_capacity(body.blocks.len());
    for (index, block) in body.blocks.iter().enumerate() {
        labels.entry(block.label.as_str()).or_insert(index);
    }
    let successors = body
        .blocks
        .iter()
        .map(|block| {
            let mut edges = Vec::new();
            for instruction in &block.instructions {
                if is_control_terminator(&instruction.opcode) {
                    edges.extend(successors_of(instruction));
                }
            }
            edges
        })
        .collect::<Vec<_>>();

    for edges in &successors {
        for edge in edges {
            let Some(target_index) = labels.get(edge.label.as_str()).copied() else {
                diagnostics.push(
                    Diagnostic::error(V1, format!("branch targets unknown block {}", edge.label))
                        .with_primary_span(edge.span),
                );
                continue;
            };
            let target = &body.blocks[target_index];
            if edge
                .argument_count
                .is_some_and(|count| count != target.parameters.len())
            {
                diagnostics.push(
                    Diagnostic::error(
                        V1,
                        format!(
                            "edge to {} passes {} arguments, expected {}",
                            edge.label,
                            edge.argument_count.unwrap_or_default(),
                            target.parameters.len()
                        ),
                    )
                    .with_primary_span(edge.span),
                );
            }
        }
    }

    verify_rpo(body, &successors, &labels, &mut diagnostics);
    crate::structured::verify_structured(
        body,
        &successors,
        &labels,
        enforce_structure,
        &mut diagnostics,
    );
    diagnostics
}

fn verify_rpo(
    body: &FunctionBody,
    successors: &[Vec<Successor>],
    labels: &HashMap<&str, usize>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let ordered_successors = successors
        .iter()
        .map(|edges| {
            let mut targets = edges
                .iter()
                .filter_map(|successor| labels.get(successor.label.as_str()).copied())
                .collect::<Vec<_>>();
            targets.sort_unstable();
            targets.dedup();
            targets
        })
        .collect::<Vec<_>>();
    let mut visited = vec![false; body.blocks.len()];
    let mut order = Vec::with_capacity(body.blocks.len());
    for root in 0..body.blocks.len() {
        if visited[root] {
            continue;
        }
        let mut postorder = Vec::new();
        visit_postorder(root, &ordered_successors, &mut visited, &mut postorder);
        order.extend(postorder.into_iter().rev());
    }
    if let Some((actual, expected)) = order
        .iter()
        .copied()
        .enumerate()
        .find(|(expected, actual)| *expected != *actual)
    {
        diagnostics.push(
            Diagnostic::error(
                V1,
                format!(
                    "blocks are not in reverse post-order: {} appears at index {actual}, expected {expected}",
                    body.blocks[actual].label
                ),
            )
            .with_primary_span(body.blocks[actual].span),
        );
    }
}

fn visit_postorder(
    block: usize,
    successors: &[Vec<usize>],
    visited: &mut [bool],
    postorder: &mut Vec<usize>,
) {
    visited[block] = true;
    let mut stack = Vec::with_capacity(successors.len().min(1024));
    stack.push((block, 0usize));
    while let Some((current, next_successor)) = stack.last().copied() {
        let next = successors[current].get(next_successor).copied();
        if let Some(target) = next {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
            if !visited[target] {
                visited[target] = true;
                stack.push((target, 0));
            }
        } else {
            postorder.push(current);
            stack.pop();
        }
    }
}

fn successors_of(instruction: &TextInstruction) -> Vec<Successor> {
    let tokens = &instruction.tokens;
    let operands = &tokens[instruction_operands_start(tokens)..];
    let operand_end = operands
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('!')))
        .unwrap_or(operands.len());
    let operands = &operands[..operand_end];
    match instruction.opcode.as_str() {
        "br" | "br.indirect" => {
            let mut targets = if instruction.opcode == "br" {
                first_target(operands).into_iter().collect()
            } else {
                bracket_targets(operands)
            };
            set_implicit_zero_arguments(&mut targets);
            targets
        }
        "cond.br" => split_top_level(operands, ',')
            .into_iter()
            .skip(1)
            .filter_map(first_target)
            .map(zero_when_implicit)
            .collect(),
        "switch.int" => switch_targets(operands)
            .into_iter()
            .map(zero_when_implicit)
            .collect(),
        "switch.enum" => switch_targets(operands)
            .into_iter()
            .map(implicit_arguments)
            .collect(),
        "invoke" => keyword_targets(operands, &["to", "unwind"])
            .into_iter()
            .map(zero_when_implicit)
            .collect(),
        "try.call" => keyword_targets(operands, &["normal", "error"])
            .into_iter()
            .map(implicit_arguments)
            .collect(),
        "coro.resume" => keyword_targets(operands, &["yielded", "done"])
            .into_iter()
            .map(implicit_arguments)
            .collect(),
        "coro.yield" => keyword_targets(operands, &["resume", "destroy"])
            .into_iter()
            .map(implicit_arguments)
            .collect(),
        "asm.goto" => assembly_targets(operands)
            .into_iter()
            .map(zero_when_implicit)
            .collect(),
        _ => Vec::new(),
    }
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

fn first_target(tokens: &[Token]) -> Option<Successor> {
    let index = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Atom(_)))?;
    target_at(tokens, index)
}

fn target_at(tokens: &[Token], index: usize) -> Option<Successor> {
    let token = tokens.get(index)?;
    let TokenKind::Atom(label) = &token.kind else {
        return None;
    };
    let argument_count = if matches!(
        tokens.get(index + 1).map(|next| &next.kind),
        Some(TokenKind::Punctuation('('))
    ) {
        Some(list_count(tokens, index + 1, '(', ')')?)
    } else {
        None
    };
    let arguments = if argument_count.is_some() {
        list_values(tokens, index + 1, '(', ')')?
    } else {
        Vec::new()
    };
    Some(Successor {
        label: label.clone(),
        argument_count,
        arguments,
        span: token.span,
    })
}

fn zero_when_implicit(mut successor: Successor) -> Successor {
    successor.argument_count.get_or_insert(0);
    successor
}

fn implicit_arguments(mut successor: Successor) -> Successor {
    successor.argument_count = None;
    successor.arguments.clear();
    successor
}

fn set_implicit_zero_arguments(successors: &mut [Successor]) {
    for successor in successors {
        successor.argument_count.get_or_insert(0);
    }
}

fn keyword_targets(tokens: &[Token], keywords: &[&str]) -> Vec<Successor> {
    let mut successors = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let is_operand_name = index.checked_sub(1).is_some_and(|previous| {
            matches!(
                tokens[previous].kind,
                TokenKind::Punctuation('%' | '@' | '$' | '.')
            )
        });
        if !is_operand_name
            && matches!(&token.kind, TokenKind::Atom(atom) if keywords.contains(&atom.as_str()))
            && let Some(successor) = target_at(tokens, index + 1)
        {
            successors.push(successor);
        }
    }
    successors
}

fn switch_targets(tokens: &[Token]) -> Vec<Successor> {
    let mut successors = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if matches!(token.kind, TokenKind::Punctuation(':'))
            && let Some(successor) = target_at(tokens, index + 1)
        {
            successors.push(successor);
        } else if matches!(&token.kind, TokenKind::Atom(atom) if atom == "default")
            && let Some(successor) = target_at(tokens, index + 1)
        {
            successors.push(successor);
        }
    }
    successors
}

fn bracket_targets(tokens: &[Token]) -> Vec<Successor> {
    let Some(open) = tokens
        .iter()
        .position(|token| matches!(token.kind, TokenKind::Punctuation('[')))
    else {
        return Vec::new();
    };
    let Some(close) = matching_delimiters(&tokens[open..], '[', ']') else {
        return Vec::new();
    };
    let close = open + close;
    split_top_level(&tokens[open + 1..close], ',')
        .into_iter()
        .filter_map(first_target)
        .collect()
}

fn assembly_targets(tokens: &[Token]) -> Vec<Successor> {
    let mut successors = keyword_targets(tokens, &["normal"]);
    if let Some(index) = tokens
        .iter()
        .position(|token| matches!(&token.kind, TokenKind::Atom(atom) if atom == "labels"))
    {
        successors.extend(bracket_targets(&tokens[index + 1..]));
    }
    successors
}

fn split_top_level(tokens: &[Token], separator: char) -> Vec<&[Token]> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut depth = [0i32; 4];
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => depth[0] += 1,
                ')' => depth[0] -= 1,
                '[' => depth[1] += 1,
                ']' => depth[1] -= 1,
                '{' => depth[2] += 1,
                '}' => depth[2] -= 1,
                '<' => depth[3] += 1,
                '>' => depth[3] -= 1,
                value if value == separator && depth == [0; 4] => {
                    segments.push(&tokens[start..index]);
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    segments.push(&tokens[start..]);
    segments
}

fn list_count(tokens: &[Token], open: usize, left: char, right: char) -> Option<usize> {
    let close = matching_delimiters(&tokens[open..], left, right)?.checked_add(open)?;
    let items = &tokens[open + 1..close];
    if items.is_empty() {
        Some(0)
    } else {
        Some(split_top_level(items, ',').len())
    }
}

fn list_values(tokens: &[Token], open: usize, left: char, right: char) -> Option<Vec<String>> {
    let close = matching_delimiters(&tokens[open..], left, right)?.checked_add(open)?;
    let items = &tokens[open + 1..close];
    if items.is_empty() {
        return Some(Vec::new());
    }
    Some(
        split_top_level(items, ',')
            .into_iter()
            .filter_map(|item| {
                match (
                    item.first().map(|token| &token.kind),
                    item.get(1).map(|token| &token.kind),
                ) {
                    (Some(TokenKind::Punctuation('%')), Some(TokenKind::Atom(name))) => {
                        Some(name.clone())
                    }
                    _ => None,
                }
            })
            .collect(),
    )
}

fn matching_delimiters(tokens: &[Token], left: char, right: char) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = token.kind {
            if character == left {
                depth += 1;
            } else if character == right {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
    }
    None
}

pub(crate) fn is_control_terminator(opcode: &str) -> bool {
    matches!(
        opcode,
        "br" | "cond.br"
            | "switch.int"
            | "switch.enum"
            | "br.indirect"
            | "invoke"
            | "try.call"
            | "coro.resume"
            | "coro.yield"
            | "asm.goto"
    )
}
