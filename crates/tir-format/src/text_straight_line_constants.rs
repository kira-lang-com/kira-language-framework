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
    if crate::text_gpu_texture_atomic::handles(&instruction.opcode) {
        crate::text_gpu_texture_atomic::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if crate::text_gpu_texture_sample::handles(&instruction.opcode) {
        crate::text_gpu_texture_sample::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if crate::text_gpu_texture_basic::handles(&instruction.opcode) {
        crate::text_gpu_texture_basic::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if crate::text_gpu_fragment::handles(&instruction.opcode) {
        crate::text_gpu_fragment::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if crate::text_gpu_tile_node::handles(&instruction.opcode) {
        crate::text_gpu_tile_node::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if crate::text_gpu_mesh::handles(&instruction.opcode) {
        crate::text_gpu_mesh::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if crate::text_gpu_subgroup::handles(&instruction.opcode) {
        crate::text_gpu_subgroup::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if crate::text_gpu_quad::handles(&instruction.opcode) {
        crate::text_gpu_quad::validate(instruction, source, item, values)?;
        return Ok(true);
    }
    if instruction.opcode == "gpu.printf" {
        crate::text_gpu_printf::validate(instruction, item, values)?;
        return Ok(true);
    }
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

pub(crate) fn handles(opcode: &str) -> bool {
    crate::text_gpu_texture_atomic::handles(opcode)
        || crate::text_gpu_texture_sample::handles(opcode)
        || crate::text_gpu_texture_basic::handles(opcode)
        || crate::text_gpu_tile_node::handles(opcode)
        || crate::text_gpu_mesh::handles(opcode)
        || crate::text_gpu_subgroup::handles(opcode)
        || crate::text_gpu_quad::handles(opcode)
        || crate::text_gpu_fragment::handles(opcode)
        || crate::text_gpu_sync::handles(opcode)
        || matches!(
            opcode,
            "gpu.builtin"
                | "gpu.spec_const"
                | "gpu.printf"
                | "const.vec"
                | "const.aggregate"
                | "const.sampler"
        )
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    types: &BinaryTypeTable,
    constant_indices: &HashMap<usize, (u64, u64)>,
    string_indices: &HashMap<usize, u64>,
    values: &[&str],
    symbols: &crate::text_direct_call::Callees,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    match instruction.opcode.as_str() {
        opcode if crate::text_gpu_texture_atomic::handles(opcode) => {
            crate::text_gpu_texture_atomic::encode(instruction, source, item, values, bytes)?;
        }
        opcode if crate::text_gpu_texture_sample::handles(opcode) => {
            crate::text_gpu_texture_sample::encode(instruction, source, item, values, bytes)?;
        }
        opcode if crate::text_gpu_texture_basic::handles(opcode) => {
            crate::text_gpu_texture_basic::encode(instruction, source, item, values, bytes)?;
        }
        opcode if crate::text_gpu_tile_node::handles(opcode) => {
            crate::text_gpu_tile_node::encode(instruction, source, item, values, bytes)?;
        }
        opcode if crate::text_gpu_mesh::handles(opcode) => {
            crate::text_gpu_mesh::encode(instruction, source, item, values, bytes)?;
        }
        opcode if crate::text_gpu_subgroup::handles(opcode) => {
            crate::text_gpu_subgroup::encode(instruction, source, item, values, bytes)?;
        }
        opcode if crate::text_gpu_quad::handles(opcode) => {
            crate::text_gpu_quad::encode(instruction, source, item, values, bytes)?;
        }
        "gpu.printf" => {
            crate::text_gpu_printf::encode(instruction, item, string_indices, values, bytes)?
        }
        opcode if crate::text_gpu_fragment::handles(opcode) => {
            crate::text_gpu_fragment::encode(instruction, source, item, bytes)?;
        }
        opcode if crate::text_gpu_sync::handles(opcode) => {
            crate::text_gpu_sync::encode(instruction, source, item, bytes)?;
        }
        "gpu.builtin" => {
            crate::text_gpu_builtin::encode(
                instruction,
                source,
                item,
                types,
                string_indices,
                bytes,
            )?;
        }
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
        0x48a => crate::text_gpu_texture_atomic::format(text, operation, types, values)?,
        0x480..=0x487 | 0x48c | 0x48d => {
            crate::text_gpu_texture_sample_format::format(text, operation, types, values)?;
        }
        0x488 | 0x489 | 0x48b => {
            crate::text_gpu_texture_basic::format(text, operation, types, values)?;
        }
        0x5a0 | 0x5c0..=0x5c5 => {
            crate::text_gpu_tile_node_format::format(text, operation, types, values)?;
        }
        0x4c0..=0x4c4 => crate::text_gpu_mesh::format(text, operation, types, values)?,
        0x440..=0x452 => crate::text_gpu_subgroup_format::format(text, operation, types, values)?,
        0x420..=0x426 => crate::text_gpu_quad::format(text, operation, types, values)?,
        0x40a => crate::text_gpu_printf::format(text, operation, strings, values)?,
        0x405..=0x409 => crate::text_gpu_fragment::format(text, operation, types, values)?,
        0x401 => crate::text_gpu_spec_const::format(text, operation, types, constants, values)?,
        0x400 => crate::text_gpu_builtin::format(text, operation, types, strings, values)?,
        0x011 => crate::text_sampler_constant::format(text, operation, types, values)?,
        0x009 => crate::text_aggregate_constant::format(text, operation, types, constants, values)?,
        0x008 => crate::text_vector_constant::format(text, operation, types, constants, values)?,
        _ => return Ok(false),
    }
    Ok(true)
}
