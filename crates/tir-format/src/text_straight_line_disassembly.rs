use std::fmt::Write as _;

use crate::types::{
    BinaryFunctionBody, BinaryFunctionType, BinaryInstruction, BinaryOperand, BinarySignatureTable,
    BinaryType, BinaryTypeTable,
};
use crate::{FormatError, StringTable};

pub(crate) fn format(
    text: &mut String,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    _strings: &StringTable,
) -> Result<(), FormatError> {
    let [block] = body.blocks.as_slice() else {
        return Err(malformed("straight-line disassembly requires one block"));
    };
    let instructions = block
        .instructions
        .as_ref()
        .ok_or_else(|| malformed("unsupported CODE instruction"))?;
    if block.instruction_count != instructions.len() as u64
        || block.parameters.len() != function.parameters.len()
        || block.cold
        || block.loop_merge.is_some()
        || block.loop_continue.is_some()
    {
        return Err(malformed("unsupported straight-line CODE block"));
    }
    text.push_str(" {\n  bb0(");
    let mut values = Vec::new();
    for (index, parameter) in block.parameters.iter().enumerate() {
        let signature = &function.parameters[index];
        if parameter.ty != signature.ty
            || parameter.convention != signature.convention
            || parameter.context != signature.context
        {
            return Err(malformed(
                "entry block parameters do not match function signature",
            ));
        }
        if index != 0 {
            text.push_str(", ");
        }
        write!(text, "%arg{index}: ").map_err(formatting_error)?;
        if parameter.context {
            text.push_str("context ");
        }
        match parameter.convention {
            0 => text.push_str("owned "),
            1 => text.push_str("borrow "),
            2 => text.push_str("borrowmut "),
            3 => {}
            _ => return Err(malformed("unsupported block parameter convention")),
        }
        text.push_str(&crate::text_type_disassembly::format_type_reference(
            types,
            parameter.ty,
            signatures,
        )?);
        values.push((format!("arg{index}"), parameter.ty));
    }
    text.push_str("):\n");
    let Some((terminator, operations)) = instructions.split_last() else {
        return Err(malformed("CODE block contains no terminator"));
    };
    format_operations(text, operations, types, &mut values)?;
    if terminator.opcode != 0x1e5 || terminator.modifiers != 0 || terminator.result_count != 0 {
        return Err(malformed("straight-line CODE must end with return"));
    }
    text.push_str("    return");
    match (
        get_type(types, function.result)?,
        terminator.operands.as_slice(),
    ) {
        (BinaryType::Unit, []) => {}
        (BinaryType::Unit, _) => return Err(malformed("unit return must not have an operand")),
        (_, [BinaryOperand::Value(value)]) => {
            let (name, ty) = get_value(&values, *value)?;
            if ty != function.result {
                return Err(malformed(
                    "return value type does not match function result",
                ));
            }
            write!(text, " %{name}").map_err(formatting_error)?;
        }
        _ => return Err(malformed("return expects one SSA value")),
    }
    text.push_str("\n  }");
    Ok(())
}

