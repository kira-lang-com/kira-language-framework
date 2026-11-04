use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(in crate::binary_code) fn infer_result_types(
    instruction: &BinaryInstruction,
    types: &TypeSection,
) -> Option<Vec<Option<u64>>> {
    if !(0x500..=0x511).contains(&instruction.opcode) {
        return None;
    }
    let count = usize::from(instruction.result_count);
    let result = infer_result_type(instruction, types);
    Some(vec![result; count])
}

pub(super) fn infer_result_type(
    instruction: &BinaryInstruction,
    types: &TypeSection,
) -> Option<u64> {
    let attribute = attribute_code(instruction);
    match instruction.opcode {
        0x501 | 0x507 => integer_type(types, 1),
        0x505 => query_attribute_type(types, attribute?),
        0x50b => trace_attribute_type(types, attribute?),
        0x50c | 0x50d | 0x50e => types
            .value_types
            .iter()
            .position(|ty| matches!(ty, TvisaValueTypeRecord::HitObject))
            .map(|index| index as u64),
        0x510 => query_attribute_type(types, attribute?),
        _ => None,
    }
}

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    types: &TypeSection,
    value_types: &[Option<u64>],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if !(0x500..=0x511).contains(&instruction.opcode) {
            continue;
        }
        let values = value_operands(instruction);
        let value_type = |index: usize| {
            values
                .get(index)
                .and_then(|value| value_type_index(*value, value_types))
                .and_then(|index| types.value_types.get(index as usize))
        };
        let invalid = match instruction.opcode {
            0x500 => {
                !value_type(0).is_some_and(is_pointer_to(4))
                    || !value_type(1)
                        .is_some_and(|ty| matches!(ty, TvisaValueTypeRecord::AccelerationStructure))
                    || !value_type(2).is_some_and(is_integer)
                    || !value_type(3).is_some_and(is_integer)
                    || !value_type(4).is_some_and(|ty| vector_is(ty, 3, is_f32, types))
                    || !value_type(5).is_some_and(is_f32)
                    || !value_type(6).is_some_and(|ty| vector_is(ty, 3, is_f32, types))
                    || !value_type(7).is_some_and(is_f32)
            }
            0x501 | 0x502 | 0x504 | 0x505 | 0x50d => !value_type(0).is_some_and(is_pointer_to(4)),
            0x503 => {
                !value_type(0).is_some_and(is_pointer_to(4)) || !value_type(1).is_some_and(is_f32)
            }
            0x506 => invalid_trace_operands(&values, value_types, true, types),
            0x507 => {
                !value_type(0).is_some_and(is_f32)
                    || !value_type(1).is_some_and(is_integer)
                    || !value_type(2).is_some_and(is_pointer_to(7))
            }
            0x50a => {
                !value_type(0).is_some_and(is_integer)
                    || !value_type(1).is_some_and(is_pointer_to(6))
            }
            0x50c => invalid_trace_operands(&values, value_types, false, types),
            0x50f => {
                !value_type(0).is_some_and(is_hitobject)
                    || !value_type(1).is_some_and(is_pointer_to(6))
            }
            0x510 | 0x511 => {
                !value_type(0).is_some_and(is_hitobject)
                    || (instruction.opcode == 0x511
                        && values.len() == 2
                        && !value_type(1).is_some_and(is_integer))
            }
            _ => false,
        };
        if invalid {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU ray-tracing opcode 0x{:03X} has invalid operand types",
                    instruction.opcode
                ),
            ));
        }
        if instruction.result_count != expected_results(instruction) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU ray-tracing opcode 0x{:03X} has an invalid result count",
                    instruction.opcode
                ),
            ));
        }
    }
    diagnostics
}

