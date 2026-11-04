use tir_format::{
    BinaryInstruction, BinaryLayout, BinaryNominalKind, BinaryOperand, BinaryType, BinaryTypeTable,
};

use super::{type_at, type_index, value_type_at};

pub(super) fn infer_destructure_results(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    count: usize,
) -> Vec<Option<u64>> {
    let Some(source) =
        value_type_at(instruction, 0, value_types).and_then(|index| type_at(types, index))
    else {
        return vec![None; count];
    };
    let fields = (|| -> Option<Vec<u64>> {
        match (instruction.opcode, source) {
            (0x106, BinaryType::Tuple(fields)) => Some(fields.clone()),
            (
                0x103,
                BinaryType::Nominal {
                    declaration,
                    arguments,
                },
            ) => {
                let declaration_record = types
                    .declarations
                    .get(usize::try_from(*declaration).ok()?)?;
                if declaration_record.kind != BinaryNominalKind::Struct
                    || declaration_record.generic_parameters.len() != arguments.len()
                {
                    None
                } else {
                    let definition = types
                        .definitions
                        .iter()
                        .find(|definition| definition.declaration == *declaration)?;
                    definition
                        .fields
                        .iter()
                        .map(|field| super::address::substitute_type(types, field.ty, arguments))
                        .collect::<Option<Vec<_>>>()
                }
            }
            _ => None,
        }
    })();
    let mut results = fields
        .unwrap_or_default()
        .into_iter()
        .take(count)
        .map(Some)
        .collect::<Vec<_>>();
    results.resize(count, None);
    results
}

pub(super) fn infer_aggregate_result(
    instruction: &BinaryInstruction,
    first: Option<u64>,
    types: &BinaryTypeTable,
) -> Option<u64> {
    let source = type_at(types, first?)?;
    match source {
        BinaryType::Tuple(elements) if instruction.opcode == 0x105 => elements
            .get(usize::try_from(index_operand(instruction)?).ok()?)
            .copied(),
        BinaryType::Array { element, .. } if instruction.opcode == 0x108 => Some(*element),
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            let declaration_record = types
                .declarations
                .get(usize::try_from(*declaration).ok()?)?;
            if declaration_record.generic_parameters.len() != arguments.len() {
                return None;
            }
            let definition = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)?;
            match instruction.opcode {
                0x101 if declaration_record.kind == BinaryNominalKind::Struct => definition
                    .fields
                    .get(usize::try_from(index_operand(instruction)?).ok()?)
                    .and_then(|field| super::address::substitute_type(types, field.ty, arguments)),
                0x10b if declaration_record.kind == BinaryNominalKind::Enum => {
                    enum_tag_type(types, definition)
                }
                0x10c | 0x10d if declaration_record.kind == BinaryNominalKind::Enum => {
                    let payload = definition
                        .variants
                        .get(usize::try_from(index_operand(instruction)?).ok()?)?
                        .payload
                        .iter()
                        .map(|ty| super::address::substitute_type(types, *ty, arguments))
                        .collect::<Option<Vec<_>>>()?;
                    payload_type(types, &payload)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn enum_tag_type(
    types: &BinaryTypeTable,
    definition: &tir_format::BinaryNominalDefinition,
) -> Option<u64> {
    let width = match definition.layout {
        BinaryLayout::C => 32,
        BinaryLayout::Ridl | BinaryLayout::Packed(_) => match definition.variants.len() {
            0..=256 => 8,
            257..=65_536 => 16,
            _ => 32,
        },
        BinaryLayout::Explicit { .. } => return None,
    };
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Integer(bits) if *bits == width),
    )
}

pub(super) fn infer_tuple_new(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<u64> {
    let values = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values),
            _ => None,
        })?;
    let elements = values
        .iter()
        .map(|value| {
            usize::try_from(*value)
                .ok()
                .and_then(|index| value_types.get(index))
                .copied()
                .flatten()
        })
        .collect::<Option<Vec<_>>>()?;
    type_index(
        types,
        |ty| matches!(ty, BinaryType::Tuple(actual) if actual == &elements),
    )
}

pub(super) fn payload_type(types: &BinaryTypeTable, payload: &[u64]) -> Option<u64> {
    match payload {
        [] => type_index(types, |ty| matches!(ty, BinaryType::Unit)),
        [only] => Some(*only),
        _ => type_index(
            types,
            |ty| matches!(ty, BinaryType::Tuple(actual) if actual == payload),
        ),
    }
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
