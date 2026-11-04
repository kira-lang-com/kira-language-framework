use tir_format::{BinaryInstruction, BinaryOperand, BinaryTypeTable};

pub(super) fn uses_atomic128(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    let value_type = match instruction.opcode {
        0x200 => instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Type(index) => Some(*index),
                _ => None,
            }),
        0x201 => value_at_operand(instruction, 0, value_types),
        0x202 | 0x203 => value_at_operand(instruction, 1, value_types),
        _ => None,
    };
    value_type
        .and_then(|index| crate::binary_constants::binary_type_layout(types, index).ok())
        .is_some_and(|layout| layout.size == 16)
}

fn value_at_operand(
    instruction: &BinaryInstruction,
    operand_index: usize,
    value_types: &[Option<u64>],
) -> Option<u64> {
    let value = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => values.get(operand_index).copied(),
            BinaryOperand::Value(value) if operand_index == 0 => Some(*value),
            _ => None,
        })?;
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}
