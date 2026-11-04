use tir_format::{
    BinaryImport, BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType,
    BinaryTypeTable,
};

pub(super) fn direct_call_result(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    imports: Option<&[BinaryImport]>,
) -> Option<u64> {
    let index = function_symbol_index(instruction)?;
    let symbol = crate::binary_symbols::resolve_function_symbol(
        index,
        signatures,
        types,
        imports.unwrap_or_default(),
    )?;
    if symbol.generic_parameter_count != 0
        || instruction
            .operands
            .iter()
            .any(|operand| matches!(operand, BinaryOperand::Types(types) if !types.is_empty()))
    {
        return None;
    }
    let BinaryType::Function(function) = super::type_at(types, symbol.function_type_index)? else {
        return None;
    };
    if symbol
        .signature
        .is_some_and(|signature| signature.coroutine.is_some())
    {
        return None;
    }
    if function.asynchronous {
        symbol
            .local_index
            .and_then(|index| super::coroutines::frame_type(types, index))
    } else {
        super::function_result(types, symbol.function_type_index)
    }
}

pub(super) fn function_reference_type(
    instruction: &BinaryInstruction,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    imports: Option<&[BinaryImport]>,
) -> Option<u64> {
    let index = function_symbol_index(instruction)?;
    let symbol = crate::binary_symbols::resolve_function_symbol(
        index,
        signatures,
        types,
        imports.unwrap_or_default(),
    )?;
    if symbol.generic_parameter_count != 0
        || instruction
            .operands
            .iter()
            .any(|operand| matches!(operand, BinaryOperand::Types(types) if !types.is_empty()))
    {
        None
    } else {
        Some(symbol.function_type_index)
    }
}

fn function_symbol_index(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Symbol { kind: 0, index } => Some(*index),
            _ => None,
        })
}
