use crate::text::TokenKind;
use crate::text_function_body_helpers::simple_block_header;
use crate::text_function_constant_assembly::{parse_entry_constant, supported_instruction_shape};
use crate::text_integer_operation_assembly::{integer_operation_info, parse_integer_operation};
use crate::text_select_assembly::parse_select;
use crate::{BasicBlock, FunctionBody, TextItem, TextParseError};
use tir_diag::Span;
use tir_types::{FunctionType, Type};

pub(crate) fn validate_return_body(
    body: &FunctionBody,
    signature: &FunctionType,
    source: &str,
    item: TextItem,
    callees: Option<&crate::text_direct_call::Callees>,
) -> Result<(), TextParseError> {
    if crate::text_control_flow_assembly::uses_supported_control_flow(body) {
        return crate::text_control_flow_assembly::validate_control_flow_body(
            body, signature, source, item, callees,
        );
    }
    if body.blocks.len() != 1 || body.blocks[0].label != "bb0" {
        return Err(item_error(
            item,
            body.span,
            "text assembly currently supports one bb0 block per function",
        ));
    }
    let block = &body.blocks[0];
    let has_unused_result = block.instructions.len() == 2
        && block.instructions[0].results.len() == 1
        && signature.result.as_ref() == &Type::Unit;
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
        if !simple_block_header(block) {
            return Err(item_error(item, block.span, "unsupported block header"));
        }
        if block.parameters.len() != signature.parameters.len()
            || block
                .parameters
                .iter()
                .zip(&signature.parameters)
                .any(|(block, parameter)| {
                    block.context != parameter.context
                        || block.convention != parameter.convention
                        || block.ty != parameter.ty
                })
        {
            return Err(item_error(
                item,
                block.span,
                "entry block parameters do not match the function signature",
            ));
        }
        return crate::text_straight_line_assembly::validate(
            body, signature, source, item, callees,
        );
    }
    if !simple_block_header(block) || !supported_instruction_shape(block) {
        return Err(item_error(
            item,
            block.span,
            "text assembly supports a return, one constant, one integer operation, or select followed by return",
        ));
    }
    if block.parameters.len() != signature.parameters.len()
        || block
            .parameters
            .iter()
            .zip(&signature.parameters)
            .any(|(block, parameter)| {
                block.context != parameter.context
                    || block.convention != parameter.convention
                    || block.ty != parameter.ty
            })
    {
        return Err(item_error(
            item,
            block.span,
            "entry block parameters do not match the function signature",
        ));
    }
    let (constant, integer_binary, selection, instruction) =
        if block.instructions.len() == 2 && block.instructions[0].opcode == "select" {
            let selection = parse_select(&block.instructions[0], item)?;
            if block
                .parameters
                .iter()
                .any(|parameter| parameter.name == selection.name)
            {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "select result duplicates a block parameter name",
                ));
            }
            let condition = find_parameter(block, &selection.condition, item)?;
            if condition.ty != Type::Integer(1) {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "select condition must have type i1",
                ));
            }
            for name in [&selection.when_true, &selection.when_false] {
                if &find_parameter(block, name, item)?.ty != signature.result.as_ref() {
                    return Err(item_error(
                        item,
                        block.instructions[0].span,
                        "select values must match the function result type",
                    ));
                }
            }
            (None, None, Some(selection), &block.instructions[1])
        } else if block.instructions.len() == 2
            && integer_operation_info(&block.instructions[0].opcode).is_some()
        {
            let integer_binary = parse_integer_operation(&block.instructions[0], item)?;
            let is_icmp = integer_binary.opcode == "icmp";
            let valid_result = if is_icmp {
                signature.result.as_ref() == &Type::Integer(1)
            } else {
                matches!(signature.result.as_ref(), Type::Integer(_))
            };
            if !valid_result {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "integer operation result type does not match its operation",
                ));
            }
            if block
                .parameters
                .iter()
                .any(|parameter| parameter.name == integer_binary.name)
            {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "integer operation result duplicates a block parameter name",
                ));
            }
            let mut comparison_type: Option<&Type> = None;
            for operand in &integer_binary.operands {
                let parameter = block
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name.as_str() == operand.as_str())
                    .ok_or_else(|| {
                        item_error(
                            item,
                            block.instructions[0].span,
                            "integer operation operands must be entry block parameters",
                        )
                    })?;
                if is_icmp {
                    if !matches!(&parameter.ty, Type::Integer(_) | Type::Pointer(_))
                        || comparison_type.is_some_and(|ty| ty != &parameter.ty)
                        || (matches!(&parameter.ty, Type::Pointer(_))
                            && matches!(integer_binary.modifiers >> 48, 2..=5))
                    {
                        return Err(item_error(
                            item,
                            block.instructions[0].span,
                            "icmp operands must have the same integer or pointer type",
                        ));
                    }
                    comparison_type = Some(&parameter.ty);
                } else if &parameter.ty != signature.result.as_ref() {
                    return Err(item_error(
                        item,
                        block.instructions[0].span,
                        "integer operation operand type does not match the function result",
                    ));
                }
            }
            (None, Some(integer_binary), None, &block.instructions[1])
        } else if block.instructions.len() == 2 {
            let constant = parse_entry_constant(&block.instructions[0], source, item)?;
            if constant.ty() != signature.result.as_ref() {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "constant type does not match the function result",
                ));
            }
            if block
                .parameters
                .iter()
                .any(|parameter| parameter.name == constant.name())
            {
                return Err(item_error(
                    item,
                    block.instructions[0].span,
                    "constant result duplicates a block parameter name",
                ));
            }
            (Some(constant), None, None, &block.instructions[1])
        } else {
            (None, None, None, &block.instructions[0])
        };
    if signature.result.as_ref() == &Type::Unit {
        if constant.is_some() || integer_binary.is_some() || selection.is_some() {
            return Err(item_error(
                item,
                instruction.span,
                "a value instruction cannot produce a unit function result",
            ));
        }
        if instruction.tokens.len() != 1 {
            return Err(item_error(
                item,
                instruction.span,
                "unit return must not have an operand",
            ));
        }
        return Ok(());
    }
    if instruction.tokens.len() != 3
        || !matches!(
            instruction.tokens.get(1).map(|token| &token.kind),
            Some(TokenKind::Punctuation('%'))
        )
    {
        return Err(item_error(
            item,
            instruction.span,
            "return requires one block parameter operand",
        ));
    }
    let name = match &instruction.tokens[2].kind {
        TokenKind::Atom(name) => name,
        _ => return Err(item_error(item, instruction.span, "invalid return operand")),
    };
    let value_type = if let Some(constant) = &constant {
        if constant.name() != name {
            return Err(item_error(
                item,
                instruction.span,
                "return must use the constant result",
            ));
        }
        constant.ty()
    } else if let Some(integer_binary) = &integer_binary {
        if integer_binary.name.as_str() != name.as_str() {
            return Err(item_error(
                item,
                instruction.span,
                "return must use the integer operation result",
            ));
        }
        signature.result.as_ref()
    } else if let Some(selection) = &selection {
        if selection.name.as_str() != name.as_str() {
            return Err(item_error(
                item,
                instruction.span,
                "return must use the select result",
            ));
        }
        signature.result.as_ref()
    } else {
        &block
            .parameters
            .iter()
            .find(|parameter| parameter.name.as_str() == name.as_str())
            .ok_or_else(|| {
                item_error(
                    item,
                    instruction.span,
                    "return operand is not a block parameter",
                )
            })?
            .ty
    };
    if value_type != signature.result.as_ref() {
        return Err(item_error(
            item,
            instruction.span,
            "return operand type does not match the function result",
        ));
    }
    Ok(())
}

fn find_parameter<'a>(
    block: &'a BasicBlock,
    name: &str,
    item: TextItem,
) -> Result<&'a crate::TextParameter, TextParseError> {
    block
        .parameters
        .iter()
        .find(|parameter| parameter.name == name)
        .ok_or_else(|| item_error(item, block.span, "value is not an entry block parameter"))
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
