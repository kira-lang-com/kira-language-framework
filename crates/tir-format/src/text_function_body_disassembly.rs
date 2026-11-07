use std::fmt::Write as _;

use crate::types::{
    BinaryConstantTable, BinaryFunctionBody, BinaryFunctionType, BinaryGlobalTable, BinaryOperand,
    BinarySignatureTable, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, StringTable};

pub(crate) fn format_return_body(
    text: &mut String,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    constants: &BinaryConstantTable,
    globals: &BinaryGlobalTable,
    strings: &StringTable,
) -> Result<(), crate::FormatError> {
    if body.block_count != 1
        || body.blocks.iter().any(|block| {
            block.instructions.as_ref().is_some_and(|instructions| {
                instructions
                    .first()
                    .is_some_and(|instruction| matches!(instruction.opcode, 0x1e0..=0x1e3))
                    || instructions.last().is_some_and(|instruction| {
                        matches!(instruction.opcode, 0x1e4..=0x1e9 | 0x215)
                    })
            })
        })
    {
        return crate::text_control_flow_disassembly::format_control_flow_body(
            text, body, function, types, signatures, constants, globals, strings,
        );
    }
    if body.block_count != 1 || body.blocks.len() != 1 {
        return Err(malformed("tirc dis supports only one-block return bodies"));
    }
    let block = &body.blocks[0];
    let unused_result = block.instruction_count == 2
        && matches!(
            types.entries.get(function.result as usize),
            Some(BinaryType::Unit)
        )
        && block.instructions.as_ref().is_some_and(|instructions| {
            instructions
                .first()
                .is_some_and(|instruction| instruction.result_count == 1)
        });
    if block.instruction_count > 2
        || unused_result
        || (block.instruction_count == 2
            && block.instructions.as_ref().is_some_and(|instructions| {
                instructions.first().is_some_and(|instruction| {
                    integer_operation_name(instruction.opcode).is_some()
                        || crate::text_conversion_assembly::name(instruction.opcode).is_some()
                        || crate::text_vector_simple::name(instruction.opcode).is_some()
                        || matches!(instruction.opcode, 0x0c3 | 0x0c6)
                        || matches!(instruction.opcode, 0x0c7 | 0x0c8)
                        || matches!(instruction.opcode, 0x0c9 | 0x0ca)
                        || matches!(
                            instruction.opcode,
                            0x122..=0x128 | 0x008 | 0x009 | 0x011 | 0x400..=0x403
                        )
                        || matches!(
                            instruction.opcode,
                            0x100 | 0x101 | 0x102 | 0x10a | 0x10b | 0x103 | 0x106 | 0x10c | 0x10d
                        )
                        || matches!(instruction.opcode, 0x104 | 0x105)
                        || matches!(instruction.opcode, 0x107..=0x109)
                        || crate::text_raw_memory::name(instruction.opcode).is_some()
                        || matches!(instruction.opcode, 0x146 | 0x147)
                        || matches!(instruction.opcode, 0x270..=0x272)
                        || matches!(instruction.opcode, 0x164..=0x166)
                        || matches!(instruction.opcode, 0x075..=0x079)
                        || matches!(instruction.opcode, 0x200 | 0x201 | 0x204)
                        || matches!(instruction.opcode, 0x205 | 0x206)
                        || matches!(instruction.opcode, 0x202 | 0x203)
                        || matches!(instruction.opcode, 0x210..=0x214)
                        || instruction.opcode == 0x1a0
                        || instruction.opcode == 0x1a1
                        || instruction.opcode == 0x00f
                        || instruction.opcode == 0x00a
                        || instruction.opcode == 0x00d
                        || matches!(instruction.opcode, 0x00b | 0x00c)
                        || matches!(instruction.opcode, 0x160..=0x163 | 0x167..=0x16a)
                        || matches!(
                            instruction.opcode,
                            0x223 | 0x224 | 0x230 | 0x243 | 0x244 | 0x245 | 0x246
                        )
                        || matches!(instruction.opcode, 0x180..=0x184)
                        || crate::text_memory_disassembly::handles(instruction.opcode)
                        || matches!(instruction.opcode, 0x187 | 0x188)
                })
            }))
    {
        return crate::text_straight_line_disassembly::format(
            text, body, function, types, signatures, constants, globals, strings,
        );
    }
    let instructions = block
        .instructions
        .as_ref()
        .ok_or_else(|| malformed("tirc dis cannot format an unsupported instruction"))?;
    let has_constant = block.instruction_count == 2
        && instructions.len() == 2
        && matches!(
            instructions[0].opcode,
            0x000 | 0x001 | 0x002 | 0x003 | 0x004 | 0x005 | 0x006 | 0x007
        )
        && instructions[0].modifiers == 0
        && instructions[0].result_count == 1;
    let has_integer_binary = block.instruction_count == 2
        && instructions.len() == 2
        && integer_operation_name(instructions[0].opcode).is_some()
        && valid_integer_operation_modifiers(&instructions[0])
        && instructions[0].result_count == 1;
    let has_select = block.instruction_count == 2
        && instructions.len() == 2
        && instructions[0].opcode == 0x03d
        && instructions[0].modifiers == 0
        && instructions[0].result_count == 1;
    let has_defined_value = has_constant || has_integer_binary || has_select;
    let return_instruction = if has_defined_value {
        instructions.get(1)
    } else if block.instruction_count == 1 && instructions.len() == 1 {
        instructions.first()
    } else {
        None
    }
    .ok_or_else(|| {
        malformed("tirc dis supports return or one value instruction followed by return")
    })?;
    if block.cold
        || block.loop_merge.is_some()
        || block.loop_continue.is_some()
        || return_instruction.opcode != 0x1e5
        || return_instruction.modifiers != 0
        || return_instruction.result_count != 0
        || block.parameters.len() != function.parameters.len()
    {
        return Err(malformed(
            "tirc dis supports only a single return instruction",
        ));
    }
    for (parameter, signature_parameter) in block.parameters.iter().zip(&function.parameters) {
        if parameter.convention != signature_parameter.convention
            || parameter.context != signature_parameter.context
            || parameter.ty != signature_parameter.ty
        {
            return Err(malformed(
                "entry block parameters do not match function signature",
            ));
        }
    }
    text.push_str(" {\n  bb0(");
    for (index, parameter) in block.parameters.iter().enumerate() {
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
            4 => {
                return Err(malformed(
                    "omitted block parameter convention is unsupported",
                ));
            }
            _ => return Err(malformed("unknown block parameter convention")),
        }
        text.push_str(&crate::text_type_disassembly::format_type_reference(
            types,
            parameter.ty,
            signatures,
        )?);
    }
    text.push_str("):\n");
    let result_name = if has_constant {
        let result_name = format!("v{}", block.parameters.len());
        match instructions[0].opcode {
            0x000 => {
                let (width, value) = format_const_int(&instructions[0], function.result, types)?;
                write!(text, "    %{result_name} = const.int i{width}, {value}\n")
                    .map_err(formatting_error)?;
            }
            0x001 => {
                let [
                    BinaryOperand::Type(type_index),
                    BinaryOperand::Index(constant_index),
                ] = instructions[0].operands.as_slice()
                else {
                    return Err(malformed("const.float operands are malformed"));
                };
                if *type_index != function.result {
                    return Err(malformed("const.float type does not match function result"));
                }
                let constant = constants
                    .records
                    .get(index(*constant_index)?)
                    .ok_or_else(|| malformed("const.float constant index is out of range"))?;
                if constant.ty != *type_index {
                    return Err(malformed("const.float constant has the wrong type"));
                }
                let Some(BinaryType::Float(float)) = usize::try_from(*type_index)
                    .ok()
                    .and_then(|index| types.entries.get(index))
                else {
                    return Err(malformed("const.float result type is not floating-point"));
                };
                let crate::types::BinaryConstantValue::Float(bytes) = &constant.value else {
                    return Err(malformed("const.float constant is not floating-point"));
                };
                let literal =
                    crate::text_global_disassembly::format_ieee_float_literal(bytes, *float)
                        .ok_or_else(|| {
                            malformed("const.float literal cannot be represented in TIR text")
                        })?;
                let ty = crate::text_type_disassembly::format_type_reference(
                    types,
                    *type_index,
                    signatures,
                )?;
                write!(text, "    %{result_name} = const.float {ty}, {literal}\n")
                    .map_err(formatting_error)?;
            }
            0x002 => {
                if !matches!(
                    usize::try_from(function.result)
                        .ok()
                        .and_then(|index| types.entries.get(index)),
                    Some(BinaryType::Integer(1))
                ) {
                    return Err(malformed("const.bool result type is not i1"));
                }
                let [BinaryOperand::Integer(value)] = instructions[0].operands.as_slice() else {
                    return Err(malformed("const.bool operand is malformed"));
                };
                let literal = match (value.negative, value.magnitude) {
                    (false, 0) => "false",
                    (false, 1) => "true",
                    _ => return Err(malformed("const.bool value is not zero or one")),
                };
                write!(text, "    %{result_name} = const.bool {literal}\n")
                    .map_err(formatting_error)?;
            }
            0x003 => {
                let Some(BinaryType::Pointer(result_space)) = usize::try_from(function.result)
                    .ok()
                    .and_then(|index| types.entries.get(index))
                else {
                    return Err(malformed("const.null result type is not a pointer"));
                };
                let [BinaryOperand::Index(space)] = instructions[0].operands.as_slice() else {
                    return Err(malformed("const.null operand is malformed"));
                };
                if *space != u64::from(*result_space) {
                    return Err(malformed(
                        "const.null address space does not match result type",
                    ));
                }
                let suffix = match *result_space {
                    0 => "",
                    1 => " device",
                    2 => " constant",
                    3 => " threadgroup",
                    4 => " thread",
                    5 => " object_payload",
                    6 => " ray_payload",
                    7 => " hit_attribute",
                    8 => " tile",
                    9 => " node_record",
                    _ => return Err(malformed("const.null has an unknown address space")),
                };
                write!(text, "    %{result_name} = const.null{suffix}\n")
                    .map_err(formatting_error)?;
            }
            0x004 | 0x005 => {
                let [BinaryOperand::Type(type_index)] = instructions[0].operands.as_slice() else {
                    return Err(malformed("const.zero operands are malformed"));
                };
                if *type_index != function.result {
                    return Err(malformed("const.zero type does not match function result"));
                }
                let ty = crate::text_type_disassembly::format_type_reference(
                    types,
                    *type_index,
                    signatures,
                )?;
                let opcode = if instructions[0].opcode == 0x004 {
                    "const.zero"
                } else {
                    "const.poison"
                };
                write!(text, "    %{result_name} = {opcode} {ty}\n").map_err(formatting_error)?;
            }
            0x007 => {
                crate::text_const_bytes_disassembly::format_const_bytes(
                    text,
                    &result_name,
                    &instructions[0],
                    function,
                    types,
                    signatures,
                )?;
            }
            0x006 => {
                crate::text_const_string_disassembly::format_const_string(
                    text,
                    &result_name,
                    &instructions[0],
                    function,
                    types,
                    strings,
                )?;
            }
            _ => return Err(malformed("unsupported entry constant")),
        }
        Some(result_name)
    } else if has_integer_binary {
        let result_name = format!("v{}", block.parameters.len());
        format_integer_binary(text, &instructions[0], &result_name, block, function, types)?;
        Some(result_name)
    } else if has_select {
        let result_name = format!("v{}", block.parameters.len());
        format_select(text, &instructions[0], &result_name, block, function, types)?;
        Some(result_name)
    } else {
        None
    };
    text.push_str("    return");
    let returns_unit = matches!(
        usize::try_from(function.result)
            .ok()
            .and_then(|index| types.entries.get(index)),
        Some(BinaryType::Unit)
    );
    match (returns_unit, return_instruction.operands.as_slice()) {
        (true, []) => {}
        (false, [BinaryOperand::Value(value)]) => {
            let value = usize::try_from(*value)
                .map_err(|_| malformed("return value index overflows memory"))?;
            if let Some(result_name) = &result_name {
                if value != block.parameters.len() {
                    return Err(malformed(
                        "return value is not the entry instruction result",
                    ));
                }
                write!(text, " %{result_name}").map_err(formatting_error)?;
            } else {
                let parameter = block
                    .parameters
                    .get(value)
                    .ok_or_else(|| malformed("return value is not a block parameter"))?;
                if parameter.ty != function.result {
                    return Err(malformed(
                        "return value type does not match function result",
                    ));
                }
                write!(text, " %arg{value}").map_err(formatting_error)?;
            }
        }
        _ => return Err(malformed("return operands do not match function result")),
    }
    text.push_str("\n  }");
    Ok(())
}

