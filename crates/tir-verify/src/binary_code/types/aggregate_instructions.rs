use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType,
    BinaryTypeTable,
};

mod borrow;
pub(super) mod properties;

use properties::{enum_tag_type, is_loadable, is_trivial, nominal_definition, substitute};

pub(in crate::binary_code) fn is_loadable_type(types: &BinaryTypeTable, ty: u64) -> bool {
    is_loadable(types, ty, &mut HashSet::new(), 0)
}

pub(in crate::binary_code) fn is_trivial_type(types: &BinaryTypeTable, ty: u64) -> bool {
    is_trivial(types, ty, 0)
}

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const UNCHECKED: u64 = 1 << 33;

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let borrowed = borrow::borrowed_values(body);
    let mut diagnostics = Vec::new();
    let mut value_cursor = 0usize;
    for block in &body.blocks {
        value_cursor = value_cursor.saturating_add(block.parameters.len());
        let Some(instructions) = &block.instructions else {
            value_cursor = value_cursor.saturating_add(block.instruction_count as usize);
            continue;
        };
        for instruction in instructions {
            if (0x100..=0x10d).contains(&instruction.opcode)
                && !verify_instruction(instruction, value_types, value_cursor, types, &borrowed)
            {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function @{function_name} CODE offset {}: aggregate opcode 0x{:03X} has incompatible operands, result type, or ownership",
                        instruction.offset, instruction.opcode
                    ),
                ));
            }
            value_cursor = value_cursor.saturating_add(instruction.result_count as usize);
        }
    }
    diagnostics
}

pub(super) fn is_type_trivial(types: &BinaryTypeTable, ty: u64) -> bool {
    properties::is_trivial(types, ty, 0)
}

pub(super) fn is_type_loadable(types: &BinaryTypeTable, ty: u64) -> bool {
    properties::is_loadable(types, ty, &mut HashSet::new(), 0)
}

pub(super) fn is_type_copyable(types: &BinaryTypeTable, ty: u64) -> bool {
    properties::is_copyable(types, ty, 0)
}

pub(in crate::binary_code) fn borrowed_values(body: &BinaryFunctionBody) -> HashSet<u64> {
    borrow::borrowed_values(body)
}

fn verify_instruction(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result_start: usize,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
) -> bool {
    let relative = instruction.opcode - 0x100;
    let allowed_modifiers = if relative == 8 { UNCHECKED } else { 0 };
    if instruction.modifiers & !allowed_modifiers != 0 {
        return false;
    }
    if !matches!(relative, 3 | 6) && instruction.result_count != 1 {
        return false;
    }
    let operand_ids = value_ids(instruction);
    let Some(values) = resolved_values(&operand_ids, value_types) else {
        return false;
    };
    let result = |offset| result_type(value_types, result_start, offset);
    match relative {
        0 => verify_struct_new(
            instruction,
            &values,
            &operand_ids,
            result(0),
            types,
            borrowed,
        ),
        1 => verify_struct_extract(
            instruction,
            &values,
            &operand_ids,
            result(0),
            types,
            borrowed,
        ),
        2 => verify_struct_insert(instruction, &values, result(0), types),
        3 => verify_destructure(
            instruction,
            &values,
            result_start,
            value_types,
            types,
            &operand_ids,
            borrowed,
        ),
        4 => verify_tuple_new(
            instruction,
            &values,
            &operand_ids,
            result(0),
            types,
            borrowed,
        ),
        5 => verify_tuple_extract(
            instruction,
            &values,
            &operand_ids,
            result(0),
            types,
            borrowed,
        ),
        6 => verify_destructure(
            instruction,
            &values,
            result_start,
            value_types,
            types,
            &operand_ids,
            borrowed,
        ),
        7 => verify_array_new(
            instruction,
            &values,
            &operand_ids,
            result(0),
            types,
            borrowed,
        ),
        8 => verify_array_extract(&values, result(0), types),
        9 => verify_array_insert(&values, result(0), types),
        10 => verify_enum_new(
            instruction,
            &values,
            &operand_ids,
            result(0),
            types,
            borrowed,
        ),
        11 => verify_enum_tag(&values, result(0), types),
        12 | 13 => verify_enum_payload(
            instruction,
            &values,
            &operand_ids,
            result(0),
            types,
            borrowed,
            relative == 13,
        ),
        _ => false,
    }
}

fn verify_struct_new(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_ids: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
) -> bool {
    let Some(target) = type_operand(instruction) else {
        return false;
    };
    let Some((definition, arguments)) =
        nominal_definition(types, target, BinaryNominalKind::Struct)
    else {
        return false;
    };
    let Some(fields) = definition
        .fields
        .iter()
        .map(|field| substitute(types, field.ty, arguments))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    values.len() == fields.len()
        && fields
            .iter()
            .zip(values)
            .zip(value_ids)
            .all(|((field, value), id)| {
                same_type(types, *value, *field)
                    && (is_trivial(types, *field, 0) || !borrowed.contains(id))
            })
        && is_loadable(types, target, &mut HashSet::new(), 0)
        && result.is_some_and(|actual| same_type(types, actual, target))
        && instruction.result_count == 1
}

