use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryInstruction, BinaryOperand};

use crate::DominatorTree;

const V1: Code = Code::new(Dialect::Tir, Area::Verifier, 1);
const V4: Code = Code::new(Dialect::Tir, Area::Verifier, 4);
const S7: Code = Code::new(Dialect::Tir, Area::ControlFlow, 7);

#[derive(Clone, Copy)]
struct Definition {
    block: usize,
    position: usize,
}

pub(super) fn verify_body(
    body: &tir_format::BinaryFunctionBody,
    value_types: &[Option<u64>],
    structured: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let successors = body
        .blocks
        .iter()
        .map(|block| {
            let mut targets = Vec::new();
            if let Some(instruction) = block.instructions.as_ref().and_then(|items| items.last()) {
                visit_edges(instruction, |target, _| {
                    if let Ok(target) = usize::try_from(target) {
                        targets.push(target);
                    }
                });
            }
            targets
        })
        .collect::<Vec<_>>();
    let dominators = DominatorTree::compute(&successors);
    verify_reverse_postorder(body, &successors, &dominators, &mut diagnostics);
    verify_edges(body, value_types, &mut diagnostics);
    let graph_blocks = body
        .blocks
        .iter()
        .map(|block| crate::StructuredGraphBlock {
            loop_merge: block
                .loop_merge
                .and_then(|index| usize::try_from(index).ok()),
            loop_continue: block
                .loop_continue
                .and_then(|index| usize::try_from(index).ok()),
            selection: block
                .instructions
                .as_ref()
                .and_then(|items| items.last())
                .is_some_and(|instruction| matches!(instruction.opcode, 0x1e1 | 0x1e2)),
            selection_merge: block
                .instructions
                .as_ref()
                .and_then(|items| items.last())
                .and_then(selection_merge),
        })
        .collect::<Vec<_>>();
    let structured = structured
        || graph_blocks.iter().any(|block| {
            block.loop_merge.is_some()
                || block.loop_continue.is_some()
                || block.selection_merge.is_some()
        });
    if structured {
        diagnostics.extend(crate::verify_structured_graph(
            &graph_blocks,
            &successors,
            Dialect::Tir,
        ));
        diagnostics.extend(
            body.blocks
                .iter()
                .filter_map(|block| block.instructions.as_ref())
                .flatten()
                .filter(|instruction| {
                    matches!(
                        instruction.opcode,
                        0x1aa | 0x1d0..=0x1d2 | 0x1e4 | 0x214..=0x215
                    )
                })
                .map(|instruction| {
                    Diagnostic::error(
                        S7,
                        format!(
                            "instruction opcode 0x{:03X} is forbidden by structured control flow",
                            instruction.opcode
                        ),
                    )
                }),
        );
    }

    let definitions = collect_definitions(body);
    for (block_index, block) in body.blocks.iter().enumerate() {
        let Some(instructions) = &block.instructions else {
            continue;
        };
        for (instruction_index, instruction) in instructions.iter().enumerate() {
            let use_position = instruction_index + 1;
            visit_values(&instruction.operands, &mut |value| {
                verify_use(
                    value,
                    block_index,
                    use_position,
                    &definitions,
                    &dominators,
                    &mut diagnostics,
                );
            });
        }
    }
    diagnostics
}

fn selection_merge(instruction: &BinaryInstruction) -> Option<usize> {
    let encoded = instruction
        .operands
        .iter()
        .rev()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })?;
    usize::try_from(encoded.checked_sub(1)?).ok()
}

