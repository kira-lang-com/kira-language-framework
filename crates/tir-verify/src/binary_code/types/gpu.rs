use tir_format::{BinaryInstruction, BinaryType, BinaryTypeTable};
use tir_types::FloatType;

use super::{float_type, integer_type, selector, type_at, type_index, vector_type};

mod advanced_types;
mod collective_types;
mod geometry_types;
mod ray_types;
mod texture_types;

pub(crate) fn verify_texture_instructions(
    body: &tir_format::BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    texture_types::verify(body, value_types, types, function_name)
}

pub(crate) fn verify_ray_instructions(
    body: &tir_format::BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    ray_types::verify(body, value_types, types, function_name)
}

pub(crate) fn verify_advanced_instructions(
    body: &tir_format::BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    advanced_types::verify(body, value_types, types, function_name)
}

pub(crate) fn verify_geometry_instructions(
    body: &tir_format::BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    geometry_types::verify(body, value_types, types, function_name)
}

pub(crate) fn verify_collective_instructions(
    body: &tir_format::BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<tir_diag::Diagnostic> {
    collective_types::verify(body, value_types, types, function_name)
}

pub(super) fn infer_texture_results(
    instruction: &BinaryInstruction,
    texture_type_index: Option<u64>,
    types: &BinaryTypeTable,
    results: &mut [Option<u64>],
) -> Option<u64> {
    let Some(BinaryType::Texture {
        dimension,
        component,
        ..
    }) = texture_type_index.and_then(|index| type_at(types, index))
    else {
        return None;
    };
    let result = match instruction.opcode {
        0x484 | 0x485 => float_type(types, FloatType::F32),
        0x486 => gather_type(types, *component),
        0x487 => vector_type(types, 4, float_type(types, FloatType::F32)?),
        0x48b => match selector(instruction) {
            3 => match dimension {
                0 => integer_type(types, 32),
                1 | 3 | 4 => vector_type(types, 2, integer_type(types, 32)?),
                2 | 5 | 6 => vector_type(types, 3, integer_type(types, 32)?),
                _ => None,
            },
            _ => integer_type(types, 32),
        },
        0x48c => vector_type(types, 2, float_type(types, FloatType::F32)?),
        _ => Some(*component),
    }?;
    if results.len() > 1 {
        results[1] = integer_type(types, 1);
    }
    Some(result)
}

pub(super) fn infer_ray_results(
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    results: &mut [Option<u64>],
) -> Option<u64> {
    let attribute = instruction
        .operands
        .iter()
        .rev()
        .find_map(|operand| match operand {
            tir_format::BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    match instruction.opcode {
        0x501 | 0x507 => integer_type(types, 1),
        0x505 | 0x510 => infer_query_attribute(types, attribute?, results),
        0x50b => infer_trace_attribute(types, attribute?, results),
        0x50c | 0x50d | 0x50e => type_index(types, |ty| matches!(ty, BinaryType::HitObject)),
        _ => None,
    }
}

fn infer_query_attribute(
    types: &BinaryTypeTable,
    attribute: u64,
    results: &mut [Option<u64>],
) -> Option<u64> {
    let result = match attribute {
        0 | 2..=5 => integer_type(types, 32),
        1 | 10 | 11 => float_type(types, FloatType::F32),
        6 => vector_type(types, 2, float_type(types, FloatType::F32)?),
        7 => integer_type(types, 1),
        8 | 9 => vector_type(types, 3, float_type(types, FloatType::F32)?),
        _ => None,
    }?;
    if attribute >= 10 {
        results.fill(Some(float_type(types, FloatType::F32)?));
    }
    Some(result)
}

fn infer_trace_attribute(
    types: &BinaryTypeTable,
    attribute: u64,
    results: &mut [Option<u64>],
) -> Option<u64> {
    let result = match attribute {
        0 | 1 => vector_type(types, 3, integer_type(types, 32)?),
        2..=5 => vector_type(types, 3, float_type(types, FloatType::F32)?),
        6 | 7 | 14 | 15 => float_type(types, FloatType::F32),
        8..=13 => integer_type(types, 32),
        16 => float_type(types, FloatType::F32),
        _ => None,
    }?;
    if matches!(attribute, 14..=16) {
        results.fill(Some(float_type(types, FloatType::F32)?));
    }
    Some(result)
}

fn gather_type(types: &BinaryTypeTable, component: u64) -> Option<u64> {
    let component = match type_at(types, component)? {
        BinaryType::FixedVector { lanes: 4, element } => *element,
        _ => component,
    };
    vector_type(types, 4, component)
}
