use std::fmt::Write as _;

use crate::FormatError;
use crate::types::{
    BinaryConstantTable, BinaryFunctionBody, BinaryFunctionType, BinaryGlobalTable, BinaryOperand,
    BinaryType, BinaryTypeTable,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn format_control_flow_body(
    text: &mut String,
    body: &BinaryFunctionBody,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    signatures: &crate::types::BinarySignatureTable,
    constants: &BinaryConstantTable,
    globals: &BinaryGlobalTable,
    strings: &crate::StringTable,
) -> Result<(), FormatError> {
    if body.block_count == 0 || usize::try_from(body.block_count).ok() != Some(body.blocks.len()) {
        return Err(malformed("CODE block count does not match its body"));
    }
    let returns_unit = matches!(
        usize::try_from(function.result)
            .ok()
            .and_then(|index| types.entries.get(index)),
        Some(BinaryType::Unit)
    );
    text.push_str(" {\n");
    let mut value_names: Vec<(String, u64)> = Vec::new();
    for (block_index, block) in body.blocks.iter().enumerate() {
        if block.cold || block.loop_merge.is_some() || block.loop_continue.is_some() {
            return Err(malformed(
                "tirc dis does not support annotated control-flow blocks",
            ));
        }
        if block_index == 0 {
            if block.parameters.len() != function.parameters.len() {
                return Err(malformed(
                    "entry block parameters do not match function signature",
                ));
            }
        }
        if block_index != 0 {
            text.push('\n');
        }
        write!(text, "  bb{block_index}(").map_err(formatting_error)?;
        {
            for (parameter_index, parameter) in block.parameters.iter().enumerate() {
                if parameter_index != 0 {
                    text.push_str(", ");
                }
                let name = if block_index == 0 {
                    format!("arg{parameter_index}")
                } else {
                    format!("b{block_index}arg{parameter_index}")
                };
                value_names.push((name.clone(), parameter.ty));
                write!(text, "%{name}: ").map_err(formatting_error)?;
                if parameter.context {
                    text.push_str("context ");
                }
                match parameter.convention {
                    0 => text.push_str("owned "),
                    1 => text.push_str("borrow "),
                    2 => text.push_str("borrowmut "),
                    3 => {}
                    _ => return Err(malformed("unknown function parameter convention")),
                }
                text.push_str(&crate::text_type_disassembly::format_type_reference(
                    types,
                    parameter.ty,
                    signatures,
                )?);
            }
        }
        text.push_str("):\n");
        let instructions = block
            .instructions
            .as_ref()
            .ok_or_else(|| malformed("tirc dis cannot format an unsupported instruction"))?;
        let (instruction, operations) = instructions
            .split_last()
            .ok_or_else(|| malformed("CODE block has no terminator"))?;
        if block.instruction_count != instructions.len() as u64 {
            return Err(malformed("CODE instruction count mismatch"));
        }
        crate::text_straight_line_disassembly::format_operations(
            text,
            operations,
            types,
            signatures,
            constants,
            globals,
            strings,
            &mut value_names,
        )?;
        text.push_str("    ");
        if !instruction.terminator
            || instruction.result_count != 0
            || (instruction.modifiers != 0
                && !(instruction.opcode == 0x1e8 && instruction.modifiers == (1 << 48))
                && !(instruction.opcode == 0x1e7
                    && crate::text_call_attributes::canonical(instruction.modifiers).is_ok()))
        {
            return Err(malformed("invalid control-flow terminator"));
        }
        match instruction.opcode {
            0x1e2 => crate::text_switch_int::format(text, instruction, types, &value_names)?,
            0x1e3 => crate::text_switch_enum::format(
                text,
                instruction,
                types,
                &value_names,
                &body.blocks,
            )?,
            0x1e4 => crate::text_indirect_branch::format(
                text,
                instruction,
                types,
                &value_names,
                body.blocks.len(),
                &body
                    .blocks
                    .iter()
                    .map(|block| block.parameters.len())
                    .collect::<Vec<_>>(),
            )?,
            0x1e0 => {
                let [BinaryOperand::BlockTarget { block, arguments }] =
                    instruction.operands.as_slice()
                else {
                    return Err(malformed("invalid br operands"));
                };
                text.push_str("br ");
                format_target(text, *block, arguments, &value_names)?;
                text.push('\n');
            }
            0x1e1 => {
                let [
                    BinaryOperand::Value(condition),
                    BinaryOperand::BlockTarget {
                        block: when_true,
                        arguments: true_arguments,
                    },
                    BinaryOperand::BlockTarget {
                        block: when_false,
                        arguments: false_arguments,
                    },
                    BinaryOperand::Index(merge),
                ] = instruction.operands.as_slice()
                else {
                    return Err(malformed("invalid cond.br operands"));
                };
                write!(text, "cond.br %{}, ", value_name(*condition, &value_names)?)
                    .map_err(formatting_error)?;
                format_target(text, *when_true, true_arguments, &value_names)?;
                text.push_str(", ");
                format_target(text, *when_false, false_arguments, &value_names)?;
                if *merge != 0 {
                    let merge_block = *merge - 1;
                    if merge_block >= body.block_count {
                        return Err(malformed("cond.br merge target is out of range"));
                    }
                    write!(text, " merge bb{merge_block}").map_err(formatting_error)?;
                }
                text.push('\n');
            }
            0x1e5 if returns_unit => {
                if !instruction.operands.is_empty() {
                    return Err(malformed("unit return has an operand"));
                }
                text.push_str("return\n");
            }
            0x1e5 => {
                let [BinaryOperand::Value(value)] = instruction.operands.as_slice() else {
                    return Err(malformed("invalid return operand"));
                };
                writeln!(text, "return %{}", value_name(*value, &value_names)?)
                    .map_err(formatting_error)?;
            }
            0x1e6 => {
                let [BinaryOperand::Value(value)] = instruction.operands.as_slice() else {
                    return Err(malformed("fail must carry exactly one SSA value"));
                };
                let Some(expected) = function.error else {
                    return Err(malformed("fail requires a function with an error type"));
                };
                let actual = value_names
                    .get(*value as usize)
                    .ok_or_else(|| malformed("fail uses an undefined SSA value"))?
                    .1;
                if actual != expected {
                    return Err(malformed(
                        "fail operand type does not match function error type",
                    ));
                }
                writeln!(text, "fail %{}", value_name(*value, &value_names)?)
                    .map_err(formatting_error)?;
            }
            0x1e8 if instruction.operands.is_empty() => {
                if instruction.modifiers == (1 << 48) {
                    text.push_str("unreachable [ub]\n");
                } else {
                    text.push_str("unreachable\n");
                }
            }
            0x1e9 => {
                let [BinaryOperand::Integer(code)] = instruction.operands.as_slice() else {
                    return Err(malformed("trap requires an integer code"));
                };
                if code.negative || code.magnitude > u128::from(u16::MAX) {
                    return Err(malformed("trap code does not fit in u16"));
                }
                writeln!(text, "trap {}", code.magnitude).map_err(formatting_error)?;
            }
            0x1e7 => {
                crate::text_direct_call::format_tail(
                    text,
                    instruction,
                    function,
                    types,
                    signatures,
                    &value_names,
                )?;
            }
            0x215 => crate::text_longjmp::format(text, instruction, types, &value_names)?,
            _ => return Err(malformed("unsupported control-flow terminator")),
        }
    }
    text.push('}');
    Ok(())
}

pub(crate) fn value_name(value: u64, names: &[(String, u64)]) -> Result<&str, FormatError> {
    usize::try_from(value)
        .ok()
        .and_then(|index| names.get(index))
        .map(|(name, _)| name.as_str())
        .ok_or_else(|| malformed("control-flow operand is not a defined SSA value"))
}

pub(crate) fn format_target(
    text: &mut String,
    block: u64,
    arguments: &[u64],
    names: &[(String, u64)],
) -> Result<(), FormatError> {
    write!(text, "bb{block}(").map_err(formatting_error)?;
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            text.push_str(", ");
        }
        write!(text, "%{}", value_name(*argument, names)?).map_err(formatting_error)?;
    }
    text.push(')');
    Ok(())
}

fn formatting_error(_: std::fmt::Error) -> FormatError {
    malformed("text formatting failed")
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