fn invalid_trace_operands(
    values: &[u64],
    value_types: &[Option<u64>],
    payload: bool,
    types: &TypeSection,
) -> bool {
    !value_record(0, values, value_types, types)
        .is_some_and(|ty| matches!(ty, TvisaValueTypeRecord::AccelerationStructure))
        || (1..=5)
            .any(|index| !value_record(index, values, value_types, types).is_some_and(is_integer))
        || !value_record(6, values, value_types, types)
            .is_some_and(|ty| vector_is(ty, 3, is_f32, types))
        || !value_record(7, values, value_types, types).is_some_and(is_f32)
        || !value_record(8, values, value_types, types)
            .is_some_and(|ty| vector_is(ty, 3, is_f32, types))
        || !value_record(9, values, value_types, types).is_some_and(is_f32)
        || (payload && !value_record(10, values, value_types, types).is_some_and(is_pointer_to(6)))
}

fn expected_results(instruction: &BinaryInstruction) -> u8 {
    match instruction.opcode {
        0x501 | 0x507 | 0x50c | 0x50d | 0x50e => 1,
        0x505 | 0x510 => {
            if attribute_code(instruction).is_some_and(|code| code >= 10) {
                12
            } else {
                1
            }
        }
        0x50b => match attribute_code(instruction) {
            Some(14 | 15) => 12,
            Some(16) => 9,
            _ => 1,
        },
        _ => 0,
    }
}

fn query_attribute_type(types: &TypeSection, code: u64) -> Option<u64> {
    match code {
        0 | 2..=5 => integer_type(types, 32),
        1 => float_type(types, 32),
        6 => vector_type(types, 2, is_f32),
        7 => integer_type(types, 1),
        8 | 9 => vector_type(types, 3, is_f32),
        10 | 11 => float_type(types, 32),
        _ => None,
    }
}

fn trace_attribute_type(types: &TypeSection, code: u64) -> Option<u64> {
    match code {
        0 | 1 => vector_type(types, 3, is_i32),
        2..=5 => vector_type(types, 3, is_f32),
        6 | 7 => float_type(types, 32),
        8..=13 => integer_type(types, 32),
        14..=16 => float_type(types, 32),
        _ => None,
    }
}

fn attribute_code(instruction: &BinaryInstruction) -> Option<u64> {
    matches!(instruction.opcode, 0x505 | 0x50b | 0x510).then(|| {
        instruction
            .operands
            .iter()
            .rev()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => Some(*index),
                _ => None,
            })
    })?
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

fn value_type_index(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

fn value_record<'a>(
    index: usize,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &'a TypeSection,
) -> Option<&'a TvisaValueTypeRecord> {
    values
        .get(index)
        .and_then(|value| value_type_index(*value, value_types))
        .and_then(|index| types.value_types.get(index as usize))
}

fn vector_is(
    ty: &TvisaValueTypeRecord,
    lanes: u64,
    element_matches: fn(&TvisaValueTypeRecord) -> bool,
    types: &TypeSection,
) -> bool {
    matches!(
        ty,
        TvisaValueTypeRecord::FixedVector { lanes: actual, element_type }
            if *actual == lanes && types.value_types.get(*element_type as usize).is_some_and(element_matches)
    )
}

fn vector_type(
    types: &TypeSection,
    lanes: u64,
    element_matches: fn(&TvisaValueTypeRecord) -> bool,
) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(|ty| vector_is(ty, lanes, element_matches, types))
        .map(|index| index as u64)
}

fn float_type(types: &TypeSection, bits: u64) -> Option<u64> {
    (bits == 32)
        .then(|| {
            types
                .value_types
                .iter()
                .position(|ty| matches!(ty, TvisaValueTypeRecord::Float { tag: 0x04 }))
                .map(|index| index as u64)
        })
        .flatten()
}

fn integer_type(types: &TypeSection, bits: u64) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(
            |ty| matches!(ty, TvisaValueTypeRecord::Integer { bits: width } if *width == bits),
        )
        .map(|index| index as u64)
}

fn is_pointer_to(address_space: u8) -> impl Fn(&TvisaValueTypeRecord) -> bool {
    move |ty| matches!(ty, TvisaValueTypeRecord::Pointer { address_space: actual } if *actual == address_space)
}

fn is_integer(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Integer { .. })
}

fn is_i32(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Integer { bits: 32 })
}

fn is_f32(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Float { tag: 0x04 })
}

fn is_hitobject(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::HitObject)
}