fn verify_struct_extract(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_ids: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
) -> bool {
    let (Some(source), Some(index)) = (values.first(), index_operand(instruction)) else {
        return false;
    };
    let Some((definition, arguments)) =
        nominal_definition(types, *source, BinaryNominalKind::Struct)
    else {
        return false;
    };
    let Some(field) = usize::try_from(index)
        .ok()
        .and_then(|index| definition.fields.get(index))
        .and_then(|field| substitute(types, field.ty, arguments))
    else {
        return false;
    };
    values.len() == 1
        && value_ids.len() == 1
        && result.is_some_and(|actual| same_type(types, actual, field))
        && instruction.result_count == 1
        && (is_trivial(types, field, 0) || borrowed.contains(&value_ids[0]))
}

fn verify_struct_insert(
    instruction: &BinaryInstruction,
    values: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 2 {
        return false;
    }
    let source = values[0];
    let Some((definition, arguments)) =
        nominal_definition(types, source, BinaryNominalKind::Struct)
    else {
        return false;
    };
    let Some(field) = index_operand(instruction)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| definition.fields.get(index))
        .and_then(|field| substitute(types, field.ty, arguments))
    else {
        return false;
    };
    result.is_some_and(|actual| same_type(types, actual, source))
        && instruction.result_count == 1
        && same_type(types, values[1], field)
        && is_trivial(types, source, 0)
}

fn verify_destructure(
    instruction: &BinaryInstruction,
    values: &[u64],
    result_start: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    value_ids: &[u64],
    borrowed: &HashSet<u64>,
) -> bool {
    if values.len() != 1 || value_ids.len() != 1 || borrowed.contains(&value_ids[0]) {
        return false;
    }
    let fields = match (instruction.opcode, type_entry(types, values[0])) {
        (0x103, Some(BinaryType::Nominal { .. })) => {
            let source = values[0];
            let Some((definition, arguments)) =
                nominal_definition(types, source, BinaryNominalKind::Struct)
            else {
                return false;
            };
            definition
                .fields
                .iter()
                .map(|field| substitute(types, field.ty, arguments))
                .collect::<Option<Vec<_>>>()
        }
        (0x106, Some(BinaryType::Tuple(fields))) => Some(fields.clone()),
        _ => None,
    };
    let Some(fields) = fields else {
        return false;
    };
    u64::try_from(fields.len()).ok() == Some(instruction.result_count)
        && fields.iter().enumerate().all(|(offset, field)| {
            result_type(value_types, result_start, offset)
                .is_some_and(|actual| same_type(types, actual, *field))
        })
}

fn verify_tuple_new(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_ids: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
) -> bool {
    let Some(result) = result else {
        return false;
    };
    let Some(actual) = type_entry(types, result) else {
        return false;
    };
    let BinaryType::Tuple(fields) = actual else {
        return false;
    };
    values.len() == fields.len()
        && fields
            .iter()
            .zip(values)
            .zip(value_ids)
            .all(|((field, value), id)| {
                same_type(types, *value, *field)
                    && (is_trivial(types, *field, 0) || !borrowed.contains(id))
            })
        && instruction.result_count == 1
        && is_loadable(types, result, &mut HashSet::new(), 0)
}

fn verify_tuple_extract(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_ids: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
) -> bool {
    let (Some(source), Some(index)) = (values.first(), index_operand(instruction)) else {
        return false;
    };
    let Some(BinaryType::Tuple(fields)) = type_entry(types, *source) else {
        return false;
    };
    let Some(field) = usize::try_from(index)
        .ok()
        .and_then(|index| fields.get(index))
    else {
        return false;
    };
    values.len() == 1
        && result.is_some_and(|actual| same_type(types, actual, *field))
        && instruction.result_count == 1
        && (is_trivial(types, *field, 0)
            || value_ids
                .first()
                .is_some_and(|source| borrowed.contains(source)))
}

fn verify_array_new(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_ids: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
) -> bool {
    let Some(target) = type_operand(instruction) else {
        return false;
    };
    let Some(BinaryType::Array { count, element }) = type_entry(types, target) else {
        return false;
    };
    u64::try_from(values.len()).ok() == Some(*count)
        && values.iter().zip(value_ids).all(|(value, id)| {
            same_type(types, *value, *element)
                && (is_trivial(types, *element, 0) || !borrowed.contains(id))
        })
        && is_loadable(types, target, &mut HashSet::new(), 0)
        && result.is_some_and(|actual| same_type(types, actual, target))
        && instruction.result_count == 1
}

