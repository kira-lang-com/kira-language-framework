use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shape {
    Scalar,
    Fixed(u64),
    Scalable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NumericKind {
    Integer(u64),
    Float(u64),
}

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        let valid = match instruction.opcode {
            0x020..=0x045 => verify_integer(instruction, value_types, types),
            0x060..=0x092 => verify_float(instruction, value_types, types),
            0x0a0..=0x0ac => verify_conversion(instruction, value_types, types),
            _ => continue,
        };
        if !valid {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "instruction opcode 0x{:03X} has incompatible operand types",
                    instruction.opcode
                ),
            ));
        }
    }
    diagnostics
}

fn verify_integer(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let operands = value_operands(instruction);
    let operand_types = operands
        .iter()
        .map(|value| value_type(*value, value_types))
        .collect::<Option<Vec<_>>>();
    let Some(operand_types) = operand_types else {
        return true;
    };
    let opcode = instruction.opcode - 0x020;
    if opcode == 29 {
        return verify_select(&operand_types, types);
    }
    if opcode == 28 {
        return verify_icmp(instruction, &operand_types, types);
    }
    if opcode == 35 || opcode == 36 {
        return operand_types.len() == 2
            && operand_types[0] == operand_types[1]
            && matches!(
                scalar_numeric(types, operand_types[0]),
                Some((NumericKind::Integer(32 | 64), Shape::Scalar))
            );
    }
    operand_types.len() == expected_integer_arity(opcode)
        && operand_types
            .first()
            .is_some_and(|first| is_integer_type(types, *first))
        && operand_types.iter().all(|ty| ty == &operand_types[0])
}

fn expected_integer_arity(opcode: u64) -> usize {
    match opcode {
        14 | 15 | 16 | 30..=34 | 37 => 1,
        22 | 23 | 29 => 3,
        _ => 2,
    }
}

fn verify_icmp(instruction: &BinaryInstruction, operands: &[u64], types: &TypeSection) -> bool {
    if operands.len() != 2 || operands[0] != operands[1] {
        return false;
    }
    let Some(record) = types.value_types.get(operands[0] as usize) else {
        return false;
    };
    let (lane_type, shape) = match record {
        TvisaValueTypeRecord::FixedVector {
            element_type,
            lanes,
        } => (*element_type, Shape::Fixed(*lanes)),
        TvisaValueTypeRecord::ScalableVector { element_type } => (*element_type, Shape::Scalable),
        _ => (operands[0], Shape::Scalar),
    };
    let Some(lane_record) = types.value_types.get(lane_type as usize) else {
        return false;
    };
    let predicate = (instruction.modifiers >> 48) & 0xf;
    match lane_record {
        TvisaValueTypeRecord::Integer { .. } => predicate <= 9,
        TvisaValueTypeRecord::Pointer { .. } => predicate <= 9,
        TvisaValueTypeRecord::Code if shape == Shape::Scalar => matches!(predicate, 0 | 1),
        _ => false,
    }
}

fn verify_select(operands: &[u64], types: &TypeSection) -> bool {
    if operands.len() != 3 || operands[1] != operands[2] {
        return false;
    }
    let Some((_, shape)) = numeric_or_pointer_shape(types, operands[1]) else {
        return false;
    };
    match shape {
        Shape::Scalar => is_integer_width(types, operands[0], 1),
        Shape::Fixed(lanes) => is_mask_type(types, operands[0], Some(lanes)),
        Shape::Scalable => is_mask_type(types, operands[0], None),
    }
}

fn verify_float(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let operands = value_operands(instruction);
    let operand_types = operands
        .iter()
        .map(|value| value_type(*value, value_types))
        .collect::<Option<Vec<_>>>();
    let Some(operand_types) = operand_types else {
        return true;
    };
    let opcode = instruction.opcode - 0x060;
    if opcode == 24 {
        return operand_types.is_empty();
    }
    if opcode == 25 {
        return operand_types.is_empty() && instruction.result_count == 0;
    }
    if opcode == 20 {
        return operand_types.len() == 2
            && operand_types[0] == operand_types[1]
            && is_float_type(types, operand_types[0])
            && ((instruction.modifiers >> 48) & 0xf) <= 15;
    }
    if opcode == 23 {
        if operand_types.len() != 2 {
            return false;
        }
        let Some((_, shape)) = scalar_numeric(types, operand_types[0]) else {
            return false;
        };
        return is_float_type(types, operand_types[0])
            && integer_shape_matches(types, operand_types[1], 32, shape);
    }
    let arity = match opcode {
        5 | 6 | 7 | 15..=19 | 21 | 22 | 26..=31 | 33..=46 | 48 | 50 => 1,
        8 | 9 => 3,
        _ => 2,
    };
    operand_types.len() == arity
        && operand_types
            .first()
            .is_some_and(|first| is_float_type(types, *first))
        && operand_types.iter().all(|ty| ty == &operand_types[0])
}

