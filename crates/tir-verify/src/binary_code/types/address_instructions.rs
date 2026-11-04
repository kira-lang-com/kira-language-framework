use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryTarget, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const SELECTOR_MASK: u64 = 0xf << 48;
const UNCHECKED: u64 = 1 << 33;

pub(super) fn verify(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    target: BinaryTarget,
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
            if (0x120..=0x128).contains(&instruction.opcode)
                && !verify_instruction(instruction, value_types, value_cursor, types, target)
            {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function @{function_name} CODE offset {}: address opcode 0x{:03X} has incompatible operands, result type, or layout",
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
    target: BinaryTarget,
) -> bool {
    let relative = instruction.opcode - 0x120;
    let selector = (instruction.modifiers & SELECTOR_MASK) >> 48;
    let allowed = match relative {
        0 => SELECTOR_MASK,
        3 => UNCHECKED,
        _ => 0,
    };
    if instruction.modifiers & !(allowed | SELECTOR_MASK) != 0
        || (relative != 0 && selector != 0)
        || (relative == 0 && selector > 10)
    {
        return false;
    }
    let values = value_ids(instruction);
    let result = result_type(value_types, result_start);
    match relative {
        0 => verify_stack_alloc(instruction, value_types, result, types, target),
        1 => {
            values.len() == 1
                && instruction.result_count == 0
                && is_address(types, value_type(value_types, values[0]))
        }
        2 => {
            values.len() == 1
                && instruction.result_count == 1
                && super::address::infer_field_address(
                    instruction,
                    value_type(value_types, values[0]),
                    types,
                )
                .is_some_and(|expected| {
                    result.is_some_and(|actual| same_type(types, actual, expected))
                })
        }
        3 => verify_element_address(instruction, &values, value_types, result, types),
        4 => {
            values.len() == 1
                && instruction.result_count == 1
                && super::address::infer_enum_payload_address(
                    instruction,
                    value_type(value_types, values[0]),
                    types,
                )
                .is_some_and(|expected| {
                    result.is_some_and(|actual| same_type(types, actual, expected))
                })
        }
        5 => {
            values.len() == 1
                && instruction.result_count == 0
                && super::address::infer_enum_payload_address(
                    instruction,
                    value_type(value_types, values[0]),
                    types,
                )
                .is_some()
        }
        6 => verify_address_cast(instruction, &values, value_types, result, types),
        7 => verify_addr_to_pointer(
            &values,
            value_types,
            result,
            types,
            instruction.result_count,
        ),
        8 => verify_pointer_to_address(instruction, &values, value_types, result, types),
        _ => false,
    }
}

fn verify_stack_alloc(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
    target: BinaryTarget,
) -> bool {
    let (Some(pointee), Some(actual)) = (type_operand(instruction), result) else {
        return false;
    };
    let Some(BinaryType::Address {
        pointee: actual_pointee,
        ..
    }) = type_entry(types, actual)
    else {
        return false;
    };
    let (Some(selector), Some(count)) = (
        instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::Index(index) => Some(*index),
                _ => None,
            }),
        instruction
            .operands
            .iter()
            .find_map(|operand| match operand {
                BinaryOperand::OptionalValue(value) => Some(*value),
                _ => None,
            }),
    ) else {
        return false;
    };
    let expected_space = match selector {
        0 => match target {
            BinaryTarget::Cpu | BinaryTarget::Both => 0,
            BinaryTarget::Gpu => 4,
        },
        1..=9 => selector as u8,
        10 => 0,
        _ => return false,
    };
    count.is_none_or(|value| value_type(value_types, value).is_some_and(|ty| is_integer(types, ty)))
        && instruction.operands.len() == 3
        && instruction.result_count == 1
        && same_type(types, *actual_pointee, pointee)
        && matches!(type_entry(types, actual), Some(BinaryType::Address { space, .. }) if *space == expected_space)
}

fn verify_element_address(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 2 || instruction.result_count != 1 {
        return false;
    }
    let Some(index) = value_type(value_types, values[1]) else {
        return false;
    };
    is_integer(types, index)
        && super::address::infer_element_address(value_type(value_types, values[0]), types)
            .is_some_and(|expected| result.is_some_and(|actual| same_type(types, actual, expected)))
}

fn verify_address_cast(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 1 || instruction.result_count != 1 {
        return false;
    }
    let (Some(source), Some(target), Some(actual)) = (
        value_type(value_types, values[0]),
        type_operand(instruction),
        result,
    ) else {
        return false;
    };
    let (
        Some(BinaryType::Address {
            pointee: from,
            space: from_space,
        }),
        Some(BinaryType::Address {
            pointee: to,
            space: to_space,
        }),
    ) = (type_entry(types, source), type_entry(types, target))
    else {
        return false;
    };
    from_space == to_space
        && layout_compatible(types, *from, *to)
        && same_type(types, actual, target)
}

fn verify_addr_to_pointer(
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
    result_count: u64,
) -> bool {
    if values.len() != 1 || result_count != 1 {
        return false;
    }
    let Some(address_type) = value_type(value_types, values[0]) else {
        return false;
    };
    let Some(address) = type_entry(types, address_type) else {
        return false;
    };
    let BinaryType::Address { space, .. } = address else {
        return false;
    };
    result.is_some_and(|actual| {
        matches!(type_entry(types, actual), Some(BinaryType::Pointer(pointer_space)) if pointer_space == space)
    })
}

fn verify_pointer_to_address(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if values.len() != 1 || instruction.result_count != 1 {
        return false;
    }
    let (Some(BinaryType::Pointer(pointer_space)), Some(target), Some(actual)) = (
        value_type(value_types, values[0]).and_then(|index| type_entry(types, index)),
        type_operand(instruction),
        result,
    ) else {
        return false;
    };
    matches!(type_entry(types, target), Some(BinaryType::Address { space, .. }) if space == pointer_space)
        && same_type(types, actual, target)
}

fn layout_compatible(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    if same_type(types, left, right) {
        return true;
    }
    match (
        crate::binary_constants::binary_type_layout(types, left),
        crate::binary_constants::binary_type_layout(types, right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
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

fn is_integer(types: &BinaryTypeTable, index: u64) -> bool {
    type_entry(types, index).is_some_and(|ty| matches!(ty, BinaryType::Integer(_)))
}

fn is_address(types: &BinaryTypeTable, index: Option<u64>) -> bool {
    index
        .and_then(|index| type_entry(types, index))
        .is_some_and(|ty| matches!(ty, BinaryType::Address { .. }))
}

fn same_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    type_entry(types, left) == type_entry(types, right) && type_entry(types, left).is_some()
}
