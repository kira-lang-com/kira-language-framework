use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);
const MASK_PRESENT: u64 = 1 << 32;

pub(super) fn verify_function(
    body: &BinaryFunctionBody,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for instruction in body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| (0x0c0..=0x0d7).contains(&instruction.opcode))
    {
        if !verify_instruction(instruction, value_types, types) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "vector instruction opcode 0x{:03X} has incompatible operands or selector",
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
    let relative = instruction.opcode - 0x0c0;
    if !valid_selector(relative, selector(instruction))
        || (instruction.modifiers & MASK_PRESENT != 0 && !supports_mask(relative))
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
    let masked = instruction.modifiers & MASK_PRESENT != 0;

    match relative {
        0 => {
            let (Some(target), Some(value)) = (type_operand(instruction), operand_types.first())
            else {
                return false;
            };
            vector_element(types, target) == Some(*value)
        }
        1 => {
            operand_types.len() == 2
                && vector_element(types, operand_types[0]).is_some()
                && is_integer_scalar(types, operand_types[1])
        }
        2 => {
            operand_types.len() == 3
                && vector_element(types, operand_types[0]) == Some(operand_types[2])
                && is_integer_scalar(types, operand_types[1])
        }
        3 => verify_shuffle(instruction, &operand_types, types),
        4 => type_operand(instruction)
            .and_then(|target| vector_element(types, target))
            .is_some_and(|element| is_integer_scalar(types, element)),
        5 => operand_types.is_empty(),
        6 => {
            let expected = 1 + usize::from(masked);
            if operand_types.len() != expected {
                return false;
            }
            let Some(element) = vector_element(types, operand_types[0]) else {
                return false;
            };
            if masked && !mask_matches(types, operand_types[1], operand_types[0]) {
                return false;
            }
            let reduction = selector(instruction);
            if reduction <= 8 {
                is_integer_scalar(types, element)
            } else {
                is_float_scalar(types, element)
            }
        }
        7 => {
            let Some(vector) = type_operand(instruction) else {
                return false;
            };
            let expected = 1 + if masked { 2 } else { 0 };
            operand_types.len() == expected
                && vector_element(types, vector).is_some()
                && is_pointer_scalar(types, operand_types[0])
                && (!masked
                    || (mask_matches(types, operand_types[1], vector)
                        && operand_types[2] == vector))
        }
        8 => {
            let expected = 2 + usize::from(masked);
            operand_types.len() == expected
                && vector_element(types, operand_types[0]).is_some()
                && is_pointer_scalar(types, operand_types[1])
                && (!masked || mask_matches(types, operand_types[2], operand_types[0]))
        }
        9 => verify_gather(instruction, &operand_types, types, masked),
        10 => verify_scatter(instruction, &operand_types, types, masked),
        11 | 12 => {
            operand_types.len() == 3
                && vector_element(types, operand_types[0]).is_some()
                && operand_types[2] == operand_types[0]
                && mask_matches(types, operand_types[1], operand_types[0])
        }
        13 => verify_dot(&operand_types, types, selector(instruction)),
        14 | 15 => {
            operand_types.len() == 2
                && operand_types[0] == operand_types[1]
                && vector_element(types, operand_types[0]).is_some()
        }
        16 => operand_types.len() == 1 && vector_element(types, operand_types[0]).is_some(),
        17 => {
            operand_types.len() == 3
                && operand_types[0] == operand_types[1]
                && vector_element(types, operand_types[0]).is_some()
                && is_integer_scalar(types, operand_types[2])
        }
        18 => verify_concat(&operand_types, types),
        19 => verify_split(&operand_types, types),
        20 => {
            operand_types.len() == 2
                && vector_element(types, operand_types[0]) == integer_type(types, 8)
                && is_integer_vector(types, operand_types[1])
        }
        21 => {
            operand_types.len() == 2
                && operand_types
                    .iter()
                    .all(|operand| is_integer_scalar(types, *operand))
        }
        22 | 23 => operand_types.len() == 1 && is_mask_vector(types, operand_types[0]),
        _ => false,
    }
}

