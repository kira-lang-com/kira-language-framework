use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn infer_result_type(
    instruction: &BinaryInstruction,
    types: &TypeSection,
) -> Option<u64> {
    match instruction.opcode {
        0x5a0 => pointer_type(types, 8),
        0x5c0 | 0x5c2 | 0x5c3 => pointer_type(types, 9),
        0x5c1 => integer_type(types, 32),
        0x5c5 => integer_type(types, 1),
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
        let operands = instruction
            .operands
            .iter()
            .filter_map(|operand| match operand {
                BinaryOperand::Values(values) => Some(values.as_slice()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let operand_type = |group: usize, item: usize| {
            operands
                .get(group)
                .and_then(|group| group.get(item))
                .and_then(|value| value_type(*value, value_types))
                .and_then(|index| types.value_types.get(index as usize))
        };
        let invalid = match instruction.opcode {
            0x4c0 | 0x4c1 => operands
                .iter()
                .flat_map(|values| values.iter())
                .any(|value| {
                    !value_type(*value, value_types).is_some_and(|index| is_integer(types, index))
                }),
            0x4c2..=0x4c4 => {
                !operand_type(0, 0).is_some_and(is_integer_type)
                    || (instruction.opcode == 0x4c4
                        && !operand_type(0, 1).is_some_and(|ty| is_index_value(ty, types)))
            }
            0x5a0 => !operand_type(0, 0).is_some_and(|ty| is_vector_i32(ty, 2, types)),
            0x5c0 => !operand_type(0, 0).is_some_and(is_integer_type),
            0x5c2 => !operand_type(0, 1).is_some_and(is_integer_type),
            0x5c3 => {
                !operand_type(0, 0).is_some_and(|ty| is_pointer(ty, 9))
                    || !operand_type(0, 1).is_some_and(is_integer_type)
            }
            0x5c4 => !operand_type(0, 0).is_some_and(|ty| is_pointer(ty, 9)),
            _ => false,
        };
        if invalid {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU geometry opcode 0x{:03X} has invalid operand types",
                    instruction.opcode
                ),
            ));
        }
        if instruction.result_count != 0 && infer_result_type(instruction, types).is_none() {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU geometry opcode 0x{:03X} has no inferable result type",
                    instruction.opcode
                ),
            ));
        }
    }
    diagnostics
}

fn is_integer(types: &TypeSection, index: u64) -> bool {
    types
        .value_types
        .get(index as usize)
        .is_some_and(is_integer_type)
}

fn is_integer_type(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Integer { .. })
}

fn is_index_value(ty: &TvisaValueTypeRecord, types: &TypeSection) -> bool {
    match ty {
        TvisaValueTypeRecord::Integer { bits: 32 } => true,
        TvisaValueTypeRecord::FixedVector {
            lanes: 2 | 3,
            element_type,
        } => matches!(
            types.value_types.get(*element_type as usize),
            Some(TvisaValueTypeRecord::Integer { bits: 32 })
        ),
        _ => false,
    }
}

fn is_vector_i32(ty: &TvisaValueTypeRecord, lanes: u64, types: &TypeSection) -> bool {
    matches!(
        ty,
        TvisaValueTypeRecord::FixedVector { lanes: actual, element_type }
            if *actual == lanes && matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Integer { bits: 32 }))
    )
}

fn is_pointer(ty: &TvisaValueTypeRecord, address_space: u8) -> bool {
    matches!(ty, TvisaValueTypeRecord::Pointer { address_space: actual } if *actual == address_space)
}

fn pointer_type(types: &TypeSection, address_space: u8) -> Option<u64> {
    types
        .value_types
        .iter()
        .position(|ty| is_pointer(ty, address_space))
        .map(|index| index as u64)
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

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}
