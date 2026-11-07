use std::collections::HashMap;

use tir_types::{FunctionType, Type};

use crate::text::TokenKind;
use crate::text_function_constant_assembly::{ParsedEntryConstant, parse_entry_constant};
use crate::text_integer_operation_assembly::{integer_operation_info, parse_integer_operation};
use crate::text_straight_line_encoding_helpers as helpers;
pub(crate) use crate::text_straight_line_encoding_helpers::error;
use crate::text_straight_line_encoding_helpers::{trivial_constant_type_index, unsupported, write};
use crate::types::{BinaryBlock, BinaryBlockParameter, BinaryFunctionBody, BinaryFunctionType};
use crate::types::{BinaryType, BinaryTypeTable};
use crate::{FunctionBody, TextItem, TextParseError};

pub(crate) fn validate(
    body: &FunctionBody,
    signature: &FunctionType,
    source: &str,
    item: TextItem,
    callees: Option<&crate::text_direct_call::Callees>,
) -> Result<(), TextParseError> {
    let block = &body.blocks[0];
    let mut values = block
        .parameters
        .iter()
        .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
        .collect::<HashMap<_, _>>();
    if values.len() != block.parameters.len() {
        return Err(error(item, block.span, "duplicate entry block parameter"));
    }
    let Some((last, instructions)) = block.instructions.split_last() else {
        return Err(error(item, block.span, "function body has no terminator"));
    };
    validate_operations(instructions, &mut values, source, item, callees)?;
    if last.opcode != "return" || !last.results.is_empty() {
        return Err(error(
            item,
            last.span,
            "straight-line function must end in return",
        ));
    }
    match signature.result.as_ref() {
        Type::Unit if last.tokens.len() == 1 => Ok(()),
        Type::Unit => Err(error(
            item,
            last.span,
            "unit return must not have an operand",
        )),
        result => {
            let [_, percent, name] = last.tokens.as_slice() else {
                return Err(error(item, last.span, "return requires one SSA operand"));
            };
            if !matches!(&percent.kind, TokenKind::Punctuation('%')) {
                return Err(error(item, last.span, "return requires one SSA operand"));
            }
            let TokenKind::Atom(name) = &name.kind else {
                return Err(error(item, last.span, "invalid return operand"));
            };
            if values.get(name) != Some(result) {
                return Err(error(
                    item,
                    last.span,
                    "return operand has the wrong type or is undefined",
                ));
            }
            Ok(())
        }
    }
}

pub(crate) use crate::text_straight_line_validation::validate_operations;

pub(crate) fn encode(
    body: &FunctionBody,
    function_index: u64,
    function: &BinaryFunctionType,
    types: &BinaryTypeTable,
    float_constants: &std::collections::HashMap<usize, (u64, u64)>,
    string_constants: &std::collections::HashMap<usize, u64>,
    callees: &crate::text_direct_call::Callees,
    source: &str,
    item: TextItem,
) -> Result<BinaryFunctionBody, TextParseError> {
    let block = &body.blocks[0];
    let mut values: Vec<&str> = block
        .parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    let mut inferred = block
        .parameters
        .iter()
        .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
        .collect::<HashMap<_, _>>();
    validate_operations(
        &block.instructions[..block.instructions.len() - 1],
        &mut inferred,
        source,
        item,
        Some(callees),
    )?;
    let mut bytes = Vec::new();
    encode_operations(
        &block.instructions[..block.instructions.len() - 1],
        &mut values,
        &inferred,
        types,
        float_constants,
        string_constants,
        callees,
        source,
        item,
        &mut bytes,
    )?;
    let terminator = block.instructions.last().expect("validated return");
    crate::text_straight_line_encoding_helpers::encode_return(
        terminator, &values, &mut bytes, item,
    )?;
    Ok(BinaryFunctionBody {
        function_index,
        body_byte_size: 0,
        block_count: 1,
        blocks: vec![BinaryBlock {
            cold: false,
            loop_merge: None,
            loop_continue: None,
            parameters: function
                .parameters
                .iter()
                .map(|parameter| BinaryBlockParameter {
                    convention: parameter.convention,
                    context: parameter.context,
                    ty: parameter.ty,
                })
                .collect(),
            instruction_count: block.instructions.len() as u64,
            instruction_bytes: bytes,
            instructions: None,
        }],
    })
}

