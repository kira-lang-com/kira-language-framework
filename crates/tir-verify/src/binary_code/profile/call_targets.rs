use tir_format::{BinaryImport, BinaryOperand};

pub(super) fn direct_target(
    instruction: &tir_format::BinaryInstruction,
    imports: Option<&[BinaryImport]>,
) -> Option<u64> {
    if !matches!(instruction.opcode, 0x1a0 | 0x1a7 | 0x1aa | 0x1e7) {
        return None;
    }
    let symbol_index = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })?;
    crate::binary_symbols::local_function_index(symbol_index, imports)
}
