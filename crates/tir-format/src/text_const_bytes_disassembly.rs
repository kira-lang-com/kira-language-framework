use std::fmt::Write as _;

use crate::FormatError;
use crate::types::{BinaryFunctionType, BinaryInstruction, BinaryOperand, BinaryTypeTable};

pub(crate) fn format_const_bytes(
    text: &mut String,
    result_name: &str,
    instruction: &BinaryInstruction,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    signatures: &crate::types::BinarySignatureTable,
) -> Result<(), FormatError> {
    let [BinaryOperand::Type(type_index), BinaryOperand::Bytes(bytes)] =
        instruction.operands.as_slice()
    else {
        return Err(malformed("const.bytes operands are malformed"));
    };
    if *type_index != function.result {
        return Err(malformed("const.bytes type does not match function result"));
    }
    let ty = crate::text_type_disassembly::format_type_reference(types, *type_index, signatures)?;
    write!(text, "    %{result_name} = const.bytes {ty}, b\"").map_err(formatting_error)?;
    for byte in bytes {
        write!(text, "\\x{byte:02X}").map_err(formatting_error)?;
    }
    text.push_str("\"\n");
    Ok(())
}

fn formatting_error(_: std::fmt::Error) -> FormatError {
    malformed("text formatting failed")
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
