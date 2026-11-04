use std::collections::HashSet;

use tir_format::{BinaryFunctionBody, BinaryInstruction, BinaryOperand};

pub(super) fn borrowed_values(body: &BinaryFunctionBody) -> HashSet<u64> {
    let mut borrowed = HashSet::new();
    let mut parameter_values = Vec::with_capacity(body.blocks.len());
    let mut instruction_starts = Vec::with_capacity(body.blocks.len());
    let mut cursor = 0_u64;
    for block in &body.blocks {
        let params =
            (cursor..cursor.saturating_add(block.parameters.len() as u64)).collect::<Vec<_>>();
        for (parameter, value) in block.parameters.iter().zip(&params) {
            if matches!(parameter.convention, 1 | 2) {
                borrowed.insert(*value);
            }
        }
        cursor = cursor.saturating_add(block.parameters.len() as u64);
        parameter_values.push(params);
        let mut starts = Vec::new();
        if let Some(instructions) = &block.instructions {
            for instruction in instructions {
                starts.push(cursor);
                if matches!(instruction.opcode, 0x141 | 0x183) && instruction.result_count > 0 {
                    borrowed.insert(cursor);
                }
                cursor = cursor.saturating_add(instruction.result_count);
            }
        } else {
            cursor = cursor.saturating_add(block.instruction_count);
        }
        instruction_starts.push(starts);
    }
    loop {
        let mut changed = false;
        for (block_index, block) in body.blocks.iter().enumerate() {
            let Some(instructions) = &block.instructions else {
                continue;
            };
            for (instruction_index, instruction) in instructions.iter().enumerate() {
                let start = instruction_starts[block_index][instruction_index];
                let values = value_ids(instruction);
                let propagates =
                    matches!(instruction.opcode, 0x03d | 0x045 | 0x122..=0x128 | 0x148)
                        || matches!(instruction.opcode, 0x101 | 0x105 | 0x10d);
                if propagates
                    && instruction.result_count > 0
                    && values.iter().any(|value| borrowed.contains(value))
                {
                    changed |= borrowed.insert(start);
                }
            }
            if let Some(terminator) = instructions.last() {
                super::super::super::structure::visit_edges(terminator, |target, arguments| {
                    let Some(target_parameters) = usize::try_from(target)
                        .ok()
                        .and_then(|index| parameter_values.get(index))
                    else {
                        return;
                    };
                    for (argument, parameter) in arguments.iter().zip(target_parameters) {
                        if borrowed.contains(argument) {
                            changed |= borrowed.insert(*parameter);
                        }
                    }
                });
            }
        }
        if !changed {
            break;
        }
    }
    borrowed
}

fn value_ids(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .copied()
        .collect()
}
