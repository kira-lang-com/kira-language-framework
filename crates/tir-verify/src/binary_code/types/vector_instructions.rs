use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const MASK_PRESENT: u64 = 1 << 32;
const SELECTOR_MASK: u64 = 0xf << 48;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Shape {
    Fixed(u64),
    Scalable,
}

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
            if (0x0c0..=0x0d7).contains(&instruction.opcode)
                && !verify_instruction(instruction, value_types, value_cursor, types)
            {
                diagnostics.push(Diagnostic::error(
                    V2,
                    format!(
                        "function @{function_name} CODE offset {}: vector opcode 0x{:03X} has incompatible operands, result type, or selector",
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
    result_index: usize,
    types: &BinaryTypeTable,
) -> bool {
    let relative = instruction.opcode - 0x0c0;
    let selector = selector(instruction);
    let allowed_modifiers = MASK_PRESENT | SELECTOR_MASK | if relative == 6 { 1 << 12 } else { 0 };
    if instruction.modifiers & !allowed_modifiers != 0
        || !valid_selector(relative, selector)
        || (instruction.modifiers & MASK_PRESENT != 0 && !supports_mask(relative))
    {
        return false;
    }
    let Some(operands) = resolved_operands(instruction, value_types) else {
        return false;
    };
    let masked = instruction.modifiers & MASK_PRESENT != 0;
    let result = u64::try_from(result_index)
        .ok()
        .and_then(|value| value_type(value_types, value));

    match relative {
        0 => {
            let (Some(target), Some(value)) = (type_operand(instruction), operands.first()) else {
                return false;
            };
            instruction.result_count == 1
                && vector_element(types, target).is_some_and(|element| element == *value)
                && result == Some(target)
        }
        1 => {
            operands.len() == 2
                && vector_element(types, operands[0]).is_some_and(|element| {
                    is_integer_scalar(types, operands[1])
                        && result.is_some_and(|actual| actual == element)
                })
                && instruction.result_count == 1
        }
        2 => {
            operands.len() == 3
                && vector_element(types, operands[0]).is_some_and(|element| {
                    is_integer_scalar(types, operands[1])
                        && same_type(types, operands[2], element)
                        && result == Some(operands[0])
                })
                && instruction.result_count == 1
        }
        3 => verify_shuffle(instruction, &operands, result, types),
        4 => type_operand(instruction).is_some_and(|target| {
            instruction.result_count == 1
                && vector_element(types, target)
                    .is_some_and(|element| is_integer_scalar(types, element))
                && result == Some(target)
        }),
        5 => operands.is_empty()
            && instruction.result_count == 1
            && is_integer_width(types, result, 64),
        6 => verify_reduce(instruction, &operands, result, types, masked),
        7 => verify_load(instruction, &operands, result, types, masked),
        8 => verify_store(instruction, &operands, types, masked),
        9 => verify_gather(instruction, &operands, result, types, masked),
        10 => verify_scatter(instruction, &operands, types, masked),
        11 | 12 => {
            operands.len() == 3
                && vector_element(types, operands[0]).is_some()
                && same_vector_type(types, operands[2], operands[0])
                && mask_matches(types, operands[1], operands[0])
                && instruction.result_count == 1
                && result == Some(operands[0])
        }
        13 => verify_dot(&operands, result, selector, instruction.result_count, types),
        14 | 15 => {
            operands.len() == 2
                && same_vector_type(types, operands[0], operands[1])
                && instruction.result_count == 1
                && result == Some(operands[0])
        }
        16 => {
            operands.len() == 1
                && vector_element(types, operands[0]).is_some()
                && instruction.result_count == 1
                && result == Some(operands[0])
        }
        17 => {
            operands.len() == 3
                && same_vector_type(types, operands[0], operands[1])
                && is_integer_scalar(types, operands[2])
                && instruction.result_count == 1
                && result == Some(operands[0])
        }
        18 => verify_concat(&operands, result, instruction.result_count, types),
        19 => verify_split(&operands, result, selector, instruction.result_count, types),
        20 => {
            operands.len() == 2
                && is_integer_scalar(types, operands[0])
                && is_integer_scalar(types, operands[1])
                && instruction.result_count == 1
                && result.is_some_and(|index| {
                    matches!(type_at(types, index), Some(BinaryType::ScalableVector { element }) if matches!(type_at(types, *element), Some(BinaryType::Integer(1))))
                })
        }
        21 | 22 => {
            operands.len() == 1
                && is_mask_vector(types, operands[0])
                && instruction.result_count == 1
                && is_integer_width(types, result, 64)
        }
        23 => verify_table(&operands, result, instruction.result_count, types),
        _ => false,
    }
}

fn verify_shuffle(
    instruction: &BinaryInstruction,
    operands: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
) -> bool {
    if operands.len() != 2 {
        return false;
    }
    let (
        Some(Shape::Fixed(left_lanes)),
        Some(Shape::Fixed(right_lanes)),
        Some(left_element),
        Some(right_element),
    ) = (
        shape(types, operands[0]),
        shape(types, operands[1]),
        vector_element(types, operands[0]),
        vector_element(types, operands[1]),
    )
    else {
        return false;
    };
    if !same_type(types, left_element, right_element) {
        return false;
    }
    let Some(indices) = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Integers(indices) => Some(indices),
            _ => None,
        })
    else {
        return false;
    };
    let Some(max_index) = left_lanes.checked_add(right_lanes) else {
        return false;
    };
    valid_fixed_lanes(indices.len() as u64)
        && indices
            .iter()
            .all(|index| !index.negative && index.magnitude < u128::from(max_index))
        && result.is_some_and(|actual| {
            vector_matches(
                types,
                actual,
                Shape::Fixed(indices.len() as u64),
                left_element,
            )
        })
        && instruction.result_count == 1
}

fn verify_reduce(
    instruction: &BinaryInstruction,
    operands: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    masked: bool,
) -> bool {
    if operands.len() != 1 + usize::from(masked) {
        return false;
    }
    let Some(element) = vector_element(types, operands[0]) else {
        return false;
    };
    let selector = selector(instruction);
    let reassoc = instruction.modifiers & (1 << 12) != 0;
    if reassoc && !matches!(selector, 9 | 10) {
        return false;
    }
    let operator_type_matches = if selector <= 8 {
        is_integer_scalar(types, element)
    } else {
        is_float_scalar(types, element)
    };
    operator_type_matches
        && (!masked || mask_matches(types, operands[1], operands[0]))
        && instruction.result_count == 1
        && result == Some(element)
}

fn verify_load(
    instruction: &BinaryInstruction,
    operands: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    masked: bool,
) -> bool {
    let Some(vector) = type_operand(instruction) else {
        return false;
    };
    let Some(element) = vector_element(types, vector) else {
        return false;
    };
    operands.len() == 1 + if masked { 2 } else { 0 }
        && is_pointer_scalar(types, operands[0])
        && address_pointee_matches(types, operands[0], element)
        && (!masked
            || (mask_matches(types, operands[1], vector)
                && same_vector_type(types, operands[2], vector)))
        && instruction.result_count == 1
        && result == Some(vector)
}

fn verify_store(
    instruction: &BinaryInstruction,
    operands: &[u64],
    types: &BinaryTypeTable,
    masked: bool,
) -> bool {
    let (Some(vector), Some(pointer)) = (operands.first(), operands.get(1)) else {
        return false;
    };
    let Some(element) = vector_element(types, *vector) else {
        return false;
    };
    operands.len() == 2 + usize::from(masked)
        && is_pointer_scalar(types, *pointer)
        && address_pointee_matches(types, *pointer, element)
        && (!masked || mask_matches(types, operands[2], *vector))
        && instruction.result_count == 0
}

fn verify_gather(
    instruction: &BinaryInstruction,
    operands: &[u64],
    result: Option<u64>,
    types: &BinaryTypeTable,
    masked: bool,
) -> bool {
    let Some(target) = type_operand(instruction) else {
        return false;
    };
    if vector_element(types, target).is_none() {
        return false;
    }
    let pointer_vector_form = selector(instruction) == 0;
    let address_end = if pointer_vector_form { 1 } else { 2 };
    let expected = address_end + usize::from(masked) * 2;
    let address_valid = if pointer_vector_form {
        operands
            .first()
            .is_some_and(|pointers| pointer_vector_matches(types, *pointers, target))
    } else {
        operands.len() >= 2
            && is_pointer_scalar(types, operands[0])
            && is_integer_vector(types, operands[1])
            && same_shape(types, operands[1], target)
    };
    operands.len() == expected
        && address_valid
        && (!masked
            || (mask_matches(types, operands[address_end], target)
                && same_vector_type(types, operands[address_end + 1], target)))
        && instruction.result_count == 1
        && result == Some(target)
}

fn verify_scatter(
    instruction: &BinaryInstruction,
    operands: &[u64],
    types: &BinaryTypeTable,
    masked: bool,
) -> bool {
    let Some(vector) = operands.first().copied() else {
        return false;
    };
    if vector_element(types, vector).is_none() {
        return false;
    }
    let pointer_vector_form = selector(instruction) == 0;
    let address_end = if pointer_vector_form { 2 } else { 3 };
    let expected = address_end + usize::from(masked);
    let address_valid = if pointer_vector_form {
        operands
            .get(1)
            .is_some_and(|pointers| pointer_vector_matches(types, *pointers, vector))
    } else {
        operands.len() >= 3
            && is_pointer_scalar(types, operands[1])
            && is_integer_vector(types, operands[2])
            && same_shape(types, operands[2], vector)
    };
    operands.len() == expected
        && address_valid
        && (!masked || mask_matches(types, operands[address_end], vector))
        && instruction.result_count == 0
}

fn verify_dot(
    operands: &[u64],
    result: Option<u64>,
    selector: u64,
    result_count: u64,
    types: &BinaryTypeTable,
) -> bool {
    if operands.len() != 3 || result_count != 1 || result != operands.first().copied() {
        return false;
    }
    let (
        Some(Shape::Fixed(accumulator_lanes)),
        Some(Shape::Fixed(left_lanes)),
        Some(Shape::Fixed(right_lanes)),
    ) = (
        shape(types, operands[0]),
        shape(types, operands[1]),
        shape(types, operands[2]),
    )
    else {
        return false;
    };
    let (Some(accumulator), Some(left), Some(right)) = (
        vector_element(types, operands[0]),
        vector_element(types, operands[1]),
        vector_element(types, operands[2]),
    ) else {
        return false;
    };
    let (input_matches, accumulator_matches, factor) = if selector <= 2 {
        (
            is_integer_width(types, Some(left), 8) && is_integer_width(types, Some(right), 8),
            is_integer_width(types, Some(accumulator), 32),
            4,
        )
    } else if selector == 3 {
        (
            type_is_float(types, left, tir_types::FloatType::Bf16)
                && type_is_float(types, right, tir_types::FloatType::Bf16),
            type_is_float(types, accumulator, tir_types::FloatType::F32),
            2,
        )
    } else {
        return false;
    };
    input_matches
        && accumulator_matches
        && left_lanes == accumulator_lanes.saturating_mul(factor)
        && right_lanes == left_lanes
}

fn verify_concat(
    operands: &[u64],
    result: Option<u64>,
    result_count: u64,
    types: &BinaryTypeTable,
) -> bool {
    if operands.len() != 2 || result_count != 1 {
        return false;
    }
    let (
        Some(Shape::Fixed(left_lanes)),
        Some(Shape::Fixed(right_lanes)),
        Some(left_element),
        Some(right_element),
    ) = (
        shape(types, operands[0]),
        shape(types, operands[1]),
        vector_element(types, operands[0]),
        vector_element(types, operands[1]),
    )
    else {
        return false;
    };
    same_type(types, left_element, right_element)
        && left_lanes.checked_add(right_lanes).is_some_and(|lanes| {
            valid_fixed_lanes(lanes)
                && result.is_some_and(|actual| {
                    vector_matches(types, actual, Shape::Fixed(lanes), left_element)
                })
        })
}

fn verify_split(
    operands: &[u64],
    result: Option<u64>,
    selector: u64,
    result_count: u64,
    types: &BinaryTypeTable,
) -> bool {
    if operands.len() != 1 || result_count != 1 {
        return false;
    }
    let (Some(Shape::Fixed(lanes)), Some(element)) = (
        shape(types, operands[0]),
        vector_element(types, operands[0]),
    ) else {
        return false;
    };
    lanes % 2 == 0
        && valid_fixed_lanes(lanes / 2)
        && selector <= 1
        && result
            .is_some_and(|actual| vector_matches(types, actual, Shape::Fixed(lanes / 2), element))
}

fn verify_table(
    operands: &[u64],
    result: Option<u64>,
    result_count: u64,
    types: &BinaryTypeTable,
) -> bool {
    if operands.len() != 2 || result_count != 1 || !is_integer_vector(types, operands[1]) {
        return false;
    }
    let (Some(table_element), Some(index_shape)) = (
        vector_element(types, operands[0]),
        shape(types, operands[1]),
    ) else {
        return false;
    };
    is_integer_width(types, Some(table_element), 8)
        && result.is_some_and(|actual| vector_matches(types, actual, index_shape, table_element))
}

fn resolved_operands(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
) -> Option<Vec<u64>> {
    instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        })
        .map(|value| value_type(value_types, *value))
        .collect()
}