fn verify_conversion(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let Some(source) = value_operands(instruction)
        .first()
        .and_then(|value| value_type(*value, value_types))
    else {
        return true;
    };
    let Some(target) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
    else {
        return false;
    };
    match instruction.opcode - 0x0a0 {
        9 => {
            matches!(
                types.value_types.get(source as usize),
                Some(TvisaValueTypeRecord::Pointer { address_space: 0 })
            ) && is_integer_width(types, target, 64)
        }
        10 => {
            is_integer_width(types, source, 64)
                && matches!(
                    types.value_types.get(target as usize),
                    Some(TvisaValueTypeRecord::Pointer {
                        address_space: 0..=2
                    })
                )
        }
        11 => bitcast_compatible(types, source, target),
        0..=8 | 12 => {
            let Some((source_kind, source_shape)) = scalar_numeric(types, source) else {
                return false;
            };
            let Some((target_kind, target_shape)) = scalar_numeric(types, target) else {
                return false;
            };
            if source_shape != target_shape {
                return false;
            }
            verify_numeric_conversion(instruction.opcode - 0x0a0, source_kind, target_kind)
        }
        _ => false,
    }
}

fn verify_numeric_conversion(
    opcode: u64,
    source_kind: NumericKind,
    target_kind: NumericKind,
) -> bool {
    match opcode {
        0 => {
            matches!((source_kind, target_kind), (NumericKind::Integer(from), NumericKind::Integer(to)) if from > to)
        }
        1 | 2 => {
            matches!((source_kind, target_kind), (NumericKind::Integer(from), NumericKind::Integer(to)) if from < to)
        }
        3 => {
            matches!((source_kind, target_kind), (NumericKind::Float(from), NumericKind::Float(to)) if from > to)
        }
        4 => {
            matches!((source_kind, target_kind), (NumericKind::Float(from), NumericKind::Float(to)) if from < to)
        }
        5 | 6 => {
            matches!(source_kind, NumericKind::Integer(_))
                && matches!(target_kind, NumericKind::Float(_))
        }
        7 | 8 => {
            matches!(source_kind, NumericKind::Float(_))
                && matches!(target_kind, NumericKind::Integer(_))
        }
        12 => matches!(
            (source_kind, target_kind),
            (NumericKind::Float(_), NumericKind::Float(_))
        ),
        _ => false,
    }
}

fn scalar_numeric(types: &TypeSection, type_index: u64) -> Option<(NumericKind, Shape)> {
    let record = types.value_types.get(type_index as usize)?;
    let (lane_index, shape) = match record {
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } => (*element_type, Shape::Fixed(*lanes)),
        TvisaValueTypeRecord::ScalableVector { element_type } => (*element_type, Shape::Scalable),
        _ => (type_index, Shape::Scalar),
    };
    let kind = match types.value_types.get(lane_index as usize)? {
        TvisaValueTypeRecord::Integer { bits } => NumericKind::Integer(*bits),
        TvisaValueTypeRecord::Float { tag } => NumericKind::Float(float_width(*tag)?),
        _ => return None,
    };
    Some((kind, shape))
}

fn numeric_or_pointer_shape(types: &TypeSection, type_index: u64) -> Option<(u64, Shape)> {
    let record = types.value_types.get(type_index as usize)?;
    match record {
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } => Some((*element_type, Shape::Fixed(*lanes))),
        TvisaValueTypeRecord::ScalableVector { element_type } => {
            Some((*element_type, Shape::Scalable))
        }
        _ => Some((type_index, Shape::Scalar)),
    }
}

fn is_integer_type(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        scalar_numeric(types, type_index),
        Some((NumericKind::Integer(_), _))
    )
}

fn is_float_type(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        scalar_numeric(types, type_index),
        Some((NumericKind::Float(_), _))
    )
}

fn is_integer_width(types: &TypeSection, type_index: u64, width: u64) -> bool {
    matches!(types.value_types.get(type_index as usize), Some(TvisaValueTypeRecord::Integer { bits }) if *bits == width)
}