fn verify_edges(
    body: &tir_format::BinaryFunctionBody,
    value_types: &[Option<u64>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (source, block) in body.blocks.iter().enumerate() {
        let Some(terminator) = block.instructions.as_ref().and_then(|items| items.last()) else {
            continue;
        };
        if matches!(terminator.opcode, 0x1a7 | 0x1aa | 0x1c1 | 0x1c2 | 0x1e3) {
            continue;
        }
        visit_edges(terminator, |target, arguments| {
            let Some(target_block) = usize::try_from(target)
                .ok()
                .and_then(|index| body.blocks.get(index))
            else {
                return;
            };
            if arguments.len() != target_block.parameters.len() {
                diagnostics.push(Diagnostic::error(
                    V1,
                    format!(
                        "edge from block {source} to block {target} passes {} arguments, expected {}",
                        arguments.len(),
                        target_block.parameters.len()
                    ),
                ));
                return;
            }
            for (argument, parameter) in arguments.iter().zip(&target_block.parameters) {
                if let Some(actual) = usize::try_from(*argument)
                    .ok()
                    .and_then(|index| value_types.get(index))
                    .copied()
                    .flatten()
                    && actual != parameter.ty
                {
                    diagnostics.push(Diagnostic::error(
                        V1,
                        format!(
                            "edge from block {source} to block {target} passes type {actual}, expected {}",
                            parameter.ty
                        ),
                    ));
                }
            }
        });
    }
}

fn collect_definitions(body: &tir_format::BinaryFunctionBody) -> Vec<Definition> {
    let capacity = body
        .blocks
        .iter()
        .map(|block| {
            block.parameters.len()
                + block
                    .instructions
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .map(|instruction| instruction.result_count as usize)
                    .sum::<usize>()
        })
        .sum();
    let mut definitions = Vec::with_capacity(capacity);
    for (block_index, block) in body.blocks.iter().enumerate() {
        definitions.extend((0..block.parameters.len()).map(|_| Definition {
            block: block_index,
            position: 0,
        }));
        if let Some(instructions) = &block.instructions {
            for (instruction_index, instruction) in instructions.iter().enumerate() {
                definitions.extend((0..instruction.result_count).map(|_| Definition {
                    block: block_index,
                    position: instruction_index + 1,
                }));
            }
        }
    }
    definitions
}

fn verify_use(
    value: u64,
    block: usize,
    position: usize,
    definitions: &[Definition],
    dominators: &DominatorTree,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(definition) = usize::try_from(value)
        .ok()
        .and_then(|index| definitions.get(index))
    else {
        diagnostics.push(Diagnostic::error(
            V1,
            format!("value {value} is used but has no definition"),
        ));
        return;
    };
    let dominates = if definition.block == block {
        definition.position < position
    } else {
        dominators.dominates(definition.block, block)
    };
    if !dominates {
        diagnostics.push(Diagnostic::error(
            V4,
            format!("definition of value {value} does not dominate its use in block {block}"),
        ));
    }
}

fn visit_values(operands: &[BinaryOperand], visit: &mut impl FnMut(u64)) {
    for operand in operands {
        match operand {
            BinaryOperand::Value(value) => visit(*value),
            BinaryOperand::Values(values) => values.iter().copied().for_each(&mut *visit),
            BinaryOperand::BlockTarget { arguments, .. } => {
                arguments.iter().copied().for_each(&mut *visit);
            }
            BinaryOperand::SwitchCases(cases) => {
                for (_, _, arguments) in cases {
                    arguments.iter().copied().for_each(&mut *visit);
                }
            }
            BinaryOperand::Type(_)
            | BinaryOperand::Types(_)
            | BinaryOperand::Index(_)
            | BinaryOperand::Indices(_)
            | BinaryOperand::Symbol { .. }
            | BinaryOperand::Integer(_)
            | BinaryOperand::Integers(_)
            | BinaryOperand::OptionalFloat32(_)
            | BinaryOperand::Bytes(_)
            | BinaryOperand::Clauses(_)
            | BinaryOperand::SwitchEnumCases(_)
            | BinaryOperand::OptionalIndex(_) => {}
            BinaryOperand::OptionalValue(Some(value)) => visit(*value),
            BinaryOperand::OptionalValue(None) => {}
        }
    }
}

pub(super) fn visit_edges(instruction: &BinaryInstruction, visit: impl FnMut(u64, &[u64])) {
    let mut visit = visit;
    match instruction.opcode {
        0x1e0 | 0x1e1 => {
            for operand in &instruction.operands {
                if let BinaryOperand::BlockTarget { block, arguments } = operand {
                    visit(*block, arguments);
                }
            }
        }
        0x1e2 => {
            for operand in &instruction.operands {
                match operand {
                    BinaryOperand::SwitchCases(cases) => {
                        for (_, block, arguments) in cases {
                            visit(*block, arguments);
                        }
                    }
                    BinaryOperand::BlockTarget { block, arguments } => visit(*block, arguments),
                    _ => {}
                }
            }
        }
        0x1e3 => {
            for operand in &instruction.operands {
                if let BinaryOperand::SwitchEnumCases(cases) = operand {
                    for (_, block) in cases {
                        visit(*block, &[]);
                    }
                }
                if let BinaryOperand::OptionalIndex(Some(block)) = operand {
                    visit(*block, &[]);
                }
            }
        }
        0x1e4 => {
            for operand in &instruction.operands {
                if let BinaryOperand::Indices(blocks) = operand {
                    for block in blocks {
                        visit(*block, &[]);
                    }
                }
            }
        }
        0x1a7 | 0x1aa | 0x1c1 | 0x1c2 => {
            for operand in &instruction.operands {
                if let BinaryOperand::Index(block) = operand {
                    visit(*block, &[]);
                }
            }
        }
        0x221 => {
            let normal = instruction
                .operands
                .iter()
                .filter_map(|operand| match operand {
                    BinaryOperand::Index(index) => Some(*index),
                    _ => None,
                })
                .nth(1);
            if let Some(normal) = normal {
                visit(normal, &[]);
            }
            for operand in &instruction.operands {
                if let BinaryOperand::Indices(labels) = operand {
                    for label in labels {
                        visit(*label, &[]);
                    }
                }
            }
        }
        _ => {}
    }
}

fn verify_reverse_postorder(
    body: &tir_format::BinaryFunctionBody,
    successors: &[Vec<usize>],
    dominators: &DominatorTree,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let ordered_successors = successors
        .iter()
        .map(|targets| {
            let mut targets = targets.clone();
            targets.sort_unstable();
            targets.dedup();
            targets
        })
        .collect::<Vec<_>>();
    let mut visited = vec![false; body.blocks.len()];
    let mut order = Vec::with_capacity(body.blocks.len());
    for root in dominators.roots() {
        let mut postorder = Vec::new();
        visit_postorder(
            *root,
            &ordered_successors,
            dominators.components(),
            &mut visited,
            &mut postorder,
        );
        order.extend(postorder.into_iter().rev());
    }
    if let Some((expected, actual)) = order
        .iter()
        .copied()
        .enumerate()
        .find(|(expected, actual)| expected != actual)
    {
        diagnostics.push(Diagnostic::error(
            V1,
            format!(
                "function {} blocks are not in reverse post-order: block {actual} appears at position {expected}, expected block {expected}",
                body.function_index
            ),
        ));
    }
}

fn visit_postorder(
    root: usize,
    successors: &[Vec<usize>],
    components: &[usize],
    visited: &mut [bool],
    postorder: &mut Vec<usize>,
) {
    visited[root] = true;
    let mut stack = vec![(root, 0usize)];
    while let Some((block, next_successor)) = stack.last().copied() {
        let next = successors[block]
            .get(next_successor)
            .copied()
            .filter(|target| components[*target] == components[block]);
        if let Some(target) = next {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
            if !visited[target] {
                visited[target] = true;
                stack.push((target, 0));
            }
        } else if next_successor < successors[block].len() {
            stack.last_mut().expect("DFS stack is nonempty").1 += 1;
        } else {
            postorder.push(block);
            stack.pop();
        }
    }
}