fn verify_shuffle(instruction: &BinaryInstruction, operands: &[u64], types: &TypeSection) -> bool {
    if operands.len() != 2 || operands[0] != operands[1] {
        return false;
    }
    let Some(TvisaValueTypeRecord::FixedVector {
        lanes,
        element_type,
    }) = types.value_types.get(operands[0] as usize)
    else {
        return false;
    };
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
    if !valid_lanes(indices.len() as u64)
        || (is_integer_width(types, *element_type, 1) && indices.len() > 64)
    {
        return false;
    }
    let Some(max_index) = lanes.checked_mul(2) else {
        return false;
    };
    indices
        .iter()
        .all(|index| !index.negative && index.magnitude < u128::from(max_index))
}

fn verify_gather(
    instruction: &BinaryInstruction,
    operands: &[u64],
    types: &TypeSection,
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
    if operands.len() != expected {
        return false;
    }
    let address_valid = if pointer_vector_form {
        is_pointer_vector(types, operands[0]) && same_lanes(types, operands[0], target)
    } else {
        is_pointer_scalar(types, operands[0])
            && is_integer_vector(types, operands[1])
            && same_lanes(types, operands[1], target)
    };
    address_valid
        && (!masked
            || (mask_matches(types, operands[address_end], target)
                && operands[address_end + 1] == target))
}

fn verify_scatter(
    instruction: &BinaryInstruction,
    operands: &[u64],
    types: &TypeSection,
    masked: bool,
) -> bool {
    let Some(value_type) = operands.first().copied() else {
        return false;
    };
    if vector_element(types, value_type).is_none() {
        return false;
    }
    let pointer_vector_form = selector(instruction) == 0;
    let address_end = if pointer_vector_form { 2 } else { 3 };
    let expected = address_end + usize::from(masked);
    if operands.len() != expected {
        return false;
    }
    let address_valid = if pointer_vector_form {
        is_pointer_vector(types, operands[1]) && same_lanes(types, operands[1], value_type)
    } else {
        is_pointer_scalar(types, operands[1])
            && is_integer_vector(types, operands[2])
            && same_lanes(types, operands[2], value_type)
    };
    address_valid && (!masked || mask_matches(types, operands[address_end], value_type))
}

fn verify_dot(operands: &[u64], types: &TypeSection, kind: u64) -> bool {
    if operands.len() != 3 {
        return false;
    }
    let (input_bits, accumulator_bits, factor) = match kind {
        0..=2 => (Some(8), 32, 4),
        3 => (None, 32, 2),
        _ => return false,
    };
    let (Some(accumulator_lanes), Some(left_lanes), Some(right_lanes)) = (
        fixed_lanes(types, operands[0]),
        fixed_lanes(types, operands[1]),
        fixed_lanes(types, operands[2]),
    ) else {
        return false;
    };
    let Some(accumulator_element) = vector_element(types, operands[0]) else {
        return false;
    };
    let Some(input_element) = vector_element(types, operands[1]) else {
        return false;
    };
    let Some(right_element) = vector_element(types, operands[2]) else {
        return false;
    };
    let input_matches = if let Some(bits) = input_bits {
        is_integer_width(types, input_element, bits) && is_integer_width(types, right_element, bits)
    } else {
        is_float_tag(types, input_element, 0x03) && is_float_tag(types, right_element, 0x03)
    };
    let accumulator_matches = if kind == 3 {
        is_float_tag(types, accumulator_element, 0x04)
    } else {
        is_integer_width(types, accumulator_element, accumulator_bits)
    };
    accumulator_matches
        && input_matches
        && left_lanes == accumulator_lanes.saturating_mul(factor)
        && right_lanes == left_lanes
}

fn verify_concat(operands: &[u64], types: &TypeSection) -> bool {
    if operands.len() != 2 {
        return false;
    }
    let (Some(left_lanes), Some(right_lanes)) = (
        fixed_lanes(types, operands[0]),
        fixed_lanes(types, operands[1]),
    ) else {
        return false;
    };
    let (Some(left_element), Some(right_element)) = (
        vector_element(types, operands[0]),
        vector_element(types, operands[1]),
    ) else {
        return false;
    };
    left_element == right_element && left_lanes.checked_add(right_lanes).is_some_and(valid_lanes)
}

