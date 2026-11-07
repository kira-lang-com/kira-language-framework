use crate::text::{Token, TokenKind};
use crate::text_function_constant_assembly::{ParsedEntryConstant, parse_entry_constant};
use crate::text_integer_operation_assembly::{integer_operation_info, parse_integer_operation};
use crate::text_select_assembly::parse_select;
use crate::types::{
    BinaryBlock, BinaryBlockParameter, BinaryFunctionBody, BinaryFunctionType, BinaryType,
    BinaryTypeTable,
};
use crate::{FunctionBody, TextItem, TextParseError};
use tir_diag::Span;
use tir_types::Type;

pub(crate) fn encode_return_body(
    body: &FunctionBody,
    function_index: u64,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    float_constant: Option<(u64, u64)>,
    float_constants: &std::collections::HashMap<usize, (u64, u64)>,
    string_constant: Option<u64>,
    string_constants: &std::collections::HashMap<usize, u64>,
    callees: &crate::text_direct_call::Callees,
    block_parameter_indices: &[Vec<u64>],
    source: &str,
    item: TextItem,
) -> Result<BinaryFunctionBody, TextParseError> {
    if crate::text_control_flow_assembly::uses_supported_control_flow(body) {
        return crate::text_control_flow_encoding::encode_control_flow_body(
            body,
            function_index,
            function,
            types,
            float_constants,
            string_constants,
            callees,
            block_parameter_indices,
            source,
            item,
        );
    }
    let block = body
        .blocks
        .first()
        .ok_or_else(|| item_error(item, body.span, "function body has no block"))?;
    let has_unused_result = block.instructions.len() == 2
        && block.instructions[0].results.len() == 1
        && matches!(
            types.entries.get(function.result as usize),
            Some(BinaryType::Unit)
        );
    if block.instructions.len() > 2
        || has_unused_result
        || (block.instructions.len() == 2
            && (crate::text_integer_operation_assembly::integer_operation_info(
                &block.instructions[0].opcode,
            )
            .is_some()
                || matches!(
                    block.instructions[0].opcode.as_str(),
                    "copy" | "move" | "destroy" | "borrow.begin" | "borrow.end"
                )
                || crate::text_memory_assembly::handles(&block.instructions[0].opcode)
                || crate::text_conversion_assembly::opcode(&block.instructions[0].opcode)
                    .is_some()
                || crate::text_vector_simple::opcode(&block.instructions[0].opcode).is_some()
                || crate::text_vector_permute::handles(&block.instructions[0].opcode)
                || crate::text_vector_memory::handles(&block.instructions[0].opcode)
                || crate::text_vector_gather::handles(&block.instructions[0].opcode)
                || crate::text_simple_instructions::handles(&block.instructions[0].opcode)
                || crate::text_tuple_simple::handles(&block.instructions[0].opcode)
                || crate::text_array_simple::handles(&block.instructions[0].opcode)
                || crate::text_nominal_construction::handles(&block.instructions[0].opcode)
                || crate::text_aggregate_destructure::handles(&block.instructions[0].opcode)
                || crate::text_enum_payload::handles(&block.instructions[0].opcode)
                || crate::text_address_projection::handles(&block.instructions[0].opcode)
                || crate::text_enum_address::handles(&block.instructions[0].opcode)
                || crate::text_pointer_address::handles(&block.instructions[0].opcode)
                || block.instructions[0].opcode == "const.vec"
                || block.instructions[0].opcode == "const.aggregate"
                || block.instructions[0].opcode == "const.sampler"
                || block.instructions[0].opcode == "gpu.builtin"
                || block.instructions[0].opcode == "gpu.spec_const"
                || crate::text_gpu_sync::handles(&block.instructions[0].opcode)
                || crate::text_raw_memory::opcode(&block.instructions[0].opcode).is_some()
                || crate::text_raw_access::handles(&block.instructions[0].opcode)
                || crate::text_device_io::handles(&block.instructions[0].opcode)
                || crate::text_raw_heap::handles(&block.instructions[0].opcode)
                || crate::text_fp_misc::handles(&block.instructions[0].opcode)
                || crate::text_atomic_simple::handles(&block.instructions[0].opcode)
                || crate::text_atomic_wait_notify::handles(&block.instructions[0].opcode)
                || crate::text_atomic_update::handles(&block.instructions[0].opcode)
                || crate::text_varargs_simple::handles(&block.instructions[0].opcode)
                || block.instructions[0].opcode == "call"
                || block.instructions[0].opcode == "call.indirect"
                || block.instructions[0].opcode == "block.addr"
                || block.instructions[0].opcode == "func.ref"
                || block.instructions[0].opcode == "descriptor.ref"
                || crate::text_global_reference::handles(&block.instructions[0].opcode)
                || crate::text_managed_heap::handles(&block.instructions[0].opcode)
                || crate::text_access_assembly::handles(&block.instructions[0].opcode)))
    {
        return crate::text_straight_line_assembly::encode(
            body,
            function_index,
            function,
            types,
            float_constants,
            string_constants,
            callees,
            source,
            item,
        );
    }
    let mut instruction_bytes = Vec::new();
    let (constant, integer_binary, selection, instruction) =
        if block.instructions.len() == 2 && block.instructions[0].opcode == "select" {
            let selection = parse_select(&block.instructions[0], item)?;
            write_uleb(
                0x03d,
                &mut instruction_bytes,
                item,
                block.instructions[0].span,
            )?;
            write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
            write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
            let current_value = u64::try_from(block.parameters.len())
                .map_err(|_| item_error(item, block.span, "block parameter count overflows"))?;
            for name in [
                &selection.condition,
                &selection.when_true,
                &selection.when_false,
            ] {
                let value = block
                    .parameters
                    .iter()
                    .position(|parameter| parameter.name.as_str() == name.as_str())
                    .ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "select operand is not an entry block parameter",
                        )
                    })?;
                let value = u64::try_from(value)
                    .map_err(|_| item_error(item, block.span, "select operand index overflows"))?;
                let delta = current_value
                    .checked_sub(value)
                    .filter(|delta| *delta != 0)
                    .ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "invalid select operand value",
                        )
                    })?;
                write_uleb(
                    delta,
                    &mut instruction_bytes,
                    item,
                    block.instructions[0].span,
                )?;
            }
            (None, None, Some(selection), &block.instructions[1])
        } else if block.instructions.len() == 2
            && integer_operation_info(&block.instructions[0].opcode).is_some()
        {
            let integer_binary = parse_integer_operation(&block.instructions[0], item)?;
            let result_type = usize::try_from(function.result)
                .ok()
                .and_then(|index| types.entries.get(index));
            let valid_result = if integer_binary.opcode == "icmp" {
                matches!(result_type, Some(BinaryType::Integer(1)))
            } else {
                matches!(result_type, Some(BinaryType::Integer(_)))
            };
            if !valid_result {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "integer operation requires an integer function result",
                ));
            }
            write_uleb(
                integer_operation_info(&integer_binary.opcode)
                    .map(|(opcode, _)| opcode)
                    .ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "unsupported integer operation",
                        )
                    })?,
                &mut instruction_bytes,
                item,
                block.instructions[0].span,
            )?;
            write_uleb(
                integer_binary.modifiers,
                &mut instruction_bytes,
                item,
                block.instructions[0].span,
            )?;
            write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
            let current_value = u64::try_from(block.parameters.len())
                .map_err(|_| item_error(item, block.span, "block parameter count overflows"))?;
            for name in &integer_binary.operands {
                let value = block
                    .parameters
                    .iter()
                    .position(|parameter| parameter.name.as_str() == name.as_str())
                    .ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "integer operation operand is not an entry block parameter",
                        )
                    })?;
                let value = u64::try_from(value)
                    .map_err(|_| item_error(item, block.span, "integer operand index overflows"))?;
                let delta = current_value
                    .checked_sub(value)
                    .filter(|delta| *delta != 0)
                    .ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "invalid integer operand value",
                        )
                    })?;
                write_uleb(
                    delta,
                    &mut instruction_bytes,
                    item,
                    block.instructions[0].span,
                )?;
            }
            (None, Some(integer_binary), None, &block.instructions[1])
        } else if block.instructions.len() == 2 {
            let constant = parse_entry_constant(&block.instructions[0], source, item)?;
            if usize::try_from(function.result)
                .ok()
                .and_then(|index| types.entries.get(index))
                .is_none()
            {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "function result type is missing",
                ));
            }
            match &constant {
                ParsedEntryConstant::Integer(value) => {
                    let BinaryType::Integer(binary_width) = usize::try_from(function.result)
                        .ok()
                        .and_then(|index| types.entries.get(index))
                        .ok_or_else(|| {
                            item_error(
                                item,
                                block.instructions[0].span,
                                "function result type is missing",
                            )
                        })?
                    else {
                        return Err(item_error(
                            item,
                            block.instructions[0].span,
                            "const.int requires an integer function result",
                        ));
                    };
                    let width = u16::try_from(*binary_width).map_err(|_| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "const.int width exceeds the text integer range",
                        )
                    })?;
                    if value.ty != Type::Integer(width) {
                        return Err(item_error(
                            item,
                            block.instructions[0].span,
                            "const.int type does not match the function result",
                        ));
                    }
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(
                        function.result,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    let byte_count = usize::from(width).div_ceil(8);
                    instruction_bytes.extend_from_slice(&value.value.to_le_bytes()[..byte_count]);
                }
                ParsedEntryConstant::Boolean(value) => {
                    if !matches!(
                        usize::try_from(function.result)
                            .ok()
                            .and_then(|index| types.entries.get(index)),
                        Some(BinaryType::Integer(1))
                    ) {
                        return Err(item_error(
                            item,
                            block.instructions[0].span,
                            "const.bool requires an i1 function result",
                        ));
                    }
                    write_uleb(
                        0x002,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    crate::leb::write_integer_immediate(
                        false,
                        u128::from(value.value),
                        &mut instruction_bytes,
                    )
                    .map_err(|error| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            format!("CODE encoding failed: {error}"),
                        )
                    })?;
                }
                ParsedEntryConstant::Float(_) => {
                    let (type_index, constant_index) = float_constant.ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "const.float constant was not assigned a CONSTANTS entry",
                        )
                    })?;
                    if type_index != function.result
                        || !matches!(
                            usize::try_from(type_index)
                                .ok()
                                .and_then(|index| types.entries.get(index)),
                            Some(BinaryType::Float(_))
                        )
                    {
                        return Err(item_error(
                            item,
                            block.instructions[0].span,
                            "const.float type does not match the function result",
                        ));
                    }
                    write_uleb(
                        0x001,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(
                        type_index,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(
                        constant_index,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                }
                ParsedEntryConstant::Null(value) => {
                    let BinaryType::Pointer(space) = usize::try_from(function.result)
                        .ok()
                        .and_then(|index| types.entries.get(index))
                        .ok_or_else(|| {
                            item_error(
                                item,
                                block.instructions[0].span,
                                "function result type is missing",
                            )
                        })?
                    else {
                        return Err(item_error(
                            item,
                            block.instructions[0].span,
                            "const.null requires a pointer function result",
                        ));
                    };
                    if *space != value.space_code {
                        return Err(item_error(
                            item,
                            block.instructions[0].span,
                            "const.null address space does not match the function result",
                        ));
                    }
                    write_uleb(
                        0x003,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    instruction_bytes.push(value.space_code);
                }
                ParsedEntryConstant::Zero(_) => {
                    write_uleb(
                        0x004,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(
                        function.result,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                }
                ParsedEntryConstant::Poison(_) => {
                    write_uleb(
                        0x005,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(
                        function.result,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                }
                ParsedEntryConstant::Bytes(value) => {
                    write_uleb(
                        0x007,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(
                        function.result,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(
                        u64::try_from(value.bytes.len()).map_err(|_| {
                            item_error(
                                item,
                                block.instructions[0].span,
                                "const.bytes length overflows",
                            )
                        })?,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    instruction_bytes.extend_from_slice(&value.bytes);
                }
                ParsedEntryConstant::String(_) => {
                    let string_index = string_constant.ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "const.string was not assigned a STRINGS entry",
                        )
                    })?;
                    write_uleb(
                        0x006,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                    write_uleb(0, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(1, &mut instruction_bytes, item, block.instructions[0].span)?;
                    write_uleb(
                        string_index,
                        &mut instruction_bytes,
                        item,
                        block.instructions[0].span,
                    )?;
                }
            }
            (Some(constant), None, None, &block.instructions[1])
        } else {
            let instruction = block
                .instructions
                .first()
                .ok_or_else(|| item_error(item, block.span, "function body has no return"))?;
            (None, None, None, instruction)
        };
    write_uleb(0x1e5, &mut instruction_bytes, item, instruction.span)?;
    write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
    write_uleb(0, &mut instruction_bytes, item, instruction.span)?;
    let returns_unit = matches!(
        usize::try_from(function.result)
            .ok()
            .and_then(|index| types.entries.get(index)),
        Some(BinaryType::Unit)
    );
    if !returns_unit {
        let name = return_value_name(&instruction.tokens, item, instruction.span)?;
        let value = if constant
            .as_ref()
            .is_some_and(|constant| constant.name() == name)
            || integer_binary
                .as_ref()
                .is_some_and(|operation| operation.name.as_str() == name)
            || selection
                .as_ref()
                .is_some_and(|selection| selection.name.as_str() == name)
        {
            block.parameters.len()
        } else {
            block
                .parameters
                .iter()
                .position(|parameter| parameter.name == name)
                .ok_or_else(|| {
                    item_error(
                        item,
                        instruction.span,
                        "return operand is not a block parameter",
                    )
                })?
        };
        let value = u64::try_from(value)
            .map_err(|_| item_error(item, instruction.span, "return value index overflows"))?;
        let parameter_count = u64::try_from(block.parameters.len())
            .map_err(|_| item_error(item, instruction.span, "block parameter count overflows"))?;
        let current_value = parameter_count
            .checked_add(u64::from(
                constant.is_some() || integer_binary.is_some() || selection.is_some(),
            ))
            .ok_or_else(|| item_error(item, instruction.span, "SSA value index overflows"))?;
        let delta = current_value.checked_sub(value).ok_or_else(|| {
            item_error(
                item,
                instruction.span,
                "return value is not an earlier SSA value",
            )
        })?;
        if delta == 0 {
            return Err(item_error(
                item,
                instruction.span,
                "return value is not an earlier SSA value",
            ));
        }
        write_uleb(delta, &mut instruction_bytes, item, instruction.span)?;
    }
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| BinaryBlockParameter {
            convention: parameter.convention,
            context: parameter.context,
            ty: parameter.ty,
        })
        .collect();
    Ok(BinaryFunctionBody {
        function_index,
        body_byte_size: 0,
        block_count: 1,
        blocks: vec![BinaryBlock {
            cold: false,
            loop_merge: None,
            loop_continue: None,
            parameters,
            instruction_count: block.instructions.len() as u64,
            instruction_bytes,
            instructions: None,
        }],
    })
}

fn return_value_name<'a>(
    tokens: &'a [Token],
    item: TextItem,
    span: Span,
) -> Result<&'a str, TextParseError> {
    if tokens.len() != 3
        || !matches!(
            tokens.get(1).map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
    {
        return Err(item_error(
            item,
            span,
            "return requires one block parameter operand",
        ));
    }
    match tokens.get(2).map(|token| &token.kind) {
        Some(TokenKind::Atom(name)) => Ok(name),
        _ => Err(item_error(item, span, "invalid return operand")),
    }
}

fn write_uleb(
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