fn format_integer_binary(
    text: &mut String,
    instruction: &crate::types::BinaryInstruction,
    result_name: &str,
    block: &crate::types::BinaryBlock,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
) -> Result<(), crate::FormatError> {
    let (opcode_name, operand_count) = integer_operation_name(instruction.opcode)
        .ok_or_else(|| malformed("unsupported integer operation"))?;
    let is_icmp = instruction.opcode == 0x03c;
    let result_type = usize::try_from(function.result)
        .ok()
        .and_then(|index| types.entries.get(index));
    let valid_result = if is_icmp {
        matches!(result_type, Some(BinaryType::Integer(1)))
    } else {
        matches!(result_type, Some(BinaryType::Integer(_)))
    };
    if !valid_result {
        return Err(malformed("integer operation result type is not an integer"));
    }
    let values = match (operand_count, instruction.operands.as_slice()) {
        (1, [BinaryOperand::Value(value)]) => vec![*value],
        (2, [BinaryOperand::Values(values)]) if values.len() == 2 => values.clone(),
        _ => return Err(malformed("integer operation operands are malformed")),
    };
    let mut parameters = Vec::with_capacity(operand_count);
    let mut comparison_type = None;
    for value in &values {
        let value = usize::try_from(*value)
            .map_err(|_| malformed("integer operand index overflows memory"))?;
        let parameter = block
            .parameters
            .get(value)
            .ok_or_else(|| malformed("integer operands must be entry block parameters"))?;
        if is_icmp {
            let operand_type = usize::try_from(parameter.ty)
                .ok()
                .and_then(|index| types.entries.get(index));
            if !matches!(
                operand_type,
                Some(BinaryType::Integer(_) | BinaryType::Pointer(_))
            ) || comparison_type.is_some_and(|ty| ty != parameter.ty)
                || (matches!(operand_type, Some(BinaryType::Pointer(_)))
                    && matches!((instruction.modifiers >> 48) & 0xf, 2..=5))
            {
                return Err(malformed(
                    "icmp operands must have the same integer or pointer type",
                ));
            }
            comparison_type = Some(parameter.ty);
        } else if parameter.ty != function.result {
            return Err(malformed(
                "integer operand type does not match function result",
            ));
        }
        parameters.push(value);
    }
    let operation_name = if is_icmp {
        let predicate = match (instruction.modifiers >> 48) & 0xf {
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
            _ => return Err(malformed("icmp predicate selector is invalid")),
        };
        format!("icmp {predicate}")
    } else {
        opcode_name.to_owned()
    };
    write!(text, "    %{result_name} = {operation_name}").map_err(formatting_error)?;
    for (index, parameter) in parameters.iter().enumerate() {
        text.push_str(if index == 0 { " " } else { ", " });
        write!(text, "%arg{parameter}").map_err(formatting_error)?;
    }
    text.push('\n');
    Ok(())
}

