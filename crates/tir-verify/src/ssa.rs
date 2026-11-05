use std::collections::HashMap;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_format::text::{Token, TokenKind};
use tir_format::{FunctionBody, TextInstruction};
use tir_types::Type;

use crate::dominators::DominatorTree;
use crate::model::{Function, TerminatorKind, ValueDefinition, ValueUse};

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);
const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const V3: Code = Code::new(Dialect::Tir, Area::Verifier, 3);
const S7: Code = Code::new(Dialect::Tir, Area::ControlFlow, 7);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VerifyOptions {
    pub structured: bool,
    pub gpu: bool,
}

#[derive(Clone)]
struct DefinitionSite {
    block: usize,
    position: usize,
    ty: Type,
    span: Span,
}

pub fn verify_function(function: &Function, options: VerifyOptions) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if function.blocks.is_empty() {
        diagnostics.push(Diagnostic::error(V3, "function body has no basic blocks"));
        return diagnostics;
    }

    let mut labels = HashMap::with_capacity(function.blocks.len());
    for (index, block) in function.blocks.iter().enumerate() {
        if let Some(previous) = labels.insert(block.label.as_str(), index) {
            diagnostics.push(
                Diagnostic::error(V1, format!("duplicate block label {}", block.label))
                    .with_primary_span(block.span)
                    .with_note_at(function.blocks[previous].span, "the first block is here"),
            );
        }
    }

    let entry = &function.blocks[0];
    for parameter_type in &function.parameters {
        verify_type(parameter_type, entry.span, &mut diagnostics);
    }
    if function.parameters.len() != entry.parameters.len() {
        diagnostics.push(
            Diagnostic::error(
                V1,
                "entry block parameters do not match the function signature",
            )
            .with_primary_span(entry.span),
        );
    } else {
        for (signature_type, entry_parameter) in function.parameters.iter().zip(&entry.parameters) {
            if signature_type != &entry_parameter.ty {
                diagnostics.push(
                    Diagnostic::error(
                        V1,
                        format!(
                            "entry parameter %{} has type {}, expected {}",
                            entry_parameter.name, entry_parameter.ty, signature_type
                        ),
                    )
                    .with_primary_span(entry_parameter.span),
                );
            }
        }
    }

    let mut definitions = HashMap::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        for parameter in &block.parameters {
            verify_type(&parameter.ty, parameter.span, &mut diagnostics);
            record_definition(
                &mut definitions,
                parameter,
                block_index,
                0,
                &mut diagnostics,
            );
        }
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            diagnostics.extend(crate::instruction_types::verify_instruction(instruction));
            if crate::is_terminator(&instruction.opcode) {
                diagnostics.push(
                    Diagnostic::error(
                        V3,
                        format!(
                            "terminator {} appears in the ordinary instruction list",
                            instruction.opcode
                        ),
                    )
                    .with_primary_span(instruction.span),
                );
            }
            for operand in &instruction.operands {
                verify_type(&operand.ty, operand.span, &mut diagnostics);
            }
            for result in &instruction.results {
                verify_type(&result.ty, result.span, &mut diagnostics);
                record_definition(
                    &mut definitions,
                    result,
                    block_index,
                    instruction_index + 1,
                    &mut diagnostics,
                );
            }
        }
        match &block.terminator {
            None => diagnostics.push(
                Diagnostic::error(V3, format!("block {} has no terminator", block.label))
                    .with_primary_span(block.span),
            ),
            Some(terminator) => {
                if !edge_count_valid(terminator.kind, terminator.edges.len()) {
                    diagnostics.push(
                        Diagnostic::error(
                            V3,
                            format!(
                                "{:?} terminator has an invalid number of successors",
                                terminator.kind
                            ),
                        )
                        .with_primary_span(terminator.span),
                    );
                }
                for operand in &terminator.operands {
                    verify_type(&operand.ty, operand.span, &mut diagnostics);
                }
                if (options.structured || options.gpu)
                    && terminator.kind.forbidden_in_structured_control_flow()
                {
                    diagnostics.push(
                        Diagnostic::error(
                            S7,
                            format!(
                                "{:?} is forbidden in structured control flow",
                                terminator.kind
                            ),
                        )
                        .with_primary_span(terminator.span),
                    );
                }
            }
        }
    }

    let successors = function
        .blocks
        .iter()
        .map(|block| {
            block
                .terminator
                .iter()
                .flat_map(|terminator| &terminator.edges)
                .filter_map(|edge| labels.get(edge.target.as_str()).copied())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let dominators = DominatorTree::compute(&successors);
    for (block_index, block) in function.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            verify_uses(
                &instruction.operands,
                block_index,
                instruction_index + 1,
                &definitions,
                &dominators,
                &mut diagnostics,
            );
        }
        let Some(terminator) = &block.terminator else {
            continue;
        };
        let terminator_position = block.instructions.len() + 1;
        verify_uses(
            &terminator.operands,
            block_index,
            terminator_position,
            &definitions,
            &dominators,
            &mut diagnostics,
        );
        for edge in &terminator.edges {
            verify_uses(
                &edge.arguments,
                block_index,
                terminator_position,
                &definitions,
                &dominators,
                &mut diagnostics,
            );
            let Some(target_index) = labels.get(edge.target.as_str()).copied() else {
                diagnostics.push(
                    Diagnostic::error(V1, format!("branch targets unknown block {}", edge.target))
                        .with_primary_span(terminator.span),
                );
                continue;
            };
            let target = &function.blocks[target_index];
            if edge.arguments.len() != target.parameters.len() {
                diagnostics.push(
                    Diagnostic::error(
                        V1,
                        format!(
                            "edge to {} passes {} arguments, expected {}",
                            edge.target,
                            edge.arguments.len(),
                            target.parameters.len()
                        ),
                    )
                    .with_primary_span(terminator.span),
                );
                continue;
            }
            for (argument, parameter) in edge.arguments.iter().zip(&target.parameters) {
                if argument.ty != parameter.ty {
                    diagnostics.push(
                        Diagnostic::error(
                            V1,
                            format!(
                                "edge argument to %{} has type {}, expected {}",
                                parameter.name, argument.ty, parameter.ty
                            ),
                        )
                        .with_primary_span(argument.span),
                    );
                }
            }
        }
    }

    verify_reverse_postorder(
        function,
        &successors,
        dominators.components(),
        dominators.roots(),
        &mut diagnostics,
    );
    diagnostics
}

