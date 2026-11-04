use std::collections::{BTreeSet, HashSet, VecDeque};

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand};

const V7: Code = Code::new(Dialect::Tir, Area::Verifier, 7);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Read,
    Modify,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenAccess {
    tokens: BTreeSet<u64>,
    addresses: BTreeSet<u64>,
    mode: Mode,
}

pub(super) fn verify(body: &BinaryFunctionBody, function_name: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if body.blocks.is_empty() || body.blocks.iter().any(|block| block.instructions.is_none()) {
        return diagnostics;
    }

    let mut parameter_starts = Vec::with_capacity(body.blocks.len());
    let mut instruction_starts = Vec::with_capacity(body.blocks.len());
    let mut successors = Vec::with_capacity(body.blocks.len());
    let mut cursor = 0_u64;
    for block in &body.blocks {
        parameter_starts.push(
            (cursor..cursor.saturating_add(block.parameters.len() as u64)).collect::<Vec<_>>(),
        );
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
        instruction_starts.push(starts);
        successors.push(edges);
    }

    let mut incoming = vec![None::<Vec<OpenAccess>>; body.blocks.len()];
    let mut reported_conflicts = HashSet::new();
    let mut queue = VecDeque::new();
    for root in 0..body.blocks.len() {
        if incoming[root].is_some() {
            continue;
        }
        incoming[root] = Some(Vec::new());
        queue.push_back(root);

        while let Some(block_index) = queue.pop_front() {
            let mut open = incoming[block_index].clone().unwrap_or_default();
            if let Some(instructions) = &body.blocks[block_index].instructions {
                for (instruction_index, instruction) in instructions.iter().enumerate() {
                    match instruction.opcode {
                        0x187 if ((instruction.modifiers >> 48) & 0xf) <= 1 => begin_access(
                            instruction,
                            instruction_starts[block_index][instruction_index],
                            &mut reported_conflicts,
                            function_name,
                            &mut open,
                            &mut diagnostics,
                        ),
                        0x188 => end_access(instruction, &mut open),
                        _ => {}
                    }
                }
            }

            for (target, arguments) in &successors[block_index] {
                let mut outgoing = open.clone();
                for (argument, parameter) in arguments.iter().zip(&parameter_starts[*target]) {
                    for access in &mut outgoing {
                        if access.tokens.contains(argument) {
                            access.tokens.insert(*parameter);
                        }
                        if access.addresses.contains(argument) {
                            access.addresses.insert(*parameter);
                        }
                    }
                }
                match incoming[*target].as_mut() {
                    None => {
                        incoming[*target] = Some(outgoing);
                        queue.push_back(*target);
                    }
                    Some(existing) => {
                        if merge_accesses(existing, outgoing) {
                            queue.push_back(*target);
                        }
                    }
                }
            }
        }
    }

    diagnostics
}

fn begin_access(
    instruction: &BinaryInstruction,
    token: u64,
    reported_conflicts: &mut HashSet<u64>,
    function_name: &str,
    open: &mut Vec<OpenAccess>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(address) = first_value(instruction) else {
        return;
    };
    let mode = if ((instruction.modifiers >> 48) & 0xf) & 1 == 0 {
        Mode::Read
    } else {
        Mode::Modify
    };
    let addresses = BTreeSet::from([address]);
    let conflict = open.iter().any(|existing| {
        !existing.addresses.is_disjoint(&addresses)
            && (mode == Mode::Modify || existing.mode == Mode::Modify)
    });
    if conflict && reported_conflicts.insert(instruction.offset) {
        diagnostics.push(Diagnostic::error(
            V7,
            format!(
                "function @{function_name} CODE offset {}: conflicting static accesses to value {address}",
                instruction.offset
            ),
        ));
    }
    open.push(OpenAccess {
        tokens: BTreeSet::from([token]),
        addresses,
        mode,
    });
}

fn end_access(instruction: &BinaryInstruction, open: &mut Vec<OpenAccess>) {
    let Some(token) = first_value(instruction) else {
        return;
    };
    open.retain(|access| !access.tokens.contains(&token));
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

fn merge_accesses(existing: &mut Vec<OpenAccess>, incoming: Vec<OpenAccess>) -> bool {
    let mut changed = false;
    for mut access in incoming {
        if let Some(current) = existing
            .iter_mut()
            .find(|current| !current.tokens.is_disjoint(&access.tokens))
        {
            let before = current.clone();
            current.tokens.append(&mut access.tokens);
            current.addresses.append(&mut access.addresses);
            if access.mode == Mode::Modify {
                current.mode = Mode::Modify;
            }
            changed |= *current != before;
        } else {
            existing.push(access);
            changed = true;
        }
    }
    changed
}