fn value_type(value_types: &[Option<u64>], value: u64) -> Option<u64> {
    value_types
        .get(usize::try_from(value).ok()?)
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

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn vector_element(types: &BinaryTypeTable, index: u64) -> Option<u64> {
    match type_at(types, index)? {
        BinaryType::FixedVector { element, .. } | BinaryType::ScalableVector { element } => {
            Some(*element)
        }
        _ => None,
    }
}

fn shape(types: &BinaryTypeTable, index: u64) -> Option<Shape> {
    Some(match type_at(types, index)? {
        BinaryType::FixedVector { lanes, .. } => Shape::Fixed(*lanes),
        BinaryType::ScalableVector { .. } => Shape::Scalable,
        _ => return None,
    })
}

fn vector_matches(
    types: &BinaryTypeTable,
    index: u64,
    expected_shape: Shape,
    element: u64,
) -> bool {
    match (expected_shape, type_at(types, index)) {
        (
            Shape::Fixed(lanes),
            Some(BinaryType::FixedVector {
                lanes: actual,
                element: actual_element,
            }),
        ) => lanes == *actual && same_type(types, element, *actual_element),
        (Shape::Scalable, Some(BinaryType::ScalableVector { element: actual })) => {
            same_type(types, element, *actual)
        }
        _ => false,
    }
}

fn same_shape(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    shape(types, left) == shape(types, right)
}

fn same_vector_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    same_shape(types, left, right)
        && vector_element(types, left).is_some_and(|left_element| {
            vector_element(types, right)
                .is_some_and(|right_element| same_type(types, left_element, right_element))
        })
}