pub(crate) fn encode_operations<'a>(
    instructions: &'a [crate::TextInstruction],
    values: &mut Vec<&'a str>,
    inferred: &HashMap<String, Type>,
    types: &BinaryTypeTable,
    float_constants: &std::collections::HashMap<usize, (u64, u64)>,
    string_constants: &std::collections::HashMap<usize, u64>,
    callees: &crate::text_direct_call::Callees,
    source: &str,
    item: TextItem,
    mut bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    for instruction in instructions {
        if crate::text_gpu_sync::handles(&instruction.opcode) {
            crate::text_gpu_sync::encode(instruction, source, item, &mut bytes)?;
        } else if instruction.opcode == "gpu.builtin" {
            crate::text_gpu_builtin::encode(
                instruction,
                source,
                item,
                types,
                string_constants,
                &mut bytes,
            )?;
        } else if matches!(
            instruction.opcode.as_str(),
            "const.vec" | "const.aggregate" | "const.sampler" | "gpu.spec_const"
        ) {
            crate::text_straight_line_constants::encode(
                instruction,
                source,
                item,
                types,
                float_constants,
                callees,
                &mut bytes,
            )?;
        } else if crate::text_pointer_address::handles(&instruction.opcode) {
            let operation = crate::text_pointer_address::parse(instruction, source, item)?;
            crate::text_pointer_address::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_enum_address::handles(&instruction.opcode) {
            let operation = crate::text_enum_address::parse(instruction, source, item)?;
            crate::text_enum_address::encode(
                &operation,
                values,
                inferred,
                callees,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_address_projection::handles(&instruction.opcode) {
            let projection = crate::text_address_projection::parse(instruction, source, item)?;
            crate::text_address_projection::encode(
                &projection,
                values,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_enum_payload::handles(&instruction.opcode) {
            let operation = crate::text_enum_payload::parse(instruction, source, item)?;
            crate::text_enum_payload::encode(
                &operation,
                values,
                inferred,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_aggregate_destructure::handles(&instruction.opcode) {
            let operation = crate::text_aggregate_destructure::parse(instruction, source, item)?;
            crate::text_aggregate_destructure::encode(
                &operation,
                values,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_nominal_construction::handles(&instruction.opcode) {
            let operation = crate::text_nominal_construction::parse(instruction, source, item)?;
            crate::text_nominal_construction::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_vector_gather::handles(&instruction.opcode) {
            let operation = crate::text_vector_gather::parse(instruction, source, item)?;
            crate::text_vector_gather::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_vector_memory::handles(&instruction.opcode) {
            let operation = crate::text_vector_memory::parse(instruction, source, item)?;
            crate::text_vector_memory::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_managed_heap::handles(&instruction.opcode) {
            let managed = crate::text_managed_heap::parse(instruction, source, item)?;
            crate::text_managed_heap::encode(
                &managed,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_global_reference::handles(&instruction.opcode) {
            let global = crate::text_global_reference::parse(instruction, source, item)?;
            crate::text_global_reference::encode(
                &global,
                callees,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if instruction.opcode == "descriptor.ref" {
            let descriptor = crate::text_descriptor_reference::parse(instruction, source, item)?;
            crate::text_descriptor_reference::encode(
                &descriptor,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if instruction.opcode == "func.ref" {
            let reference = crate::text_function_reference::parse(instruction, source, item)?;
            crate::text_function_reference::encode(
                &reference,
                callees,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if instruction.opcode == "block.addr" {
            let address = crate::text_block_address::parse(instruction, source, item)?;
            crate::text_block_address::encode(&address, &mut bytes, item, instruction.span)?;
        } else if instruction.opcode == "call" {
            let operation = crate::text_direct_call::parse(instruction, source, item)?;
            crate::text_direct_call::encode(
                &operation,
                values,
                callees,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if instruction.opcode == "call.indirect" {
            let operation = crate::text_indirect_call::parse(instruction, source, item)?;
            crate::text_indirect_call::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_simple_instructions::handles(&instruction.opcode) {
            let operation = crate::text_simple_instructions::parse(instruction, source, item)?;
            crate::text_simple_instructions::encode(
                &operation,
                values,
                &mut bytes,
                string_constants.get(&instruction.span.start).copied(),
                item,
                instruction.span,
            )?;
        } else if crate::text_tuple_simple::handles(&instruction.opcode) {
            let operation = crate::text_tuple_simple::parse(instruction, source, item)?;
            crate::text_tuple_simple::encode(
                &operation,
                values,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_array_simple::handles(&instruction.opcode) {
            let operation = crate::text_array_simple::parse(instruction, source, item)?;
            crate::text_array_simple::encode(
                &operation,
                values,
                &mut bytes,
                types,
                item,
                instruction.span,
            )?;
        } else if crate::text_raw_memory::opcode(&instruction.opcode).is_some() {
            let operation = crate::text_raw_memory::parse(instruction, source, item)?;
            crate::text_raw_memory::encode(&operation, values, &mut bytes, item, instruction.span)?;
        } else if crate::text_raw_access::handles(&instruction.opcode) {
            let operation = crate::text_raw_access::parse(instruction, source, item)?;
            crate::text_raw_access::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_device_io::handles(&instruction.opcode) {
            let operation = crate::text_device_io::parse(instruction, source, item)?;
            crate::text_device_io::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_raw_heap::handles(&instruction.opcode) {
            let operation = crate::text_raw_heap::parse(instruction, source, item)?;
            crate::text_raw_heap::encode(&operation, values, &mut bytes, item, instruction.span)?;
        } else if crate::text_fp_misc::handles(&instruction.opcode) {
            let operation = crate::text_fp_misc::parse(instruction, source, item)?;
            crate::text_fp_misc::encode(&operation, values, &mut bytes, item, instruction.span)?;
        } else if crate::text_atomic_simple::handles(&instruction.opcode) {
            let operation = crate::text_atomic_simple::parse(instruction, source, item)?;
            crate::text_atomic_simple::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_atomic_wait_notify::handles(&instruction.opcode) {
            let operation = crate::text_atomic_wait_notify::parse(instruction, source, item)?;
            crate::text_atomic_wait_notify::encode(
                &operation,
                values,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_atomic_update::handles(&instruction.opcode) {
            let operation = crate::text_atomic_update::parse(instruction, source, item)?;
            crate::text_atomic_update::encode(
                &operation,
                values,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_varargs_simple::handles(&instruction.opcode) {
            let operation = crate::text_varargs_simple::parse(instruction, source, item)?;
            crate::text_varargs_simple::encode(
                &operation,
                values,
                types,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if let Some((opcode, _)) = integer_operation_info(&instruction.opcode) {
            let parsed = parse_integer_operation(instruction, item)?;
            write(opcode, &mut bytes, item, instruction.span)?;
            write(parsed.modifiers, &mut bytes, item, instruction.span)?;
            write(1, &mut bytes, item, instruction.span)?;
            for operand in &parsed.operands {
                let position = values
                    .iter()
                    .position(|name| name == operand)
                    .ok_or_else(|| error(item, instruction.span, "undefined SSA operand"))?;
                write(
                    (values.len() - position) as u64,
                    &mut bytes,
                    item,
                    instruction.span,
                )?;
            }
        } else if crate::text_float_assembly::operation_info(&instruction.opcode).is_some() {
            let parsed = crate::text_float_assembly::parse(instruction, item)?;
            write(parsed.opcode, &mut bytes, item, instruction.span)?;
            write(parsed.modifiers, &mut bytes, item, instruction.span)?;
            write(1, &mut bytes, item, instruction.span)?;
            for operand in &parsed.operands {
                let position = values
                    .iter()
                    .position(|name| name == operand)
                    .ok_or_else(|| {
                        error(item, instruction.span, "undefined floating-point operand")
                    })?;
                write(
                    (values.len() - position) as u64,
                    &mut bytes,
                    item,
                    instruction.span,
                )?;
            }
        } else if crate::text_vector_permute::handles(&instruction.opcode) {
            let operation = crate::text_vector_permute::parse(instruction, source, item)?;
            crate::text_vector_permute::encode(
                &operation,
                values,
                &mut bytes,
                item,
                instruction.span,
            )?;
        } else if crate::text_vector_simple::opcode(&instruction.opcode).is_some() {
            let parsed = crate::text_vector_simple::parse(instruction, source, item)?;
            let ty = parsed
                .ty
                .as_ref()
                .map(|ty| {
                    crate::text_conversion_assembly::target_index(types, ty).ok_or_else(|| {
                        error(item, instruction.span, "vector target type is not interned")
                    })
                })
                .transpose()?;
            write(parsed.opcode, &mut bytes, item, instruction.span)?;
            write(parsed.modifiers, &mut bytes, item, instruction.span)?;
            write(1, &mut bytes, item, instruction.span)?;
            for operand in &parsed.args {
                let position = values
                    .iter()
                    .position(|name| name == operand)
                    .ok_or_else(|| error(item, instruction.span, "undefined vector operand"))?;
                write(
                    (values.len() - position) as u64,
                    &mut bytes,
                    item,
                    instruction.span,
                )?;
            }
            if let Some(index) = ty {
                write(index as u64, &mut bytes, item, instruction.span)?;
            }
        } else if crate::text_conversion_assembly::opcode(&instruction.opcode).is_some() {
            let conversion = crate::text_conversion_assembly::parse(instruction, source, item)?;
            let position = values
                .iter()
                .position(|name| name == &conversion.source)
                .ok_or_else(|| error(item, instruction.span, "undefined conversion operand"))?;
            let type_index =
                crate::text_conversion_assembly::target_index(types, &conversion.target)
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "conversion target type is not interned",
                        )
                    })?;
            write(conversion.opcode, &mut bytes, item, instruction.span)?;
            write(conversion.modifiers, &mut bytes, item, instruction.span)?;
            write(1, &mut bytes, item, instruction.span)?;
            write(
                (values.len() - position) as u64,
                &mut bytes,
                item,
                instruction.span,
            )?;
            write(type_index as u64, &mut bytes, item, instruction.span)?;
        } else if matches!(
            instruction.opcode.as_str(),
            "const.int"
                | "const.bool"
                | "const.zero"
                | "const.float"
                | "const.null"
                | "const.poison"
                | "const.bytes"
                | "const.string"
        ) {
            let constant = parse_entry_constant(instruction, source, item)?;
            let zero_type = constant.ty().clone();
            match constant {
                ParsedEntryConstant::Integer(value) => {
                    let Type::Integer(width) = value.ty else {
                        return Err(error(
                            item,
                            instruction.span,
                            "const.int must have an integer type",
                        ));
                    };
                    let ty = types.entries.iter().position(|ty| matches!(ty, BinaryType::Integer(bits) if *bits == u64::from(width)))
                        .ok_or_else(|| error(item, instruction.span, "const.int type is not interned"))?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    write(ty as u64, &mut bytes, item, instruction.span)?;
                    crate::leb::write_integer_bits(value.value, width, &mut bytes).map_err(
                        |error_message| {
                            error(
                                item,
                                instruction.span,
                                format!("const.int encoding failed: {error_message}"),
                            )
                        },
                    )?;
                }
                ParsedEntryConstant::Boolean(value) => {
                    write(0x002, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    crate::leb::write_integer_immediate(false, u128::from(value.value), &mut bytes)
                        .map_err(|error_message| {
                            error(
                                item,
                                instruction.span,
                                format!("const.bool encoding failed: {error_message}"),
                            )
                        })?;
                }
                ParsedEntryConstant::Float(value) => {
                    let (type_index, constant_index) = float_constants
                        .get(&instruction.span.start)
                        .copied()
                        .ok_or_else(|| {
                            error(item, instruction.span, "const.float has no CONSTANTS entry")
                        })?;
                    let parsed_type = types.entries.get(type_index as usize);
                    if !matches!(parsed_type, Some(BinaryType::Float(float))
                        if value.ty == Type::Float(*float))
                    {
                        return Err(error(
                            item,
                            instruction.span,
                            "const.float CONSTANTS type mismatch",
                        ));
                    }
                    write(0x001, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    write(type_index, &mut bytes, item, instruction.span)?;
                    write(constant_index, &mut bytes, item, instruction.span)?;
                }
                ParsedEntryConstant::String(_) => {
                    let string_index = string_constants
                        .get(&instruction.span.start)
                        .copied()
                        .ok_or_else(|| {
                            error(
                                item,
                                instruction.span,
                                "const.string is not interned in STRINGS",
                            )
                        })?;
                    write(0x006, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    write(string_index, &mut bytes, item, instruction.span)?;
                }
                ParsedEntryConstant::Null(value) => {
                    if !types.entries.iter().any(|entry|
                        matches!(entry, BinaryType::Pointer(space) if *space == value.space_code))
                    {
                        return Err(error(item, instruction.span, "const.null pointer type is not interned"));
                    }
                    write(0x003, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    bytes.push(value.space_code);
                }
                ParsedEntryConstant::Poison(_) => {
                    let index = trivial_constant_type_index(types, &zero_type)
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "const.poison type is not interned or is not a supported trivial type",
                        )
                    })?;
                    write(0x005, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    write(index as u64, &mut bytes, item, instruction.span)?;
                }
                ParsedEntryConstant::Bytes(value) => {
                    let Some(index) =
                        crate::text_memory_assembly::supported_type_index(types, &value.ty)
                    else {
                        return Err(error(
                            item,
                            instruction.span,
                            "const.bytes type is not a supported scalar type",
                        ));
                    };
                    let expected_bytes = match value.ty {
                        Type::Integer(width) if width % 8 == 0 => usize::from(width / 8),
                        Type::Float(float) if float.bits() % 8 == 0 => {
                            usize::from(float.bits() / 8)
                        }
                        _ => {
                            return Err(error(
                                item,
                                instruction.span,
                                "const.bytes requires a byte-aligned scalar type",
                            ));
                        }
                    };
                    if expected_bytes != value.bytes.len() {
                        return Err(error(
                            item,
                            instruction.span,
                            "const.bytes payload length must match its scalar type",
                        ));
                    }
                    write(0x007, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    write(index as u64, &mut bytes, item, instruction.span)?;
                    write(value.bytes.len() as u64, &mut bytes, item, instruction.span)?;
                    bytes.extend_from_slice(&value.bytes);
                }
                ParsedEntryConstant::Zero(_) => {
                    let type_index = trivial_constant_type_index(types, &zero_type)
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "const.zero type is not interned or is not a supported scalar/vector",
                        )
                    })?;
                    write(0x004, &mut bytes, item, instruction.span)?;
                    write(0, &mut bytes, item, instruction.span)?;
                    write(1, &mut bytes, item, instruction.span)?;
                    write(type_index as u64, &mut bytes, item, instruction.span)?;
                }
            }
        } else if crate::text_memory_assembly::handles(&instruction.opcode) {
            let parsed = crate::text_memory_assembly::parse(instruction, source, item)?;
            parsed.encode(values, types, &mut bytes, item, instruction.span)?;
        } else if crate::text_access_assembly::handles(&instruction.opcode) {
            helpers::encode_access(instruction, values, &mut bytes, item)?;
        } else if matches!(
            instruction.opcode.as_str(),
            "copy" | "move" | "destroy" | "borrow.begin" | "borrow.end"
        ) {
            helpers::encode_ownership(instruction, values, &mut bytes, item)?;
        } else if instruction.opcode == "select" {
            helpers::encode_select(instruction, values, &mut bytes, item)?;
        } else {
            return Err(unsupported(item, instruction.span));
        }
        values.extend(instruction.results.iter().map(String::as_str));
    }
    Ok(())
}
