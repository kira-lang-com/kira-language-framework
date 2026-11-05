use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &TypeSection,
    gpu_target: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| (0x200..=0x204).contains(&instruction.opcode))
    {
        if !verify_instruction(instruction, value_types, types, gpu_target) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "atomic instruction opcode 0x{:03X} has incompatible operands or selector",
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
    gpu_target: bool,
) -> bool {
    let selector = (instruction.modifiers >> 48) & 0xf;
    let order = (instruction.modifiers >> 20) & 0x7;
    let failure_order = (instruction.modifiers >> 23) & 0x7;
    let scope = (instruction.modifiers >> 26) & 0x7;
    let allowed_modifiers = (0x7 << 20) | (0x7 << 23) | (0x7 << 26) | (1 << 29) | (0xf << 48);
    if (instruction.opcode == 0x202 && selector > 18)
        || (instruction.opcode != 0x202 && selector != 0)
        || (instruction.opcode != 0x203 && instruction.modifiers & (1 << 29) != 0)
        || instruction.modifiers & !allowed_modifiers != 0
        || !valid_ordering(instruction.opcode, order, failure_order)
        || !valid_scope(scope, gpu_target)
    {
        return false;
    }
    let values = value_operands(instruction);
    let Some(operand_types) = values
        .iter()
        .map(|value| value_type(*value, value_types))
        .collect::<Option<Vec<_>>>()
    else {
        return true;
    };

    match instruction.opcode {
        0x200 => {
            let Some(result_type) = type_operand(instruction) else {
                return false;
            };
            operand_types.len() == 1
                && is_atomic_pointer(types, operand_types[0], gpu_target)
                && is_atomic_value(types, result_type)
        }
        0x201 => {
            operand_types.len() == 2
                && is_atomic_value(types, operand_types[0])
                && is_atomic_pointer(types, operand_types[1], gpu_target)
        }
        0x202 => {
            operand_types.len() == 2
                && is_atomic_pointer(types, operand_types[0], gpu_target)
                && is_atomic_value(types, operand_types[1])
                && valid_rmw_type(types, selector, operand_types[1])
        }
        0x203 => {
            operand_types.len() == 3
                && is_atomic_pointer(types, operand_types[0], gpu_target)
                && operand_types[1] == operand_types[2]
                && is_atomic_value(types, operand_types[1])
        }
        0x204 => operand_types.is_empty(),
        _ => false,
    }
}

fn valid_ordering(opcode: u64, success: u64, failure: u64) -> bool {
    let load_order = matches!(success, 0 | 1 | 2 | 5);
    let store_order = matches!(success, 0 | 1 | 3 | 5);
    let cmpxchg_failure = match failure {
        0 | 1 => true,
        2 => matches!(success, 0 | 2 | 4 | 5),
        5 => matches!(success, 0 | 5),
        _ => false,
    };
    match opcode {
        0x200 => load_order && failure == 0,
        0x201 => store_order && failure == 0,
        0x202 | 0x204 => success <= 5 && failure == 0,
        0x203 => success <= 5 && cmpxchg_failure,
        _ => false,
    }
}

fn valid_scope(scope: u64, gpu_target: bool) -> bool {
    match scope {
        0 | 1 | 5 => true,
        2..=4 => gpu_target,
        _ => false,
    }
}

fn valid_rmw_type(types: &TypeSection, operation: u64, value_type: u64) -> bool {
    match operation {
        0 => true,
        1..=10 | 15..=18 => is_integer_value(types, value_type),
        11..=14 => is_float_value(types, value_type),
        _ => false,
    }
}

fn is_atomic_value(types: &TypeSection, type_index: u64) -> bool {
    if !is_memory_type(types, type_index) {
        return false;
    }
    matches!(
        super::instruction_types::storage_size(types, type_index),
        Some(1 | 2 | 4 | 8 | 16)
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

fn is_atomic_pointer(types: &TypeSection, type_index: u64, gpu_target: bool) -> bool {
    match types.value_types.get(type_index as usize) {
        Some(TvisaValueTypeRecord::Pointer { address_space }) => {
            !gpu_target || matches!(address_space, 1 | 3)
        }
        _ => false,
    }
}

fn is_integer_value(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        types.value_types.get(type_index as usize),
        Some(TvisaValueTypeRecord::Integer { .. })
    )
}

fn is_float_value(types: &TypeSection, type_index: u64) -> bool {
    match types.value_types.get(type_index as usize) {
        Some(TvisaValueTypeRecord::Float { tag }) => !is_narrow_float(*tag),
        _ => false,
    }
}

fn is_narrow_float(tag: u8) -> bool {
    matches!(tag, 0x07..=0x0c)
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
