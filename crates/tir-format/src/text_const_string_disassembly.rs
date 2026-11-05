use std::fmt::Write as _;

use crate::types::{
    BinaryFunctionType, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, StringTable};

pub(crate) fn format_const_string(
    text: &mut String,
    result_name: &str,
    instruction: &BinaryInstruction,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    strings: &StringTable,
) -> Result<(), FormatError> {
    let [BinaryOperand::Index(string_index)] = instruction.operands.as_slice() else {
        return Err(malformed("const.string operands are malformed"));
    };
    let result_type = usize::try_from(function.result)
        .ok()
        .and_then(|index| types.entries.get(index));
    let is_string_type = matches!(
        result_type,
        Some(BinaryType::Tuple(elements))
            if elements.len() == 2
                && matches!(
                    usize::try_from(elements[0]).ok().and_then(|index| types.entries.get(index)),
                    Some(BinaryType::Pointer(0))
                )
                && matches!(
                    usize::try_from(elements[1]).ok().and_then(|index| types.entries.get(index)),
                    Some(BinaryType::Integer(64))
                )
    );
    if !is_string_type {
        return Err(malformed("const.string result type is not `(ptr, i64)`"));
    }
    let value = strings
        .get(index(*string_index)?)
        .ok_or_else(|| malformed("const.string index is out of range"))?;
    let literal = crate::text_global_disassembly::quote(value)?;
    writeln!(text, "    %{result_name} = const.string {literal}").map_err(formatting_error)
}

fn index(value: u64) -> Result<usize, FormatError> {
    usize::try_from(value).map_err(|_| malformed("const.string index overflows memory"))
}

fn formatting_error(_: std::fmt::Error) -> FormatError {
    malformed("text formatting failed")
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
