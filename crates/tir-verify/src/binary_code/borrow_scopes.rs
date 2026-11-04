use std::collections::{BTreeSet, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand};

const V6: Code = Code::new(Dialect::Tir, Area::Verifier, 6);

pub(super) fn verify(body: &BinaryFunctionBody, function_name: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if body.blocks.is_empty() {
        return diagnostics;
    }

    let mut starts = Vec::with_capacity(body.blocks.len());
    let mut parameter_starts = Vec::with_capacity(body.blocks.len());
    let mut cursor = 0_u64;
    let mut successors = Vec::with_capacity(body.blocks.len());
    for block in &body.blocks {
        parameter_starts.push(
            (cursor..cursor.saturating_add(block.parameters.len() as u64)).collect::<Vec<_>>(),
        );
        cursor = cursor.saturating_add(block.parameters.len() as u64);
        let mut block_starts = Vec::new();
        let mut targets = Vec::new();
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                block_starts.push(cursor);
                cursor = cursor.saturating_add(instruction.result_count);
            }
            if let Some(terminator) = instructions.last() {
                super::structure::visit_edges(terminator, |target, arguments| {
                    if let Ok(target) = usize::try_from(target)
                        && target < body.blocks.len()
                    {
                        targets.push((target, arguments.to_vec()));
                    }
                });
            }
        }
        starts.push(block_starts);
        successors.push(targets);
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
            let mut scopes = incoming[block_index].clone().unwrap_or_default();
            let block = &body.blocks[block_index];
            if let Some(instructions) = &block.instructions {
                for (instruction_index, instruction) in instructions.iter().enumerate() {
                    match instruction.opcode {
                        0x141 | 0x183 => {
                            scopes.push(BTreeSet::from([starts[block_index][instruction_index]]));
                        }
                        0x184 => {
                            let borrowed = first_value(instruction);
                            match (borrowed, scopes.last()) {
                                (Some(borrowed), Some(open)) if open.contains(&borrowed) => {
                                    scopes.pop();
                                }
                                (Some(borrowed), Some(open)) => {
                                    let innermost = open
                                        .iter()
                                        .map(u64::to_string)
                                        .collect::<Vec<_>>()
                                        .join(" or ");
                                    diagnostics.push(Diagnostic::error(
                                        V6,
                                        format!(
                                            "function @{function_name} borrow.end value {borrowed} closes scopes out of order; innermost open scope includes {innermost}"
                                        ),
                                    ));
                                    if let Some(position) =
                                        scopes.iter().rposition(|scope| scope.contains(&borrowed))
                                    {
                                        scopes.truncate(position);
                                    }
                                }
                                (Some(borrowed), None) => diagnostics.push(Diagnostic::error(
                                    V6,
                                    format!(
                                        "function @{function_name} borrow.end value {borrowed} has no matching open scope"
                                    ),
                                )),
                                (None, _) => diagnostics.push(Diagnostic::error(
                                    V6,
                                    format!(
                                        "function @{function_name} borrow.end has no value operand"
                                    ),
                                )),
                            }
                        }
                        _ => {}
                    }
                }
            }

            if successors[block_index].is_empty() && !scopes.is_empty() {
                diagnostics.push(Diagnostic::error(
                    V6,
                    format!(
                        "function @{function_name} exits with borrow scopes still open: {}",
                        scopes
                            .iter()
                            .map(|aliases| {
                                aliases
                                    .iter()
                                    .next()
                                    .map_or_else(|| "<unknown>".to_owned(), u64::to_string)
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }

            for (target, arguments) in &successors[block_index] {
                let mut edge_scopes = scopes.clone();
                for (argument, parameter) in arguments.iter().zip(&parameter_starts[*target]) {
                    for scope in &mut edge_scopes {
                        if scope.contains(argument) {
                            scope.insert(*parameter);
                        }
                    }
                }
                match incoming[*target].as_mut() {
                    None => {
                        incoming[*target] = Some(edge_scopes);
                        queue.push_back(*target);
                    }
                    Some(existing) => {
                        let compatible = existing.len() == edge_scopes.len()
                            && existing.iter().zip(&edge_scopes).all(|(left, right)| {
                                left.iter().any(|value| right.contains(value))
                            });
                        if !compatible && reported_joins.insert(*target) {
                            diagnostics.push(Diagnostic::error(
                                V6,
                                format!(
                                    "function @{function_name} borrow scopes reaching block {target} differ across control-flow paths"
                                ),
                            ));
                        } else if compatible {
                            for (left, right) in existing.iter_mut().zip(edge_scopes) {
                                left.extend(right);
                            }
                        }
                    }
                }
            }
        }
    }

    diagnostics
}

fn first_value(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Value(value) => Some(*value),
            _ => None,
        })
}