fn same_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    type_at(types, left) == type_at(types, right) && type_at(types, left).is_some()
}

fn mask_matches(types: &BinaryTypeTable, mask: u64, vector: u64) -> bool {
    let Some(shape) = shape(types, vector) else {
        return false;
    };
    match (shape, type_at(types, mask)) {
        (
            Shape::Fixed(lanes),
            Some(BinaryType::FixedVector {
                lanes: actual,
                element,
            }),
        ) => lanes == *actual && matches!(type_at(types, *element), Some(BinaryType::Integer(1))),
        (Shape::Scalable, Some(BinaryType::ScalableVector { element })) => {
            matches!(type_at(types, *element), Some(BinaryType::Integer(1)))
        }
        _ => false,
    }
}

fn is_mask_vector(types: &BinaryTypeTable, index: u64) -> bool {
    vector_element(types, index).is_some_and(|element| is_integer_width(types, Some(element), 1))
}

fn is_integer_vector(types: &BinaryTypeTable, index: u64) -> bool {
    vector_element(types, index).is_some_and(|element| is_integer_scalar(types, element))
}

fn is_integer_scalar(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(_)))
}

fn is_float_scalar(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Float(_)))
}

fn is_pointer_scalar(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        type_at(types, index),
        Some(BinaryType::Pointer(_) | BinaryType::Address { .. })
    )
}

