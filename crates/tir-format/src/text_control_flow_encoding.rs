use crate::text_control_flow_assembly::{ControlTarget, ControlTerminator, parse_terminator};
use crate::types::{
    BinaryBlock, BinaryBlockParameter, BinaryFunctionBody, BinaryFunctionType, BinaryType,
    BinaryTypeTable,
};
use crate::{FunctionBody, TextItem, TextParseError};
use std::collections::HashMap;
use tir_diag::Span;
use tir_types::{Convention, Type};

pub(crate) fn encode_control_flow_body(
    body: &FunctionBody,
    function_index: u64,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    float_constants: &std::collections::HashMap<usize, (u64, u64)>,
    string_constants: &std::collections::HashMap<usize, u64>,
    callees: &crate::text_direct_call::Callees,
    block_parameter_indices: &[Vec<u64>],
    source: &str,
    item: TextItem,
) -> Result<BinaryFunctionBody, TextParseError> {
    let result_type = usize::try_from(function.result)
        .ok()
        .and_then(|index| types.entries.get(index))
        .ok_or_else(|| item_error(item, body.span, "function result type is missing"))?;
    let returns_unit = matches!(result_type, BinaryType::Unit);
    let mut values: Vec<&str> = body.blocks[0]
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    if block_parameter_indices.len() != body.blocks.len() {
        return Err(item_error(
            item,
            body.span,
            "missing CODE block parameter type indexes",
        ));
    }
    let labels = body
        .blocks
        .iter()
        .map(|block| block.label.as_str())
        .collect::<Vec<_>>();
    let mut inferred = body
        .blocks
        .iter()
        .flat_map(|block| &block.parameters)
        .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
        .collect::<HashMap<String, Type>>();
    for block in &body.blocks {
        let (_, operations) = block
            .instructions
            .split_last()
            .ok_or_else(|| item_error(item, block.span, "CODE control block has no terminator"))?;
        crate::text_straight_line_validation::validate_operations(
            operations,
            &mut inferred,
            source,
            item,
            Some(callees),
        )?;
    }
    let mut blocks = Vec::with_capacity(body.blocks.len());

    for block in &body.blocks {
        let block_index = blocks.len();
        if block_index != 0 {
            values.extend(
                block
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.as_str()),
            );
        }
        if block_index != 0 && block_parameter_indices[block_index].len() != block.parameters.len()
        {
            return Err(item_error(
                item,
                block.span,
                "CODE parameter type indexes do not match block",
            ));
        }
        let (instruction, operations) = block
            .instructions
            .split_last()
            .ok_or_else(|| item_error(item, block.span, "block has no terminator"))?;
        let terminator = parse_terminator(instruction, returns_unit, source, item)?;
        let mut instruction_bytes = Vec::new();
        crate::text_straight_line_assembly::encode_operations(
            operations,
            &mut values,
            &inferred,
            types,
            float_constants,
            string_constants,
            callees,
            source,
            item,
            &mut instruction_bytes,
        )?;
        let current_value = u64::try_from(values.len())
            .map_err(|_| item_error(item, instruction.span, "CODE SSA value count overflows"))?;
        match terminator {
            ControlTerminator::Branch(target) => {
                write_uleb(0x1e0, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_target(
                    &values,
                    &labels,
                    current_value,
                    &target,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
            }
            ControlTerminator::Conditional {
                condition,
                when_true,
                when_false,
                merge,
            } => {
                write_uleb(0x1e1, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_value_delta(
                    &values,
                    &condition,
                    current_value,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
                write_target(
                    &values,
                    &labels,
                    current_value,
                    &when_true,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
                write_target(
                    &values,
                    &labels,
                    current_value,
                    &when_false,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
                let merge = merge.map_or(Ok(0), |label| {
                    target_index(&labels, &label, item, instruction.span).and_then(|index| {
                        index.checked_add(1).ok_or_else(|| {
                            item_error(item, instruction.span, "merge block index overflows")
                        })
                    })
                })?;
                write_uleb(merge, &mut instruction_bytes, item, instruction.span)?;
            }
            ControlTerminator::Switch(switch) => {
                crate::text_switch_int::encode(
                    &switch,
                    &values,
                    &labels,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
            }
            ControlTerminator::EnumSwitch(switch) => {
                crate::text_switch_enum::encode(
                    &switch,
                    body,
                    types,
                    block_parameter_indices,
                    &values,
                    &labels,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
            }
            ControlTerminator::Indirect(branch) => {
                crate::text_indirect_branch::encode(
                    &branch,
                    &values,
                    &labels,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
            }
            ControlTerminator::Return(value) => {
                write_uleb(0x1e5, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                if let Some(value) = value {
                    write_value_delta(
                        &values,
                        &value,
                        current_value,
                        &mut instruction_bytes,
                        item,
                        instruction.span,
                    )?;
                }
            }
            ControlTerminator::Fail(value) => {
                write_uleb(0x1e6, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_value_delta(
                    &values,
                    &value,
                    current_value,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
            }
            ControlTerminator::Unreachable { undefined_behavior } => {
                write_uleb(0x1e8, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(
                    if undefined_behavior { 1 << 48 } else { 0 },
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
            }
            ControlTerminator::Trap(code) => {
                write_uleb(0x1e9, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
                crate::leb::write_integer_immediate(
                    false,
                    u128::from(code),
                    &mut instruction_bytes,
                )
                .map_err(|error| item_error(item, instruction.span, error.to_string()))?;
            }
            ControlTerminator::TailCall(call) => {
                crate::text_direct_call::encode_tail(
                    &call,
                    &values,
                    callees,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
            }
            ControlTerminator::Longjmp(longjmp) => {
                crate::text_longjmp::encode(
                    &longjmp,
                    &values,
                    &mut instruction_bytes,
                    item,
                    instruction.span,
                )?;
            }
        }

        let parameters = if block.label == "bb0" {
            function
                .parameters
                .iter()
                .map(|parameter| BinaryBlockParameter {
                    convention: parameter.convention,
                    context: parameter.context,
                    ty: parameter.ty,
                })
                .collect()
        } else {
            block
                .parameters
                .iter()
                .zip(&block_parameter_indices[block_index])
                .map(|(parameter, ty)| BinaryBlockParameter {
                    convention: match parameter.convention {
                        Some(Convention::Owned) => 0,
                        Some(Convention::Borrow) => 1,
                        Some(Convention::BorrowMut) => 2,
                        None => 3,
                    },
                    context: parameter.context,
                    ty: *ty,
                })
                .collect()
        };
        blocks.push(BinaryBlock {
            cold: false,
            loop_merge: None,
            loop_continue: None,
            parameters,
            instruction_count: block.instructions.len() as u64,
            instruction_bytes,
            instructions: None,
        });
    }

    Ok(BinaryFunctionBody {
        function_index,
        body_byte_size: 0,
        block_count: u64::try_from(blocks.len())
            .map_err(|_| item_error(item, body.span, "block count overflows"))?,
        blocks,
    })
}

pub(crate) fn write_target(
    values: &[&str],
    labels: &[&str],
    current_value: u64,
    target: &ControlTarget,
    output: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let target_index = target_index(labels, &target.label, item, span)?;
    write_uleb(target_index, output, item, span)?;
    write_uleb(
        u64::try_from(target.arguments.len())
            .map_err(|_| item_error(item, span, "branch argument count overflows"))?,
        output,
        item,
        span,
    )?;
    for argument in &target.arguments {
        write_value_delta(values, argument, current_value, output, item, span)?;
    }
    Ok(())
}

pub(crate) fn target_index(
    labels: &[&str],
    label: &str,
    item: TextItem,
    span: Span,
) -> Result<u64, TextParseError> {
    labels
        .iter()
        .position(|candidate| *candidate == label)
        .and_then(|index| u64::try_from(index).ok())
        .ok_or_else(|| item_error(item, span, "branch target does not name a block"))
}

pub(crate) fn write_value_delta(
    values: &[&str],
    name: &str,
    current_value: u64,
    output: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index = values
        .iter()
        .position(|candidate| *candidate == name)
        .ok_or_else(|| item_error(item, span, "value has no SSA definition"))?;
    let delta = current_value
        .checked_sub(index as u64)
        .filter(|delta| *delta != 0)
        .ok_or_else(|| item_error(item, span, "invalid SSA value reference"))?;
    write_uleb(delta, output, item, span)
}

pub(crate) fn write_uleb(
    value: u64,
    output: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    crate::leb::write_uleb(value, output)
        .map_err(|error| item_error(item, span, format!("CODE encoding failed: {error}")))
}

fn item_error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
