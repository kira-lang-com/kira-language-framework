use std::collections::{BTreeSet, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);

pub(super) fn verify(body: &BinaryFunctionBody, function_name: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if body.blocks.is_empty() {
        return diagnostics;
    }
    let mut parameter_starts = Vec::with_capacity(body.blocks.len());
    let mut instruction_starts = Vec::with_capacity(body.blocks.len());
    let mut successors = Vec::with_capacity(body.blocks.len());
    let mut cursor = 0_u64;
    for block in &body.blocks {
        let parameters =
            (cursor..cursor.saturating_add(block.parameters.len() as u64)).collect::<Vec<_>>();
        cursor = cursor.saturating_add(block.parameters.len() as u64);
        let mut starts = Vec::new();
        let mut edges = Vec::new();
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                starts.push(cursor);
                cursor = cursor.saturating_add(instruction.result_count);
            }
            if let Some(terminator) = instructions.last() {
                super::structure::visit_edges(terminator, |target, arguments| {
                    if let Ok(target) = usize::try_from(target)
                        && target < body.blocks.len()
                    {
                        edges.push((target, arguments.to_vec()));
                    }
                });
            }
        }
        parameter_starts.push(parameters);
        instruction_starts.push(starts);
        successors.push(edges);
    }

    let mut incoming = vec![None::<Vec<BTreeSet<u64>>>; body.blocks.len()];
    let mut reported_joins = HashSet::new();
    let mut queue = VecDeque::new();
    for root in 0..body.blocks.len() {
        if incoming[root].is_some() {
            continue;
        }
        incoming[root] = Some(Vec::new());
        queue.push_back(root);

        while let Some(block_index) = queue.pop_front() {
            let mut stack = incoming[block_index].clone().unwrap_or_default();
            if let Some(instructions) = &body.blocks[block_index].instructions {
                for (instruction_index, instruction) in instructions.iter().enumerate() {
                    match instruction.opcode {
                        0x120 => {
                            if instruction.result_count == 1 {
                                stack.push(BTreeSet::from([
                                    instruction_starts[block_index][instruction_index]
                                ]));
                            }
                        }
                        0x121 => {
                            deallocate(instruction, &mut stack, function_name, &mut diagnostics)
                        }
                        _ => propagate_aliases(
                            instruction,
                            instruction_starts[block_index][instruction_index],
                            &mut stack,
                        ),
                    }
                }
            }

            for (target, arguments) in &successors[block_index] {
                let mut outgoing = stack.clone();
                for (argument, parameter) in arguments.iter().zip(&parameter_starts[*target]) {
                    for aliases in &mut outgoing {
                        if aliases.contains(argument) {
                            aliases.insert(*parameter);
                        }
                    }
                }
                match incoming[*target].as_mut() {
                    None => {
                        incoming[*target] = Some(outgoing);
                        queue.push_back(*target);
                    }
                    Some(existing) => {
                        if existing.len() != outgoing.len() {
                            if reported_joins.insert(*target) {
                                diagnostics.push(Diagnostic::error(
                                    V2,
                                    format!(
                                        "function @{function_name} stack allocations reaching block {target} differ in depth across control-flow paths"
                                    ),
                                ));
                            }
                            continue;
                        }
                        let compatible = existing
                            .iter()
                            .zip(&outgoing)
                            .all(|(left, right)| !left.is_disjoint(right));
                        if !compatible {
                            if reported_joins.insert(*target) {
                                diagnostics.push(Diagnostic::error(
                                    V2,
                                    format!(
                                        "function @{function_name} stack allocation order reaching block {target} differs across control-flow paths"
                                    ),
                                ));
                            }
                            continue;
                        }
                        let mut changed = false;
                        for (left, right) in existing.iter_mut().zip(outgoing) {
                            let before = left.len();
                            left.extend(right);
                            changed |= before != left.len();
                        }
                        if changed {
                            queue.push_back(*target);
                        }
                    }
                }
            }
        }
    }
    diagnostics
}

fn deallocate(
    instruction: &BinaryInstruction,
    stack: &mut Vec<BTreeSet<u64>>,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let value = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Value(value) => Some(*value),
            _ => None,
        });
    if instruction.result_count != 0 || value.is_none() {
        diagnostics.push(Diagnostic::error(
            V2,
            format!(
                "function @{function_name} CODE offset {} has an invalid stack.dealloc shape",
                instruction.offset
            ),
        ));
        return;
    }
    let value = value.unwrap_or_default();
    if stack.last().is_some_and(|aliases| aliases.contains(&value)) {
        stack.pop();
        return;
    }
    if let Some(position) = stack.iter().rposition(|aliases| aliases.contains(&value)) {
        diagnostics.push(Diagnostic::error(
            V2,
            format!(
                "function @{function_name} CODE offset {} deallocates value {value} out of LIFO order",
                instruction.offset
            ),
        ));
        stack.truncate(position);
    } else {
        diagnostics.push(Diagnostic::error(
            V2,
            format!(
                "function @{function_name} CODE offset {} deallocates value {value} without an active stack allocation",
                instruction.offset
            ),
        ));
    }
}

fn propagate_aliases(
    instruction: &BinaryInstruction,
    result_start: u64,
    stack: &mut [BTreeSet<u64>],
) {
    if instruction.result_count != 1 {
        return;
    }
    let values = instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .copied()
        .collect::<Vec<_>>();
    let sources: Option<Vec<u64>> = match instruction.opcode {
        0x126 | 0x127 | 0x128 | 0x045 => values.first().copied().map(|value| vec![value]),
        0x03d if values.len() >= 3 => Some(values[values.len() - 2..].to_vec()),
        _ => None,
    };
    let Some(sources) = sources else {
        return;
    };
    for aliases in stack {
        if sources.iter().all(|source| aliases.contains(source)) {
            aliases.insert(result_start);
        }
    }
}