pub fn verify_text_value_dominance(body: &FunctionBody) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if body.blocks.is_empty() {
        return diagnostics;
    }
    let successors = crate::text_successors(body);
    let dominators = DominatorTree::compute(&successors);
    let mut definitions = HashMap::new();
    for (block_index, block) in body.blocks.iter().enumerate() {
        for parameter in &block.parameters {
            definitions.entry(parameter.name.as_str()).or_insert((
                block_index,
                0usize,
                parameter.span,
            ));
        }
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            for result in &instruction.results {
                definitions.entry(result.as_str()).or_insert((
                    block_index,
                    instruction_index + 1,
                    instruction.span,
                ));
            }
        }
    }

    for (block_index, block) in body.blocks.iter().enumerate() {
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            let use_position = instruction_index + 1;
            for (name, span) in text_value_uses(instruction) {
                let Some((definition_block, definition_position, _)) =
                    definitions.get(name.as_str())
                else {
                    diagnostics.push(
                        Diagnostic::error(V1, format!("value %{name} is used but not defined"))
                            .with_primary_span(span),
                    );
                    continue;
                };
                let dominates = if *definition_block == block_index {
                    *definition_position < use_position
                } else {
                    dominators.dominates(*definition_block, block_index)
                };
                if !dominates {
                    diagnostics.push(
                        Diagnostic::error(
                            V1,
                            format!("definition of %{name} does not dominate this use"),
                        )
                        .with_primary_span(span),
                    );
                }
            }
        }
    }
    diagnostics
}