fn verify_array_extract(values: &[u64], result: Option<u64>, types: &BinaryTypeTable) -> bool {
    if values.len() != 2 || !is_integer_type(types, values[1]) {
        return false;
    }
    let Some(BinaryType::Array { element, .. }) = type_entry(types, values[0]) else {
        return false;
    };
    result.is_some_and(|actual| same_type(types, actual, *element))
        && is_trivial(types, *element, 0)
}

fn verify_array_insert(values: &[u64], result: Option<u64>, types: &BinaryTypeTable) -> bool {
    if values.len() != 3 || !is_integer_type(types, values[1]) {
        return false;
    }
    let source = values[0];
    let Some(BinaryType::Array { element, .. }) = type_entry(types, source) else {
        return false;
    };
    result.is_some_and(|actual| same_type(types, actual, source))
        && same_type(types, values[2], *element)
        && is_trivial(types, source, 0)
}

fn verify_enum_new(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_ids: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
) -> bool {
    let (Some(target), Some(variant)) = (type_operand(instruction), index_operand(instruction))
    else {
        return false;
    };
    let Some((definition, arguments)) = nominal_definition(types, target, BinaryNominalKind::Enum)
    else {
        return false;
    };
    let Some(payload) = usize::try_from(variant)
        .ok()
        .and_then(|index| definition.variants.get(index))
        .and_then(|variant| {
            variant
                .payload
                .iter()
                .map(|ty| substitute(types, *ty, arguments))
                .collect::<Option<Vec<_>>>()
        })
    else {
        return false;
    };
    values.len() == payload.len()
        && payload
            .iter()
            .zip(values)
            .zip(value_ids)
            .all(|((expected, value), id)| {
                same_type(types, *value, *expected)
                    && (is_trivial(types, *expected, 0) || !borrowed.contains(id))
            })
        && is_loadable(types, target, &mut HashSet::new(), 0)
        && result.is_some_and(|actual| same_type(types, actual, target))
        && instruction.result_count == 1
}

fn verify_enum_tag(values: &[u64], result: Option<u64>, types: &BinaryTypeTable) -> bool {
    if values.len() != 1 {
        return false;
    }
    let source = values[0];
    let Some((definition, _)) = nominal_definition(types, source, BinaryNominalKind::Enum) else {
        return false;
    };
    let Some(expected) = enum_tag_type(types, definition) else {
        return false;
    };
    result.is_some_and(|actual| same_type(types, actual, expected))
}

fn verify_enum_payload(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_ids: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    borrowed: &HashSet<u64>,
    project: bool,
) -> bool {
    let (Some(source), Some(variant)) = (values.first(), index_operand(instruction)) else {
        return false;
    };
    let Some((definition, arguments)) = nominal_definition(types, *source, BinaryNominalKind::Enum)
    else {
        return false;
    };
    let Some(expected) = usize::try_from(variant)
        .ok()
        .and_then(|index| definition.variants.get(index))
        .and_then(|variant| {
            let payload = variant
                .payload
                .iter()
                .map(|ty| substitute(types, *ty, arguments))
                .collect::<Option<Vec<_>>>()?;
            super::aggregate::payload_type(types, &payload)
        })
    else {
        return false;
    };
    values.len() == 1
        && instruction.result_count == 1
        && result.is_some_and(|actual| same_type(types, actual, expected))
        && if project {
            value_ids
                .first()
                .is_some_and(|source| borrowed.contains(source))
        } else {
            value_ids
                .first()
                .is_none_or(|source| !borrowed.contains(source))
        }
}

fn resolved_values(ids: &[u64], value_types: &[Option<u64>]) -> Option<Vec<u64>> {
    ids.iter()
        .map(|value| lookup_value_type(value_types, *value))
        .collect()
}

fn value_ids(instruction: &BinaryInstruction) -> Vec<u64> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .copied()
        .collect()
}

fn lookup_value_type(value_types: &[Option<u64>], value: u64) -> Option<u64> {
    value_types
        .get(usize::try_from(value).ok()?)
        .copied()
        .flatten()
}

fn result_type(value_types: &[Option<u64>], start: usize, offset: usize) -> Option<u64> {
    value_types
        .get(start.checked_add(offset)?)
        .copied()
        .flatten()
}

fn type_operand(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

fn index_operand(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        })
}

fn type_entry(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn same_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    type_entry(types, left) == type_entry(types, right) && type_entry(types, left).is_some()
}

fn is_integer_type(types: &BinaryTypeTable, ty: u64) -> bool {
    type_entry(types, ty).is_some_and(|ty| matches!(ty, BinaryType::Integer(_)))
}
