use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
        if !matches!(instruction.opcode, 0x146..=0x14d | 0x270..=0x272) {
            continue;
        }
        if !verify_instruction(instruction, value_types, types) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "memory instruction opcode 0x{:03X} has incompatible operands or selector",
                    instruction.opcode
                ),
            ));
        }
    }
    diagnostics
}

fn verify_instruction(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let selector = (instruction.modifiers >> 48) & 0xf;
    let selector_valid = match instruction.opcode {
        0x14d => selector <= 1,
        0x272 => selector <= 2,
        _ => selector == 0,
    };
    if !selector_valid || !valid_modifiers(instruction) {
        return false;
    }
    let values = value_operands(instruction);
    let Some(operands) = values
        .iter()
        .map(|value| value_type(*value, value_types))
        .collect::<Option<Vec<_>>>()
    else {
        return true;
    };
    match instruction.opcode {
        0x146 => {
            let Some(target) = type_operand(instruction) else {
                return false;
            };
            operands.len() == 1 && is_pointer(types, operands[0]) && is_memory_type(types, target)
        }
        0x147 => {
            operands.len() == 2
                && is_memory_type(types, operands[0])
                && is_pointer(types, operands[1])
        }
        0x148 => {
            operands.len() == 2
                && is_pointer(types, operands[0])
                && is_integer_value(types, operands[1])
        }
        0x149 => {
            operands.len() == 2 && is_pointer(types, operands[0]) && is_pointer(types, operands[1])
        }
        0x14a | 0x14b => {
            operands.len() == 3
                && is_pointer(types, operands[0])
                && is_pointer(types, operands[1])
                && is_integer_value(types, operands[2])
        }
        0x14c => {
            operands.len() == 3
                && is_pointer(types, operands[0])
                && is_integer_value(types, operands[1])
                && is_integer_value(types, operands[2])
        }
        0x14d => operands.len() == 1 && is_pointer(types, operands[0]),
        0x270 => {
            let Some(target) = type_operand(instruction) else {
                return false;
            };
            operands.len() == 1 && is_pointer(types, operands[0]) && is_mmio_type(types, target)
        }
        0x271 => {
            operands.len() == 2
                && is_mmio_type(types, operands[0])
                && is_pointer(types, operands[1])
        }
        0x272 => operands.is_empty(),
        _ => false,
    }
}

fn valid_modifiers(instruction: &BinaryInstruction) -> bool {
    let modifiers = instruction.modifiers;
    let order = (modifiers >> 20) & 0x7;
    match instruction.opcode {
        0x146 | 0x147 => modifiers & !((1 << 18) | (1 << 19) | (0xf << 48)) == 0,
        0x148 => modifiers & !((1 << 5) | (0xf << 48)) == 0,
        0x149 => modifiers == 0,
        0x14a => modifiers & !((1 << 18) | (1 << 31) | (0xf << 48)) == 0,
        0x14b | 0x14c => modifiers & !((1 << 18) | (0xf << 48)) == 0,
        0x14d => modifiers & !(0x7 | (0xf << 48)) == 0,
        0x270 => matches!(order, 0..=2) && modifiers & !(0x7 << 20) == 0,
        0x271 => matches!(order, 0 | 1 | 3) && modifiers & !(0x7 << 20) == 0,
        0x272 => modifiers & !(0xf << 48) == 0,
        _ => false,
    }
}

fn is_mmio_type(types: &TypeSection, type_index: u64) -> bool {
    is_memory_type(types, type_index)
        && matches!(
            super::instruction_types::storage_size(types, type_index),
            Some(1 | 2 | 4 | 8)
        )
}

fn is_memory_type(types: &TypeSection, type_index: u64) -> bool {
    !matches!(
        types.value_types.get(type_index as usize),
        Some(
            TvisaValueTypeRecord::Integer { bits: 1 }
                | TvisaValueTypeRecord::Code
                | TvisaValueTypeRecord::ScalableVector { .. }
                | TvisaValueTypeRecord::RayQuery
                | TvisaValueTypeRecord::HitObject
                | TvisaValueTypeRecord::CooperativeMatrix { .. }
        ) | None
    )
}

fn is_pointer(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        types.value_types.get(type_index as usize),
        Some(TvisaValueTypeRecord::Pointer { .. })
    )
}

fn is_integer_value(types: &TypeSection, type_index: u64) -> bool {
    match types.value_types.get(type_index as usize) {
        Some(TvisaValueTypeRecord::Integer { .. }) => true,
        Some(TvisaValueTypeRecord::FixedVector { element_type, .. }) => matches!(
            types.value_types.get(*element_type as usize),
            Some(TvisaValueTypeRecord::Integer { .. })
        ),
        _ => false,
    }
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

fn type_operand(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(type_index) => Some(*type_index),
            _ => None,
        })
}
