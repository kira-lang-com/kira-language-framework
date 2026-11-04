use tir_format::{
    BinaryLinkage, BinaryOperand, BinarySignatureTable, BinaryTarget, BinaryType, BinaryTypeTable,
};

pub(super) fn resolve(
    instruction: &tir_format::BinaryInstruction,
    value_types: &[Option<u64>],
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
) -> Vec<usize> {
    if instruction.opcode != 0x404 {
        return Vec::new();
    }
    let call_type = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        });
    let table_value = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => values.first().copied(),
            _ => None,
        });
    let table_type = table_value
        .and_then(|value| usize::try_from(value).ok())
        .and_then(|value| value_types.get(value))
        .copied()
        .flatten()
        .and_then(|type_index| usize::try_from(type_index).ok())
        .and_then(|type_index| types.entries.get(type_index))
        .and_then(|ty| match ty {
            BinaryType::VisibleFunctionTable { function_type } => Some(*function_type),
            _ => None,
        });
    let Some(table_type) = table_type.filter(|table_type| call_type == Some(*table_type)) else {
        return Vec::new();
    };
    signatures
        .records
        .iter()
        .enumerate()
        .filter_map(|(index, signature)| {
            (signature.function_type == table_type
                && signature.linkage == BinaryLinkage::Exported
                && signature.target == BinaryTarget::Gpu
                && signature.entry_kind.is_none())
            .then_some(index)
        })
        .collect()
}