fn verify_split(operands: &[u64], types: &TypeSection) -> bool {
    if operands.len() != 1 {
        return false;
    }
    let Some(lanes) = fixed_lanes(types, operands[0]) else {
        return false;
    };
    lanes % 2 == 0 && valid_lanes(lanes / 2)
}

fn valid_selector(relative: u64, selector: u64) -> bool {
    match relative {
        6 => selector <= 14,
        9 | 10 => selector <= 1,
        13 => selector <= 3,
        14 | 15 | 17 | 19 => selector <= 1,
        _ => selector == 0,
    }
}

fn supports_mask(relative: u64) -> bool {
    matches!(relative, 6..=10)
}

fn selector(instruction: &BinaryInstruction) -> u64 {
    (instruction.modifiers >> 48) & 0xf
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

fn vector_element(types: &TypeSection, type_index: u64) -> Option<u64> {
    match types.value_types.get(type_index as usize)? {
        TvisaValueTypeRecord::FixedVector { element_type, .. }
        | TvisaValueTypeRecord::ScalableVector { element_type } => Some(*element_type),
        _ => None,
    }
}

fn same_lanes(types: &TypeSection, left: u64, right: u64) -> bool {
    match (
        types.value_types.get(left as usize),
        types.value_types.get(right as usize),
    ) {
        (
            Some(TvisaValueTypeRecord::FixedVector { lanes: left, .. }),
            Some(TvisaValueTypeRecord::FixedVector { lanes: right, .. }),
        ) => left == right,
        (
            Some(TvisaValueTypeRecord::ScalableVector { .. }),
            Some(TvisaValueTypeRecord::ScalableVector { .. }),
        ) => true,
        _ => false,
    }
}

fn fixed_lanes(types: &TypeSection, type_index: u64) -> Option<u64> {
    match types.value_types.get(type_index as usize)? {
        TvisaValueTypeRecord::FixedVector { lanes, .. } => Some(*lanes),
        _ => None,
    }
}

fn is_integer_scalar(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        types.value_types.get(type_index as usize),
        Some(TvisaValueTypeRecord::Integer { .. })
    )
}

fn is_float_scalar(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        types.value_types.get(type_index as usize),
        Some(TvisaValueTypeRecord::Float { .. })
    )
}

fn is_pointer_scalar(types: &TypeSection, type_index: u64) -> bool {
    matches!(
        types.value_types.get(type_index as usize),
        Some(TvisaValueTypeRecord::Pointer { .. })
    )
}

fn is_integer_vector(types: &TypeSection, type_index: u64) -> bool {
    vector_element(types, type_index).is_some_and(|element| is_integer_scalar(types, element))
}

fn is_pointer_vector(types: &TypeSection, type_index: u64) -> bool {
    vector_element(types, type_index).is_some_and(|element| is_pointer_scalar(types, element))
}

fn is_mask_vector(types: &TypeSection, type_index: u64) -> bool {
    vector_element(types, type_index).is_some_and(|element| is_integer_width(types, element, 1))
}

fn mask_matches(types: &TypeSection, mask: u64, vector: u64) -> bool {
    same_lanes(types, mask, vector) && is_mask_vector(types, mask)
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

fn is_integer_width(types: &TypeSection, type_index: u64, bits: u64) -> bool {
    matches!(types.value_types.get(type_index as usize), Some(TvisaValueTypeRecord::Integer { bits: width }) if *width == bits)
}

fn is_float_tag(types: &TypeSection, type_index: u64, tag: u8) -> bool {
    matches!(types.value_types.get(type_index as usize), Some(TvisaValueTypeRecord::Float { tag: actual }) if *actual == tag)
}

fn valid_lanes(lanes: u64) -> bool {
    matches!(lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64)
}
