//! Shared CODE handling for text vector and aggregate constants.
use std::collections::HashMap;
use tir_types::Type;

use crate::types::BinaryTypeTable;
use crate::types::{BinaryConstantTable, BinaryInstruction};
use crate::{FormatError, StringTable};
use crate::{TextInstruction, TextItem, TextParseError};

pub(crate) fn validate(
    instruction: &TextInstruction,
    values: &mut HashMap<String, Type>,
    source: &str,
    item: TextItem,
    symbols: Option<&crate::text_direct_call::Callees>,
) -> Result<bool, TextParseError> {
    if crate::text_gpu_sync::handles(&instruction.opcode) {
        crate::text_gpu_sync::parse(instruction, source, item)?;
        return Ok(true);
    }
    if instruction.opcode == "gpu.builtin" {
        crate::text_gpu_builtin::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if instruction.opcode == "gpu.spec_const" {
        crate::text_gpu_spec_const::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    let (name, ty) = match instruction.opcode.as_str() {
        "const.sampler" => {
            let constant = crate::text_sampler_constant::parse(instruction, source, item)?;
            crate::text_sampler_constant::validate(&constant, values, item, instruction.span)?
        }
        "const.vec" => {
            let constant = crate::text_vector_constant::parse(instruction, source, item)?;
            crate::text_vector_constant::validate(&constant, values, item, instruction.span)?
        }
        "const.aggregate" => {
            let constant =
                crate::text_aggregate_constant::parse(instruction, source, item, symbols)?;
            crate::text_aggregate_constant::validate(&constant, values, item, instruction.span)?
        }
        _ => return Ok(false),
    };
    values.insert(name, ty);
    Ok(true)
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    types: &BinaryTypeTable,
    constant_indices: &HashMap<usize, (u64, u64)>,
    symbols: &crate::text_direct_call::Callees,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    match instruction.opcode.as_str() {
        "gpu.spec_const" => crate::text_gpu_spec_const::encode(
            instruction,
            source,
            item,
            types,
            constant_indices,
            bytes,
        )?,
        "const.sampler" => crate::text_sampler_constant::encode(instruction, source, item, bytes)?,
        "const.vec" => crate::text_vector_constant::encode_instruction(
            instruction,
            source,
            item,
            types,
            constant_indices,
            bytes,
        )?,
        "const.aggregate" => crate::text_aggregate_constant::encode(
            instruction,
            source,
            item,
            types,
            constant_indices,
            symbols,
            bytes,
        )?,
        _ => {
            return Err(crate::text_straight_line_encoding_helpers::error(
                item,
                instruction.span,
                "unsupported aggregate/vector constant opcode",
            ));
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    operation: &BinaryInstruction,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    strings: &StringTable,
    values: &mut Vec<(String, u64)>,
) -> Result<bool, FormatError> {
    match operation.opcode {
        0x401 => crate::text_gpu_spec_const::format(text, operation, types, constants, values)?,
        0x400 => crate::text_gpu_builtin::format(text, operation, types, strings, values)?,
        0x011 => crate::text_sampler_constant::format(text, operation, types, values)?,
        0x009 => crate::text_aggregate_constant::format(text, operation, types, constants, values)?,
        0x008 => crate::text_vector_constant::format(text, operation, types, constants, values)?,
        _ => return Ok(false),
    }
    Ok(true)
}