pub(super) fn text_value_uses(instruction: &TextInstruction) -> Vec<(String, Span)> {
    if instruction.opcode == "func.ref" {
        return Vec::new();
    }
    let start = aggregate_value_list_start(instruction)
        .unwrap_or_else(|| text_instruction_operands_start(&instruction.tokens));
    let mut uses = Vec::new();
    let mut cursor = start;
    while cursor + 1 < instruction.tokens.len() {
        if matches!(instruction.tokens[cursor].kind, TokenKind::Punctuation('%'))
            && let TokenKind::Atom(name) = &instruction.tokens[cursor + 1].kind
        {
            uses.push((name.clone(), instruction.tokens[cursor + 1].span));
            cursor += 2;
        } else {
            cursor += 1;
        }
    }
    uses
}

fn aggregate_value_list_start(instruction: &TextInstruction) -> Option<usize> {
    let start = text_instruction_operands_start(&instruction.tokens);
    if !matches!(
        instruction.opcode.as_str(),
        "struct.new" | "array.new" | "enum.new"
    ) {
        return None;
    }
    let mut nesting = Vec::new();
    let mut saw_enum_comma = false;
    for (index, token) in instruction.tokens.iter().enumerate().skip(start) {
        if matches!(instruction.opcode.as_str(), "struct.new" | "array.new")
            && has_punctuation(token, '(')
            && nesting.is_empty()
        {
            return Some(index + 1);
        }
        if instruction.opcode == "enum.new" && has_punctuation(token, ',') && nesting.is_empty() {
            saw_enum_comma = true;
            continue;
        }
        if instruction.opcode == "enum.new"
            && saw_enum_comma
            && has_punctuation(token, '(')
            && nesting.is_empty()
        {
            return Some(index + 1);
        }
        match &token.kind {
            TokenKind::Punctuation(character @ ('<' | '(' | '[' | '{')) => {
                nesting.push(*character);
            }
            TokenKind::Punctuation(character @ ('>' | ')' | ']' | '}')) => {
                let expected = match *character {
                    '>' => '<',
                    ')' => '(',
                    ']' => '[',
                    '}' => '{',
                    _ => continue,
                };
                if nesting.last() == Some(&expected) {
                    nesting.pop();
                }
            }
            _ => {}
        }
    }
    None
}

fn has_punctuation(token: &Token, expected: char) -> bool {
    matches!(token.kind, TokenKind::Punctuation(character) if character == expected)
}

