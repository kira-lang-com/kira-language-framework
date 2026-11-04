use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const SELECTOR_MASK: u64 = 0xf << 48;
const INBOUNDS: u64 = 1 << 5;
const VOLATILE: u64 = 1 << 18;
const NONTEMPORAL: u64 = 1 << 19;
const NOOVERLAP: u64 = 1 << 31;

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut value_cursor = 0usize;
    for block in &body.blocks {
        value_cursor = value_cursor.saturating_add(block.parameters.len());
        let Some(instructions) = &block.instructions else {
            value_cursor = value_cursor.saturating_add(block.instruction_count as usize);
            continue;
        };
        for instruction in instructions {
            if (0x140..=0x14d).contains(&instruction.opcode)
                && !verify_instruction(instruction, value_types, value_cursor, types)
            {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function @{function_name} CODE offset {}: memory opcode 0x{:03X} has incompatible operands, result type, or ownership mode",
                        instruction.offset, instruction.opcode
                    ),
                ));
            }
            value_cursor = value_cursor.saturating_add(instruction.result_count as usize);
        }
    }
    diagnostics
}

fn verify_instruction(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result_start: usize,
    types: &BinaryTypeTable,
) -> bool {
    let relative = instruction.opcode - 0x140;
    let selector = (instruction.modifiers & SELECTOR_MASK) >> 48;
    let allowed = match relative {
        0 | 2 | 3 | 4 | 13 => SELECTOR_MASK,
        6 | 7 => VOLATILE | NONTEMPORAL,
        8 => INBOUNDS,
        10 => VOLATILE | NOOVERLAP,
        11 | 12 => VOLATILE,
        _ => 0,
    };
    if instruction.modifiers & !allowed != 0 {
        return false;
    }
    let values = value_ids(instruction);
    let result = result_type(value_types, result_start);
    match relative {
        0 => verify_load(instruction, &values, value_types, result, types, selector),
        1 => verify_address_load(instruction, &values, value_types, result, types),
        2 => verify_store(instruction, &values, value_types, types, selector),
        3 => verify_address_copy(instruction, &values, value_types, types, selector),
        4 => verify_address_move(instruction, &values, value_types, types, selector),
        5 => {
            values.len() == 1
                && instruction.result_count == 0
                && is_address(types, value_type(value_types, values[0]))
        }
        6 => verify_raw_load(instruction, &values, value_types, result, types),
        7 => verify_raw_store(instruction, &values, value_types, types),
        8 => {
            values.len() == 2
                && instruction.result_count == 1
                && value_type(value_types, values[0]).is_some_and(|ty| is_pointer(types, ty))
                && value_type(value_types, values[1])
                    .is_some_and(|ty| is_integer_width(types, ty, 64))
                && result.is_some_and(|actual| {
                    value_type(value_types, values[0])
                        .is_some_and(|expected| same_type(types, actual, expected))
                })
        }
        9 => {
            values.len() == 2
                && instruction.result_count == 1
                && value_type(value_types, values[0]).is_some_and(|left| is_pointer(types, left))
                && value_type(value_types, values[1]).is_some_and(|right| {
                    value_type(value_types, values[0])
                        .is_some_and(|left| same_type(types, left, right))
                })
                && result.is_some_and(|actual| is_integer_width(types, actual, 64))
        }
        10 | 11 => verify_memory_range(instruction, &values, value_types, types),
        12 => verify_memory_set(instruction, &values, value_types, types),
        13 => {
            values.len() == 1
                && instruction.result_count == 0
                && value_type(value_types, values[0]).is_some_and(|ty| is_memory_pointer(types, ty))
        }
        _ => false,
    }
}

fn verify_load(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
    selector: u64,
) -> bool {
    if values.len() != 1 || instruction.result_count != 1 {
        return false;
    }
    let Some(address) = value_type(value_types, values[0]) else {
        return false;
    };
    let Some(BinaryType::Address { pointee, .. }) = type_entry(types, address) else {
        return false;
    };
    let valid_mode = is_loadable(types, *pointee)
        && match selector {
            0 => super::aggregate_instructions::is_type_trivial(types, *pointee),
            1 => super::aggregate_instructions::is_type_copyable(types, *pointee),
            2 => true,
            _ => false,
        };
    valid_mode && result.is_some_and(|actual| same_type(types, actual, *pointee))
}

fn verify_address_load(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 1 || instruction.result_count != 1 {
        return false;
    }
    let Some(address) = value_type(value_types, values[0]) else {
        return false;
    };
    let Some(BinaryType::Address { pointee, .. }) = type_entry(types, address) else {
        return false;
    };
    is_loadable(types, *pointee) && result.is_some_and(|actual| same_type(types, actual, *pointee))
}