pub(crate) fn format_operations(
    text: &mut String,
    operations: &[BinaryInstruction],
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let i1_type = types
        .entries
        .iter()
        .position(|ty| matches!(ty, BinaryType::Integer(1)))
        .map(|value| value as u64);
    for operation in operations {
        if crate::text_memory_disassembly::handles(operation.opcode) {
            crate::text_memory_disassembly::format(text, operation, types, values)?;
            continue;
        }
        if matches!(operation.opcode, 0x000 | 0x002) {
            if operation.result_count != 1 || operation.modifiers != 0 {
                return Err(malformed("constant has invalid results or modifiers"));
            }
            let result_name = format!("v{}", values.len());
            let result_ty = if operation.opcode == 0x000 {
                let [BinaryOperand::Type(index), BinaryOperand::Bytes(bytes)] =
                    operation.operands.as_slice()
                else {
                    return Err(malformed("const.int operands are malformed"));
                };
                let BinaryType::Integer(width) = get_type(types, *index)? else {
                    return Err(malformed("const.int must have an integer type"));
                };
                let width = u16::try_from(*width)
                    .map_err(|_| malformed("const.int width is unsupported"))?;
                if width == 0
                    || width > 128
                    || bytes.len() != usize::from(width).div_ceil(8)
                    || (width % 8 != 0 && bytes.last().is_some_and(|byte| byte >> (width % 8) != 0))
                {
                    return Err(malformed("invalid const.int payload width or high bits"));
                }
                let mut raw = [0u8; 16];
                raw[..bytes.len()].copy_from_slice(bytes);
                let value = u128::from_le_bytes(raw);
                write!(text, "    %{result_name} = const.int i{width}, {value}\n")
                    .map_err(formatting_error)?;
                *index
            } else {
                let [BinaryOperand::Integer(value)] = operation.operands.as_slice() else {
                    return Err(malformed("const.bool operands are malformed"));
                };
                let literal = match (value.negative, value.magnitude) {
                    (false, 0) => "false",
                    (false, 1) => "true",
                    _ => return Err(malformed("const.bool is not false or true")),
                };
                write!(text, "    %{result_name} = const.bool {literal}\n")
                    .map_err(formatting_error)?;
                i1_type.ok_or_else(|| malformed("const.bool result i1 is not interned"))?
            };
            values.push((result_name, result_ty));
            continue;
        }
        if matches!(operation.opcode, 0x180..=0x184) {
            let [BinaryOperand::Value(value)] = operation.operands.as_slice() else {
                return Err(malformed("ownership operation expects one SSA value"));
            };
            if operation.modifiers != 0
                || operation.result_count != u64::from(!matches!(operation.opcode, 0x182 | 0x184))
            {
                return Err(malformed(
                    "invalid ownership instruction modifiers or result count",
                ));
            }
            let (operand, ty) = get_value(values, *value)?;
            if matches!(operation.opcode, 0x182 | 0x184) {
                let opcode = if operation.opcode == 0x182 {
                    "destroy"
                } else {
                    "borrow.end"
                };
                writeln!(text, "    {opcode} %{operand}").map_err(formatting_error)?;
            } else {
                let opcode = match operation.opcode {
                    0x180 => "copy",
                    0x181 => "move",
                    0x183 => "borrow.begin",
                    _ => return Err(malformed("invalid ownership instruction")),
                };
                let result_name = format!("v{}", values.len());
                writeln!(text, "    %{result_name} = {opcode} %{operand}")
                    .map_err(formatting_error)?;
                values.push((result_name, ty));
            }
            continue;
        }
        let (opcode, operands, result_ty) = if let Some((opcode, operand_count)) =
            super::text_function_body_disassembly::integer_operation_name(operation.opcode)
        {
            if !super::text_function_body_disassembly::valid_integer_operation_modifiers(operation)
            {
                return Err(malformed("unsupported integer operation modifier"));
            }
            let operands = value_operands(operation, operand_count)?;
            let first = get_value(&values, operands[0])?.1;
            if operands
                .iter()
                .any(|operand| get_value(&values, *operand).is_err())
                || operands
                    .iter()
                    .any(|operand| get_value(&values, *operand).is_ok_and(|(_, ty)| ty != first))
            {
                return Err(malformed("integer operation operands have different types"));
            }
            let operand_ty = get_type(types, first)?;
            let result_ty = if operation.opcode == 0x03c {
                if !matches!(operand_ty, BinaryType::Integer(_) | BinaryType::Pointer(_))
                    || (matches!(operand_ty, BinaryType::Pointer(_))
                        && matches!(operation.modifiers >> 48, 2..=5))
                {
                    return Err(malformed("invalid icmp operand type"));
                }
                i1_type.ok_or_else(|| malformed("icmp result type i1 is missing"))?
            } else {
                if !matches!(operand_ty, BinaryType::Integer(_)) {
                    return Err(malformed("integer operation requires integer operands"));
                }
                first
            };
            let opcode = if operation.opcode == 0x03c {
                let predicate = match operation.modifiers >> 48 {
                    0 => "eq",
                    1 => "ne",
                    2 => "slt",
                    3 => "sle",
                    4 => "sgt",
                    5 => "sge",
                    6 => "ult",
                    7 => "ule",
                    8 => "ugt",
                    9 => "uge",
                    _ => return Err(malformed("invalid icmp predicate")),
                };
                format!("icmp {predicate}")
            } else {
                opcode.to_owned()
            };
            (opcode, operands, result_ty)
        } else if operation.opcode == 0x03d {
            if operation.result_count != 1 || operation.modifiers != 0 {
                return Err(malformed("invalid select result or modifiers"));
            }
            let operands = value_operands(operation, 3)?;
            if Some(get_value(&values, operands[0])?.1) != i1_type {
                return Err(malformed("select condition must be i1"));
            }
            let ty = get_value(&values, operands[1])?.1;
            if get_value(&values, operands[2])?.1 != ty {
                return Err(malformed("select value types must match"));
            }
            ("select".to_owned(), operands, ty)
        } else {
            return Err(malformed("unsupported straight-line CODE instruction"));
        };
        if operation.result_count != 1 {
            return Err(malformed("straight-line operation must define one value"));
        }
        let result_name = format!("v{}", values.len());
        write!(text, "    %{result_name} = {opcode}").map_err(formatting_error)?;
        for (index, operand) in operands.iter().enumerate() {
            let (name, _) = get_value(&values, *operand)?;
            write!(text, "{}%{name}", if index == 0 { " " } else { ", " })
                .map_err(formatting_error)?;
        }
        text.push('\n');
        values.push((result_name, result_ty));
    }
    Ok(())
}

fn value_operands(instruction: &BinaryInstruction, count: usize) -> Result<Vec<u64>, FormatError> {
    match instruction.operands.as_slice() {
        [BinaryOperand::Value(value)] if count == 1 => Ok(vec![*value]),
        [BinaryOperand::Values(values)] if values.len() == count => Ok(values.clone()),
        _ => Err(malformed("unexpected operation operand shape")),
    }
}

fn get_type(types: &BinaryTypeTable, index: u64) -> Result<&BinaryType, FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
        .ok_or_else(|| malformed("type index is out of range"))
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("SSA value index is out of range"))
}

fn malformed(message: &'static str) -> FormatError {
    FormatError::malformed(None, message)
}

fn formatting_error(error: std::fmt::Error) -> FormatError {
    FormatError::malformed(None, error.to_string())
}