pub(super) fn text_instruction_operands_start(tokens: &[Token]) -> usize {
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

fn verify_type(ty: &Type, span: Span, diagnostics: &mut Vec<Diagnostic>) {
    if let Err(error) = ty.validate() {
        diagnostics.push(Diagnostic::error(V2, error.to_string()).with_primary_span(span));
    }
}

fn edge_count_valid(kind: TerminatorKind, count: usize) -> bool {
    match kind {
        TerminatorKind::Branch => count == 1,
        TerminatorKind::ConditionalBranch
        | TerminatorKind::TryCall
        | TerminatorKind::Invoke
        | TerminatorKind::CoroutineYield
        | TerminatorKind::CoroutineResume => count == 2,
        TerminatorKind::SwitchInteger
        | TerminatorKind::SwitchEnum
        | TerminatorKind::IndirectBranch
        | TerminatorKind::AssemblyGoto => count != 0,
        TerminatorKind::Return
        | TerminatorKind::Fail
        | TerminatorKind::TailCall
        | TerminatorKind::Unreachable
        | TerminatorKind::Trap
        | TerminatorKind::CoroutineFinish
        | TerminatorKind::Resume
        | TerminatorKind::Throw
        | TerminatorKind::LongJump
        | TerminatorKind::Deopt
        | TerminatorKind::IgnoreHit
        | TerminatorKind::AcceptHit => count == 0,
    }
}

fn record_definition(
    definitions: &mut HashMap<String, DefinitionSite>,
    definition: &ValueDefinition,
    block: usize,
    position: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(previous) = definitions.get(&definition.name) {
        diagnostics.push(
            Diagnostic::error(
                V1,
                format!("SSA value %{} is defined more than once", definition.name),
            )
            .with_primary_span(definition.span)
            .with_note_at(previous.span, "the first definition is here"),
        );
    } else {
        definitions.insert(
            definition.name.clone(),
            DefinitionSite {
                block,
                position,
                ty: definition.ty.clone(),
                span: definition.span,
            },
        );
    }
}

fn verify_uses(
    uses: &[ValueUse],
    block: usize,
    position: usize,
    definitions: &HashMap<String, DefinitionSite>,
    dominators: &DominatorTree,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for usage in uses {
        let Some(definition) = definitions.get(&usage.name) else {
            diagnostics.push(
                Diagnostic::error(V1, format!("use of undefined SSA value %{}", usage.name))
                    .with_primary_span(usage.span),
            );
            continue;
        };
        if definition.block == block {
            if definition.position >= position {
                diagnostics.push(
                    Diagnostic::error(
                        V1,
                        format!("SSA value %{} is used before its definition", usage.name),
                    )
                    .with_primary_span(usage.span)
                    .with_note_at(definition.span, "the definition is here"),
                );
            }
        } else if !dominators.dominates(definition.block, block) {
            diagnostics.push(
                Diagnostic::error(
                    V1,
                    format!("definition of %{} does not dominate this use", usage.name),
                )
                .with_primary_span(usage.span)
                .with_note_at(definition.span, "the definition is here"),
            );
        }
        if definition.ty != usage.ty {
            diagnostics.push(
                Diagnostic::error(
                    V2,
                    format!(
                        "use of %{} has type {}, expected {}",
                        usage.name, usage.ty, definition.ty
                    ),
                )
                .with_primary_span(usage.span)
                .with_note_at(definition.span, "the value is defined here"),
            );
        }
    }
}

fn verify_reverse_postorder(
    function: &Function,
    successors: &[Vec<usize>],
    component: &[usize],
    roots: &[usize],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut visited = vec![false; function.blocks.len()];
    let mut order = Vec::with_capacity(function.blocks.len());
    for root in roots {
        let mut postorder = Vec::new();
        visit_postorder(*root, successors, component, &mut visited, &mut postorder);
        order.extend(postorder.into_iter().rev());
    }
    for (expected, actual) in order.iter().copied().enumerate() {
        if expected != actual {
            diagnostics.push(
                Diagnostic::error(
                    V1,
                    format!(
                        "blocks are not in reverse post-order: {} appears at index {actual}, expected {expected}",
                        function.blocks[actual].label
                    ),
                )
                .with_primary_span(function.blocks[actual].span),
            );
            break;
        }
    }
}

fn visit_postorder(
    block: usize,
    successors: &[Vec<usize>],
    component: &[usize],
    visited: &mut [bool],
    postorder: &mut Vec<usize>,
) {
    visited[block] = true;
    let mut stack = vec![(block, 0usize)];
    while let Some((current, next_successor)) = stack.last().copied() {
        let next = successors[current]
            .get(next_successor)
            .copied()
            .filter(|target| component[*target] == component[current]);
        if let Some(target) = next {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
            if !visited[target] {
                visited[target] = true;
                stack.push((target, 0));
            }
        } else if next_successor < successors[current].len() {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
        } else {
            postorder.push(current);
            stack.pop();
        }
    }
}

#[must_use]
pub const fn terminator_is_s7_forbidden(kind: TerminatorKind) -> bool {
    kind.forbidden_in_structured_control_flow()
}