fn address_pointee_matches(types: &BinaryTypeTable, pointer: u64, expected: u64) -> bool {
    match type_at(types, pointer) {
        Some(BinaryType::Address { pointee, .. }) => same_type(types, *pointee, expected),
        Some(BinaryType::Pointer(_)) => true,
        _ => false,
    }
}

fn pointer_vector_matches(types: &BinaryTypeTable, pointers: u64, target: u64) -> bool {
    let Some(pointer_element) = vector_element(types, pointers) else {
        return false;
    };
    same_shape(types, pointers, target)
        && matches!(
            type_at(types, pointer_element),
            Some(BinaryType::Pointer(_))
        )
}

fn is_integer_width(types: &BinaryTypeTable, index: Option<u64>, width: u64) -> bool {
    index
        .and_then(|index| type_at(types, index))
        .is_some_and(|ty| matches!(ty, BinaryType::Integer(actual) if *actual == width))
}

fn type_is_float(types: &BinaryTypeTable, index: u64, expected: tir_types::FloatType) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Float(actual)) if *actual == expected)
}

fn valid_selector(relative: u64, selector: u64) -> bool {
    match relative {
        6 => selector <= 14,
        9 | 10 | 14 | 15 | 17 | 19 => selector <= 1,
        13 => selector <= 3,
        _ => selector == 0,
    }
}

fn supports_mask(relative: u64) -> bool {
    matches!(relative, 6..=10)
}

fn selector(instruction: &BinaryInstruction) -> u64 {
    (instruction.modifiers & SELECTOR_MASK) >> 48
}

fn valid_fixed_lanes(lanes: u64) -> bool {
    matches!(lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64)
}