fn format_select(
    text: &mut String,
    instruction: &crate::types::BinaryInstruction,
    result_name: &str,
    block: &crate::types::BinaryBlock,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
) -> Result<(), crate::FormatError> {
    let [BinaryOperand::Values(values)] = instruction.operands.as_slice() else {
        return Err(malformed("select operands are malformed"));
    };
    if values.len() != 3 {
        return Err(malformed("select requires three operands"));
    }
    let indices = values
        .iter()
        .map(|value| {
            usize::try_from(*value).map_err(|_| malformed("select value index overflows memory"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let condition = block
        .parameters
        .get(indices[0])
        .ok_or_else(|| malformed("select condition is not an entry parameter"))?;
    if !matches!(
        usize::try_from(condition.ty)
            .ok()
            .and_then(|index| types.entries.get(index)),
        Some(BinaryType::Integer(1))
    ) {
        return Err(malformed("select condition does not have type i1"));
    }
    let when_true = block
        .parameters
        .get(indices[1])
        .ok_or_else(|| malformed("select value is not an entry parameter"))?;
    let when_false = block
        .parameters
        .get(indices[2])
        .ok_or_else(|| malformed("select value is not an entry parameter"))?;
    if when_true.ty != function.result || when_false.ty != function.result {
        return Err(malformed(
            "select values do not match the function result type",
        ));
    }
    write!(
        text,
        "    %{result_name} = select %arg{}, %arg{}, %arg{}\n",
        indices[0], indices[1], indices[2]
    )
    .map_err(formatting_error)
}

pub(crate) fn integer_operation_name(opcode: u64) -> Option<(&'static str, usize)> {
    match opcode {
        0x020 => Some(("add", 2)),
        0x021 => Some(("sub", 2)),
        0x022 => Some(("mul", 2)),
        0x027 => Some(("sdiv", 2)),
        0x028 => Some(("udiv", 2)),
        0x029 => Some(("srem", 2)),
        0x02a => Some(("urem", 2)),
        0x02b => Some(("and", 2)),
        0x02c => Some(("or", 2)),
        0x02d => Some(("xor", 2)),
        0x02e => Some(("not", 1)),
        0x02f => Some(("neg", 1)),
        0x030 => Some(("abs", 1)),
        0x031 => Some(("shl", 2)),
        0x032 => Some(("lshr", 2)),
        0x033 => Some(("ashr", 2)),
        0x034 => Some(("rotl", 2)),
        0x035 => Some(("rotr", 2)),
        0x036 => Some(("fshl", 3)),
        0x037 => Some(("fshr", 3)),
        0x038 => Some(("smin", 2)),
        0x039 => Some(("smax", 2)),
        0x03a => Some(("umin", 2)),
        0x03b => Some(("umax", 2)),
        0x03c => Some(("icmp", 2)),
        0x03e => Some(("ctlz", 1)),
        0x03f => Some(("cttz", 1)),
        0x040 => Some(("ctpop", 1)),
        0x041 => Some(("bswap", 1)),
        0x042 => Some(("bitreverse", 1)),
        0x043 => Some(("clmul", 2)),
        0x044 => Some(("clmulh", 2)),
        0x045 => Some(("freeze", 1)),
        _ => None,
    }
}

pub(crate) fn valid_integer_operation_modifiers(
    instruction: &crate::types::BinaryInstruction,
) -> bool {
    let allowed = match instruction.opcode {
        0x03c => 0xfu64 << 48,
        0x028 => 1 << 3,
        0x02c => 1 << 4,
        0x03e | 0x03f => 1 << 47,
        _ => 0,
    };
    instruction.modifiers & !allowed == 0
        && (instruction.opcode != 0x03c || (instruction.modifiers >> 48) <= 9)
}

fn format_const_int(
    instruction: &crate::types::BinaryInstruction,
    function_result: u64,
    types: &BinaryTypeTable,
) -> Result<(u16, u128), crate::FormatError> {
    let [BinaryOperand::Type(type_index), BinaryOperand::Bytes(bytes)] =
        instruction.operands.as_slice()
    else {
        return Err(malformed("const.int operands are malformed"));
    };
    if *type_index != function_result {
        return Err(malformed("const.int type does not match function result"));
    }
    let Some(BinaryType::Integer(width)) = usize::try_from(*type_index)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return Err(malformed("const.int result type is not an integer"));
    };
    let width = u16::try_from(*width).map_err(|_| malformed("const.int width exceeds i128"))?;
    if width == 0 || width > 128 {
        return Err(malformed(
            "tirc dis supports const.int widths from i1 to i128",
        ));
    }
    let byte_count = usize::from(width).div_ceil(8);
    if bytes.len() != byte_count {
        return Err(malformed(
            "const.int payload length does not match its type",
        ));
    }
    if width % 8 != 0 && bytes.last().is_some_and(|byte| *byte >> (width % 8) != 0) {
        return Err(malformed("const.int has nonzero unused high bits"));
    }
    let mut raw = [0_u8; 16];
    raw[..bytes.len()].copy_from_slice(bytes);
    Ok((width, u128::from_le_bytes(raw)))
}

fn index(index: u64) -> Result<usize, crate::FormatError> {
    usize::try_from(index).map_err(|_| malformed("function type index overflows memory"))
}

fn formatting_error(_: std::fmt::Error) -> FormatError {
    malformed("TIR text formatting failed")
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
