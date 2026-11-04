use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

pub(super) mod advanced;
mod geometry;
pub(super) mod ray;
pub(super) mod texture;

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn infer_result_type(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<u64> {
    if let Some(result) = geometry::infer_result_type(instruction, types) {
        return Some(result);
    }
    if let Some(result) = ray::infer_result_type(instruction, types) {
        return Some(result);
    }
    if (0x480..=0x48d).contains(&instruction.opcode) {
        return texture::infer_result_type(instruction, value_types, types);
    }
    if (0x560..=0x56b).contains(&instruction.opcode) {
        return advanced::infer_result_type(instruction, value_types, types);
    }
    match instruction.opcode {
        0x420..=0x424 | 0x44b..=0x452 => value_type(first_value(instruction)?, value_types),
        0x425 | 0x426 | 0x440..=0x443 | 0x447 => integer_type(types, 1),
        0x446 => vector_type(types, 4, 32),
        0x448..=0x44a => integer_type(types, 32),
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
        verify_instruction(instruction, types, value_types, &mut diagnostics);
        if instruction.result_count != 0
            && infer_result_type(instruction, value_types, types).is_none()
        {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU collective opcode 0x{:03X} has no inferable result type",
                    instruction.opcode
                ),
            ));
        }
    }
    diagnostics.extend(geometry::verify_function(body, types, value_types));
    diagnostics.extend(ray::verify_function(body, types, value_types));
    diagnostics.extend(texture::verify_function(body, types, value_types));
    diagnostics.extend(advanced::verify_function(body, types, value_types));
    diagnostics
}

fn verify_instruction(
    instruction: &BinaryInstruction,
    types: &TypeSection,
    value_types: &[Option<u64>],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let values = instruction
        .operands
        .iter()
        .filter_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.as_slice()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let operand_type = |group: usize, item: usize| {
        values
            .get(group)
            .and_then(|group| group.get(item))
            .and_then(|value| value_type(*value, value_types))
            .and_then(|index| types.value_types.get(index as usize))
    };
    let invalid = match instruction.opcode {
        0x420..=0x422 => !operand_type(0, 0).is_some_and(|ty| is_float_value(ty, types)),
        0x423 | 0x444 => !operand_type(0, 1).is_some_and(is_i32),
        0x425 | 0x426 | 0x441 | 0x442 | 0x446 => !matches!(
            operand_type(0, 0),
            Some(TvisaValueTypeRecord::Integer { bits: 1 })
        ),
        0x443 => !operand_type(0, 0).is_some_and(is_scalar),
        0x447..=0x44a => !operand_type(0, 0).is_some_and(|ty| is_ballot(ty, types)),
        0x44b..=0x44f => !operand_type(0, 1).is_some_and(is_i32),
        0x450..=0x452 => !valid_reduction_type(instruction, operand_type(0, 0), types),
        _ => false,
    };
    if invalid {
        diagnostics.push(Diagnostic::error(
            VFY3,
            format!(
                "GPU collective opcode 0x{:03X} has invalid operand types",
                instruction.opcode
            ),
        ));
    }

    if matches!(instruction.opcode, 0x450..=0x452)
        && instruction
            .operands
            .iter()
            .filter(|operand| matches!(operand, BinaryOperand::Index(_)))
            .count()
            < 2
    {
        diagnostics.push(Diagnostic::error(
            VFY3,
            "subgroup reduction lacks its cluster field",
        ));
    }
}

fn valid_reduction_type(
    instruction: &BinaryInstruction,
    ty: Option<&TvisaValueTypeRecord>,
    types: &TypeSection,
) -> bool {
    let selector = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let Some(selector) = selector else {
        return false;
    };
    let Some(ty) = ty else {
        return false;
    };
    let ty = match ty {
        TvisaValueTypeRecord::FixedVector { element_type, .. } => {
            types.value_types.get(*element_type as usize)
        }
        scalar => Some(scalar),
    };
    let Some(ty) = ty else {
        return false;
    };
    match (selector, ty) {
        (0..=1 | 3..=4 | 6..=7 | 9..=10 | 12..=14, ty) if is_integer(ty) => true,
        (2 | 5 | 8 | 11, TvisaValueTypeRecord::Float { tag: 0x02..=0x05 }) => true,
        _ => false,
    }
}

fn is_scalar(ty: &TvisaValueTypeRecord) -> bool {
    matches!(
        ty,
        TvisaValueTypeRecord::Integer { .. } | TvisaValueTypeRecord::Float { .. }
    )
}

fn is_integer(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Integer { .. })
}

fn is_i32(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Integer { bits: 32 })
}

fn is_float_value(ty: &TvisaValueTypeRecord, types: &TypeSection) -> bool {
    match ty {
        TvisaValueTypeRecord::Float { tag: 0x02..=0x05 } => true,
        TvisaValueTypeRecord::FixedVector { element_type, .. } => matches!(
            types.value_types.get(*element_type as usize),
            Some(TvisaValueTypeRecord::Float { tag: 0x02..=0x05 })
        ),
        _ => false,
    }
}

fn is_ballot(ty: &TvisaValueTypeRecord, types: &TypeSection) -> bool {
    matches!(
        ty,
        TvisaValueTypeRecord::FixedVector { lanes: 4, element_type }
            if matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Integer { bits: 32 }))
    )
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

fn vector_type(types: &TypeSection, lanes: u64, element_bits: u64) -> Option<u64> {
    types.value_types.iter().position(|ty| {
        matches!(
            ty,
            TvisaValueTypeRecord::FixedVector { lanes: actual, element_type }
                if *actual == lanes && matches!(types.value_types.get(*element_type as usize), Some(TvisaValueTypeRecord::Integer { bits }) if *bits == element_bits)
        )
    }).map(|index| index as u64)
}

fn first_value(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => values.first().copied(),
            BinaryOperand::Value(value) => Some(*value),
            _ => None,
        })
}

fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}