fn verify_store(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    selector: u64,
) -> bool {
    if values.len() != 2 || instruction.result_count != 0 {
        return false;
    }
    let (Some(value), Some(address)) = (
        value_type(value_types, values[0]),
        value_type(value_types, values[1]),
    ) else {
        return false;
    };
    let Some(BinaryType::Address { pointee, .. }) = type_entry(types, address) else {
        return false;
    };
    selector <= 2
        && is_loadable(types, *pointee)
        && same_type(types, value, *pointee)
        && (selector != 0 || super::aggregate_instructions::is_type_trivial(types, *pointee))
}

fn verify_address_copy(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    selector: u64,
) -> bool {
    if values.len() != 2 || instruction.result_count != 0 || selector > 1 {
        return false;
    }
    let (Some(source), Some(destination)) = (
        address_pointee(types, value_type(value_types, values[0])),
        address_pointee(types, value_type(value_types, values[1])),
    ) else {
        return false;
    };
    same_type(types, source, destination)
        && super::aggregate_instructions::is_type_copyable(types, source)
}

fn verify_address_move(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    selector: u64,
) -> bool {
    if values.len() != 2 || instruction.result_count != 0 || selector > 1 {
        return false;
    }
    let (Some(source), Some(destination)) = (
        address_pointee(types, value_type(value_types, values[0])),
        address_pointee(types, value_type(value_types, values[1])),
    ) else {
        return false;
    };
    same_type(types, source, destination)
}

fn verify_raw_load(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    let (Some(pointee), Some(pointer)) = (type_operand(instruction), values.first()) else {
        return false;
    };
    values.len() == 1
        && instruction.result_count == 1
        && value_type(value_types, *pointer).is_some_and(|ty| is_pointer(types, ty))
        && super::aggregate_instructions::is_type_trivial(types, pointee)
        && crate::binary_constants::binary_type_layout(types, pointee).is_ok()
        && result.is_some_and(|actual| same_type(types, actual, pointee))
}

fn verify_raw_store(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 2 || instruction.result_count != 0 {
        return false;
    }
    let (Some(value), Some(pointer)) = (
        value_type(value_types, values[0]),
        value_type(value_types, values[1]),
    ) else {
        return false;
    };
    is_pointer(types, pointer)
        && super::aggregate_instructions::is_type_trivial(types, value)
        && crate::binary_constants::binary_type_layout(types, value).is_ok()
}

fn verify_memory_range(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    values.len() == 3
        && instruction.result_count == 0
        && values.iter().take(2).all(|value| {
            value_type(value_types, *value).is_some_and(|ty| is_memory_pointer(types, ty))
        })
        && value_type(value_types, values[2]).is_some_and(|ty| is_integer_width(types, ty, 64))
}

fn verify_memory_set(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    values.len() == 3
        && instruction.result_count == 0
        && value_type(value_types, values[0]).is_some_and(|ty| is_memory_pointer(types, ty))
        && value_type(value_types, values[1]).is_some_and(|ty| is_integer_width(types, ty, 8))
        && value_type(value_types, values[2]).is_some_and(|ty| is_integer_width(types, ty, 64))
}

fn is_loadable(types: &BinaryTypeTable, ty: u64) -> bool {
    super::aggregate_instructions::is_type_loadable(types, ty)
}

fn is_pointer(types: &BinaryTypeTable, ty: u64) -> bool {
    matches!(type_entry(types, ty), Some(BinaryType::Pointer(_)))
}

fn is_address(types: &BinaryTypeTable, ty: Option<u64>) -> bool {
    ty.and_then(|ty| type_entry(types, ty))
        .is_some_and(|ty| matches!(ty, BinaryType::Address { .. }))
}

fn is_memory_pointer(types: &BinaryTypeTable, ty: u64) -> bool {
    matches!(
        type_entry(types, ty),
        Some(BinaryType::Pointer(_) | BinaryType::Address { .. })
    )
}

fn is_integer_width(types: &BinaryTypeTable, ty: u64, width: u64) -> bool {
    matches!(type_entry(types, ty), Some(BinaryType::Integer(actual)) if *actual == width)
}

fn address_pointee(types: &BinaryTypeTable, address: Option<u64>) -> Option<u64> {
    match type_entry(types, address?)? {
        BinaryType::Address { pointee, .. } => Some(*pointee),
        _ => None,
    }
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

fn value_type(value_types: &[Option<u64>], value: u64) -> Option<u64> {
    value_types
        .get(usize::try_from(value).ok()?)
        .copied()
        .flatten()
}

fn result_type(value_types: &[Option<u64>], start: usize) -> Option<u64> {
    value_types.get(start).copied().flatten()
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

fn type_entry(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn same_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    type_entry(types, left) == type_entry(types, right) && type_entry(types, left).is_some()
}
