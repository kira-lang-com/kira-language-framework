use std::collections::HashSet;

use tir_format::{BinaryFunctionBody, BinaryOperand};

use crate::Uniformity;

pub(super) struct Edge {
    source: usize,
    target: usize,
    arguments: Vec<u64>,
}

pub(super) fn control_flow_edges(body: &BinaryFunctionBody) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (source, block) in body.blocks.iter().enumerate() {
        let Some(terminator) = block.instructions.as_ref().and_then(|items| items.last()) else {
            continue;
        };
        match terminator.opcode {
            0x1e0 | 0x1e1 => {
                for operand in &terminator.operands {
                    if let BinaryOperand::BlockTarget { block, arguments } = operand {
                        push_edge(&mut edges, source, *block, arguments);
                    }
                }
            }
            0x1e2 => {
                for operand in &terminator.operands {
                    match operand {
                        BinaryOperand::SwitchCases(cases) => {
                            for (_, block, arguments) in cases {
                                push_edge(&mut edges, source, *block, arguments);
                            }
                        }
                        BinaryOperand::BlockTarget { block, arguments } => {
                            push_edge(&mut edges, source, *block, arguments);
                        }
                        _ => {}
                    }
                }
            }
            0x1e3 => {
                for operand in &terminator.operands {
                    match operand {
                        BinaryOperand::SwitchEnumCases(cases) => {
                            for (_, block) in cases {
                                push_edge(&mut edges, source, *block, &[]);
                            }
                        }
                        BinaryOperand::OptionalIndex(Some(block)) => {
                            push_edge(&mut edges, source, *block, &[]);
                        }
                        _ => {}
                    }
                }
            }
            0x1e4 => {
                for operand in &terminator.operands {
                    if let BinaryOperand::Indices(targets) = operand {
                        for target in targets {
                            push_edge(&mut edges, source, *target, &[]);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    edges
}

fn push_edge(edges: &mut Vec<Edge>, source: usize, target: u64, arguments: &[u64]) {
    if let Ok(target) = usize::try_from(target) {
        edges.push(Edge {
            source,
            target,
            arguments: arguments.to_vec(),
        });
    }
}

pub(super) fn successors(block_count: usize, edges: &[Edge]) -> Vec<Vec<usize>> {
    let mut successors = vec![Vec::new(); block_count];
    for edge in edges {
        if edge.source < block_count
            && edge.target < block_count
            && !successors[edge.source].contains(&edge.target)
        {
            successors[edge.source].push(edge.target);
        }
    }
    successors
}

pub(super) fn update_control(
    body: &BinaryFunctionBody,
    successors: &[Vec<usize>],
    values: &[Uniformity],
    control: &mut [Uniformity],
) -> bool {
    let mut changed = false;
    for (index, block) in body.blocks.iter().enumerate() {
        let Some(branch) = block.instructions.as_ref().and_then(|items| items.last()) else {
            continue;
        };
        if !matches!(branch.opcode, 0x1e1 | 0x1e2 | 0x1e3) {
            continue;
        }
        let merge = if branch.opcode == 0x1e3 {
            None
        } else {
            last_index(&branch.operands)
                .filter(|merge| *merge != 0)
                .and_then(|merge| usize::try_from(merge - 1).ok())
        };
        if merge.is_none() && branch.opcode != 0x1e3 {
            continue;
        }
        let condition = value_operands(&branch.operands)
            .first()
            .and_then(|value| value_uniformity(*value, values))
            .unwrap_or(Uniformity::Divergent);
        let nested_control = control[index].join(condition);
        let mut pending = successors[index].clone();
        let mut visited = HashSet::new();
        while let Some(target) = pending.pop() {
            if Some(target) == merge || !visited.insert(target) {
                continue;
            }
            if let Some(target_control) = control.get_mut(target) {
                let raised = target_control.join(nested_control);
                if raised != *target_control {
                    *target_control = raised;
                    changed = true;
                }
            }
            pending.extend(successors.get(target).into_iter().flatten().copied());
        }
    }
    changed
}

pub(super) fn update_block_parameters(
    body: &BinaryFunctionBody,
    edges: &[Edge],
    block_starts: &[usize],
    values: &[Uniformity],
    control: &[Uniformity],
    updated: &mut [Uniformity],
) -> bool {
    let mut changed = false;
    for (block_index, block) in body.blocks.iter().enumerate().skip(1) {
        for parameter_index in 0..block.parameters.len() {
            let Some(value_index) = block_starts
                .get(block_index)
                .and_then(|start| start.checked_add(parameter_index))
            else {
                continue;
            };
            let incoming = edges
                .iter()
                .filter(|edge| edge.target == block_index)
                .map(|edge| {
                    let argument = edge.arguments.get(parameter_index).copied();
                    let uniformity = argument
                        .and_then(|argument| value_uniformity(argument, values))
                        .unwrap_or(Uniformity::Divergent);
                    (argument, uniformity, control[edge.source])
                })
                .collect::<Vec<_>>();
            let candidate = if incoming.is_empty() {
                Uniformity::Divergent
            } else if incoming
                .iter()
                .all(|(argument, _, _)| *argument == incoming[0].0 && argument.is_some())
            {
                incoming
                    .iter()
                    .fold(Uniformity::Dispatch, |current, (_, value, _)| {
                        current.join(*value)
                    })
            } else {
                incoming
                    .iter()
                    .fold(Uniformity::Dispatch, |current, (_, value, path)| {
                        current.join(*value).join(*path)
                    })
            };
            if let Some(current) = updated.get_mut(value_index) {
                let raised = current.join(candidate);
                if raised != *current {
                    *current = raised;
                    changed = true;
                }
            }
        }
    }
    changed
}

pub(super) fn value_operands(operands: &[BinaryOperand]) -> Vec<u64> {
    let mut values = Vec::new();
    for operand in operands {
        match operand {
            BinaryOperand::Value(value) => values.push(*value),
            BinaryOperand::Values(items) => values.extend(items.iter().copied()),
            _ => {}
        }
    }
    values
}

pub(super) fn value_uniformity(value: u64, values: &[Uniformity]) -> Option<Uniformity> {
    usize::try_from(value)
        .ok()
        .and_then(|index| values.get(index))
        .copied()
}

pub(super) fn last_index(operands: &[BinaryOperand]) -> Option<u64> {
    operands.iter().rev().find_map(|operand| match operand {
        BinaryOperand::Index(index) => Some(*index),
        _ => None,
    })
}