fn integer_shape_matches(types: &TypeSection, type_index: u64, width: u64, shape: Shape) -> bool {
    match (types.value_types.get(type_index as usize), shape) {
        (Some(TvisaValueTypeRecord::Integer { bits }), Shape::Scalar) => *bits == width,
        (
            Some(TvisaValueTypeRecord::FixedVector {
                lanes,
                element_type,
            }),
            Shape::Fixed(expected_lanes),
        ) => *lanes == expected_lanes && is_integer_width(types, *element_type, width),
        (Some(TvisaValueTypeRecord::ScalableVector { element_type }), Shape::Scalable) => {
            is_integer_width(types, *element_type, width)
        }
        _ => false,
    }
}

fn is_mask_type(types: &TypeSection, type_index: u64, lanes: Option<u64>) -> bool {
    match (types.value_types.get(type_index as usize), lanes) {
        (Some(TvisaValueTypeRecord::Integer { bits: 1 }), None) => true,
        (
            Some(TvisaValueTypeRecord::FixedVector {
                lanes: actual_lanes,
                element_type,
            }),
            Some(expected_lanes),
        ) => *actual_lanes == expected_lanes && is_integer_width(types, *element_type, 1),
        (Some(TvisaValueTypeRecord::ScalableVector { element_type }), None) => {
            is_integer_width(types, *element_type, 1)
        }
        _ => false,
    }
}

fn bitcast_compatible(types: &TypeSection, source: u64, target: u64) -> bool {
    if matches!(
        types.value_types.get(source as usize),
        Some(TvisaValueTypeRecord::Code)
    ) || matches!(
        types.value_types.get(target as usize),
        Some(TvisaValueTypeRecord::Code)
    ) {
        return false;
    }
    storage_size(types, source).is_some()
        && storage_size(types, source) == storage_size(types, target)
}

pub(super) fn storage_size(types: &TypeSection, type_index: u64) -> Option<u128> {
    match types.value_types.get(type_index as usize)? {
        TvisaValueTypeRecord::Integer { bits } => Some(scalar_storage_size(*bits)),
        TvisaValueTypeRecord::Float { tag } => Some(scalar_storage_size(float_width(*tag)?)),
        TvisaValueTypeRecord::Pointer { .. }
        | TvisaValueTypeRecord::Sampler
        | TvisaValueTypeRecord::AccelerationStructure
        | TvisaValueTypeRecord::VisibleFunctionTable { .. }
        | TvisaValueTypeRecord::Texture { .. }
        | TvisaValueTypeRecord::Tensor { .. } => Some(64),
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } => {
            let lane_width = scalar_bit_width(types, *element_type)?;
            let lanes = u128::from(*lanes);
            if lane_width == 1 {
                Some(lanes)
            } else if matches!(lane_width, 4 | 6) {
                Some((lanes * u128::from(lane_width)).div_ceil(8))
            } else {
                let lane_size = scalar_storage_size(lane_width);
                let alignment = (lane_size * lanes.checked_next_power_of_two()?).min(16);
                let size = lane_size * lanes;
                Some((size + alignment - 1) / alignment * alignment)
            }
        }
        TvisaValueTypeRecord::ScalableVector { .. }
        | TvisaValueTypeRecord::Code
        | TvisaValueTypeRecord::RayQuery
        | TvisaValueTypeRecord::HitObject
        | TvisaValueTypeRecord::CooperativeMatrix { .. } => None,
    }
}

fn scalar_bit_width(types: &TypeSection, type_index: u64) -> Option<u64> {
    match types.value_types.get(type_index as usize)? {
        TvisaValueTypeRecord::Integer { bits } => Some(*bits),
        TvisaValueTypeRecord::Float { tag } => float_width(*tag),
        TvisaValueTypeRecord::Pointer { .. } => Some(64),
        _ => None,
    }
}

fn scalar_storage_size(bits: u64) -> u128 {
    u128::from(bits.div_ceil(8).next_power_of_two().min(16))
}

fn value_operands(instruction: &BinaryInstruction) -> Vec<u64> {
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

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn float_width(tag: u8) -> Option<u64> {
    Some(match tag {
        0x02 | 0x03 => 16,
        0x04 => 32,
        0x05 => 64,
        0x06 => 128,
        0x07 | 0x08 | 0x0c => 8,
        0x09 | 0x0a => 6,
        0x0b => 4,
        _ => return None,
    })
}
