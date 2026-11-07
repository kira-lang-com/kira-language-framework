use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};

const V2: Code = Code::new(Dialect::Tir, Area::Verifier, 2);
const V19: Code = Code::new(Dialect::Tir, Area::Verifier, 19);

#[derive(Clone, Copy, Eq, PartialEq)]
enum Shape {
    Scalar,
    Fixed(u64),
    Scalable,
}

#[derive(Clone, Copy)]
enum NumericKind {
    Integer(u64),
    Float(u16),
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
            if let Some((code, message)) =
                verify_instruction(instruction, value_types, value_cursor, types)
            {
                diagnostics.push(Diagnostic::error(
                    code,
                    format!(
                        "function @{function_name} CODE offset {}: {message}",
                        instruction.offset
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
) -> Option<(Code, String)> {
    match instruction.opcode {
        0x020..=0x045 | 0x060..=0x092 | 0x0a0..=0x0ac => {
            if resolved_operands(instruction, value_types).is_none() {
                return Some((
                    V2,
                    format!(
                        "opcode 0x{:03X} has an operand whose type cannot be inferred",
                        instruction.opcode
                    ),
                ));
            }
            match instruction.opcode {
                0x020..=0x045 => verify_integer(instruction, value_types, result_index, types)
                    .map(|message| (V2, message)),
                0x060..=0x092 => verify_float(instruction, value_types, result_index, types)
                    .map(|message| (V2, message)),
                0x0a0..=0x0ac => verify_conversion(instruction, value_types, types),
                _ => None,
            }
        }
        _ => None,
    }
}

fn verify_integer(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result_index: usize,
    types: &BinaryTypeTable,
) -> Option<String> {
    let relative = instruction.opcode - 0x020;
    let operands = resolved_operands(instruction, value_types)?;
    let first = *operands.first()?;
    let valid = match relative {
        3..=5 => {
            operands.len() == 2
                && is_integer(types, first)
                && operands[1] == first
                && result_has_overflow_shape(instruction, result_index, first, value_types, types)
        }
        14..=16 | 30..=34 => {
            operands.len() == 1
                && is_integer(types, first)
                && (relative != 33 || integer_width_multiple_of_16(types, first))
                && result_type(value_types, result_index) == Some(first)
        }
        37 => {
            operands.len() == 1
                && (is_integer(types, first)
                    || is_float(types, first)
                    || matches!(type_at(types, first), Some(BinaryType::Pointer(_))))
                && result_type(value_types, result_index) == Some(first)
        }
        22 | 23 => {
            operands.len() == 3
                && operands.iter().all(|operand| *operand == first)
                && is_integer(types, first)
                && result_type(value_types, result_index) == Some(first)
        }
        28 => {
            operands.len() == 2
                && operands[1] == first
                && is_integer_or_pointer_compare(types, first)
                && result_has_mask_shape(instruction, result_index, first, value_types, types)
        }
        29 => {
            operands.len() == 3
                && operands[1] == operands[2]
                && result_type(value_types, result_index) == Some(operands[1])
                && mask_matches(types, operands[0], operands[1])
        }
        35 | 36 => {
            operands.len() == 2
                && operands[1] == first
                && matches!(type_at(types, first), Some(BinaryType::Integer(32 | 64)))
                && result_type(value_types, result_index) == Some(first)
        }
        _ => {
            operands.len() == 2
                && operands[1] == first
                && is_integer(types, first)
                && result_type(value_types, result_index) == Some(first)
        }
    };
    (!valid).then(|| {
        format!(
            "integer opcode 0x{:03X} has incompatible operand or result types",
            instruction.opcode
        )
    })
}

fn verify_float(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    result_index: usize,
    types: &BinaryTypeTable,
) -> Option<String> {
    let relative = instruction.opcode - 0x060;
    let operands = resolved_operands(instruction, value_types)?;
    let first = operands.first().copied();
    let valid = match relative {
        20 => {
            operands.len() == 2
                && first.is_some_and(|first| operands[1] == first && is_float(types, first))
                && first.is_some_and(|first| {
                    result_has_mask_shape(instruction, result_index, first, value_types, types)
                })
        }
        21 => first.is_some_and(|first| {
            operands.len() == 1
                && is_float(types, first)
                && result_matches_lane(
                    types,
                    result_type(value_types, result_index),
                    first,
                    BinaryType::Integer(32),
                )
        }),
        22 => first.is_some_and(|first| {
            operands.len() == 1
                && is_float(types, first)
                && result_matches_frexp(types, result_type(value_types, result_index), first)
        }),
        23 => first.is_some_and(|first| {
            operands.len() == 2
                && is_float(types, first)
                && result_type(value_types, result_index) == Some(first)
                && integer_type_matches_shape(
                    types,
                    operands[1],
                    32,
                    shape(types, first).unwrap_or(Shape::Scalar),
                )
        }),
        24 => {
            operands.is_empty()
                && type_is_integer_width(types, result_type(value_types, result_index), 32)
        }
        25 => operands.is_empty() && instruction.result_count == 0,
        _ => {
            let arity = float_arity(relative);
            operands.len() == arity
                && first.is_some_and(|first| is_float(types, first))
                && first.is_some_and(|first| operands.iter().all(|operand| *operand == first))
                && result_type(value_types, result_index) == first
        }
    };
    (!valid).then(|| {
        format!(
            "floating-point opcode 0x{:03X} has incompatible operand or result types",
            instruction.opcode
        )
    })
}

fn float_arity(relative: u64) -> usize {
    match relative {
        5..=7 | 15..=19 | 21 | 22 | 26..=31 | 33..=46 | 48 | 50 => 1,
        8 | 9 => 3,
        _ => 2,
    }
}

fn verify_conversion(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> Option<(Code, String)> {
    let source = resolved_operands(instruction, value_types)?
        .first()
        .copied()?;
    let target = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })?;
    let function_bitcast = (instruction.opcode == 0x0ab)
        .then(|| function_bitcast_compatible(types, source, target))
        .flatten();
    let valid = if let Some(valid) = function_bitcast {
        valid
    } else {
        match instruction.opcode - 0x0a0 {
        0 => numeric_widths(types, source, target).is_some_and(|(from, to, same_shape)| {
            same_shape
                && matches!((from, to), (NumericKind::Integer(a), NumericKind::Integer(b)) if a > b)
        }),
        1 | 2 => numeric_widths(types, source, target).is_some_and(|(from, to, same_shape)| {
            same_shape
                && matches!((from, to), (NumericKind::Integer(a), NumericKind::Integer(b)) if a < b)
        }),
        3 => numeric_widths(types, source, target).is_some_and(|(from, to, same_shape)| {
            same_shape
                && matches!((from, to), (NumericKind::Float(a), NumericKind::Float(b)) if a > b)
        }),
        4 => numeric_widths(types, source, target).is_some_and(|(from, to, same_shape)| {
            same_shape
                && matches!((from, to), (NumericKind::Float(a), NumericKind::Float(b)) if a < b)
        }),
        5 | 6 => numeric_widths(types, source, target).is_some_and(|(from, to, same_shape)| {
            same_shape && matches!((from, to), (NumericKind::Integer(_), NumericKind::Float(_)))
        }),
        7 | 8 => numeric_widths(types, source, target).is_some_and(|(from, to, same_shape)| {
            same_shape && matches!((from, to), (NumericKind::Float(_), NumericKind::Integer(_)))
        }),
        9 => {
            matches!(type_at(types, source), Some(BinaryType::Pointer(_)))
                && type_is_integer_width(types, Some(target), 64)
        }
        10 => {
            type_is_integer_width(types, Some(source), 64)
                && matches!(type_at(types, target), Some(BinaryType::Pointer(0..=2)))
        }
        11 => bitcast_compatible(types, source, target),
        12 => matches!(
            numeric_widths(types, source, target),
            Some((NumericKind::Float(_), NumericKind::Float(_), true))
        ),
            _ => false,
        }
    };
    let code = if function_bitcast == Some(false) {
        V19
    } else {
        V2
    };
    (!valid).then(|| {
        (
            code,
            format!(
                "conversion opcode 0x{:03X} cannot convert type {source} to type {target}",
                instruction.opcode
            ),
        )
    })
}

fn resolved_operands(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
) -> Option<Vec<u64>> {
    let values = instruction
        .operands
        .iter()
        .flat_map(|operand| match operand {
            BinaryOperand::Value(value) => std::slice::from_ref(value),
            BinaryOperand::Values(values) => values.as_slice(),
            _ => &[],
        });
    values
        .map(|value| value_type(value_types, *value))
        .collect()
}

fn value_type(value_types: &[Option<u64>], value: u64) -> Option<u64> {
    value_types
        .get(usize::try_from(value).ok()?)
        .copied()
        .flatten()
}

fn result_type(value_types: &[Option<u64>], result_index: usize) -> Option<u64> {
    value_types.get(result_index).copied().flatten()
}

fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    types.entries.get(usize::try_from(index).ok()?)
}

fn is_integer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(numeric_kind(types, index), Some(NumericKind::Integer(_)))
}

fn integer_width_multiple_of_16(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(numeric_kind(types, index), Some(NumericKind::Integer(bits)) if bits % 16 == 0)
}

fn is_float(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(numeric_kind(types, index), Some(NumericKind::Float(_)))
}

fn numeric_kind(types: &BinaryTypeTable, index: u64) -> Option<NumericKind> {
    let element = element_type(types, index)?;
    match type_at(types, element)? {
        BinaryType::Integer(bits) => Some(NumericKind::Integer(*bits)),
        BinaryType::Float(float) => Some(NumericKind::Float(float.bits())),
        _ => None,
    }
}

fn numeric_widths(
    types: &BinaryTypeTable,
    source: u64,
    target: u64,
) -> Option<(NumericKind, NumericKind, bool)> {
    let (source_kind, target_kind) = (numeric_kind(types, source)?, numeric_kind(types, target)?);
    Some((
        source_kind,
        target_kind,
        shape(types, source)? == shape(types, target)?,
    ))
}

fn element_type(types: &BinaryTypeTable, index: u64) -> Option<u64> {
    match type_at(types, index)? {
        BinaryType::FixedVector { element, .. } | BinaryType::ScalableVector { element } => {
            Some(*element)
        }
        _ => Some(index),
    }
}

fn shape(types: &BinaryTypeTable, index: u64) -> Option<Shape> {
    Some(match type_at(types, index)? {
        BinaryType::FixedVector { lanes, .. } => Shape::Fixed(*lanes),
        BinaryType::ScalableVector { .. } => Shape::Scalable,
        _ => Shape::Scalar,
    })
}

fn is_integer_or_pointer_compare(types: &BinaryTypeTable, index: u64) -> bool {
    is_integer(types, index) || matches!(type_at(types, index), Some(BinaryType::Pointer(_)))
}

fn mask_matches(types: &BinaryTypeTable, mask: u64, value: u64) -> bool {
    match (shape(types, value), type_at(types, mask)) {
        (Some(Shape::Scalar), Some(BinaryType::Integer(1))) => true,
        (
            Some(Shape::Fixed(lanes)),
            Some(BinaryType::FixedVector {
                lanes: actual,
                element,
            }),
        ) => lanes == *actual && matches!(type_at(types, *element), Some(BinaryType::Integer(1))),
        (Some(Shape::Scalable), Some(BinaryType::ScalableVector { element })) => {
            matches!(type_at(types, *element), Some(BinaryType::Integer(1)))
        }
        _ => false,
    }
}

fn result_has_mask_shape(
    instruction: &BinaryInstruction,
    result_index: usize,
    input: u64,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    result_type(value_types, result_index).is_some_and(|result| mask_matches(types, result, input))
        && instruction.result_count == 1
}

fn result_has_overflow_shape(
    instruction: &BinaryInstruction,
    result_index: usize,
    input: u64,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
) -> bool {
    let Some(BinaryType::Tuple(elements)) =
        result_type(value_types, result_index).and_then(|ty| type_at(types, ty))
    else {
        return false;
    };
    elements.len() == 2
        && elements[0] == input
        && mask_matches(types, elements[1], input)
        && instruction.result_count == 1
}

fn result_matches_lane(
    types: &BinaryTypeTable,
    result: Option<u64>,
    input: u64,
    expected_scalar: BinaryType,
) -> bool {
    let Some(result) = result.and_then(|index| type_at(types, index)) else {
        return false;
    };
    let Some(input_shape) = shape(types, input) else {
        return false;
    };
    match (input_shape, result) {
        (Shape::Scalar, actual) => actual == &expected_scalar,
        (
            Shape::Fixed(lanes),
            BinaryType::FixedVector {
                lanes: actual,
                element,
            },
        ) => lanes == *actual && type_at(types, *element) == Some(&expected_scalar),
        (Shape::Scalable, BinaryType::ScalableVector { element }) => {
            type_at(types, *element) == Some(&expected_scalar)
        }
        _ => false,
    }
}

fn result_matches_frexp(types: &BinaryTypeTable, result: Option<u64>, input: u64) -> bool {
    let Some(BinaryType::Tuple(elements)) = result.and_then(|index| type_at(types, index)) else {
        return false;
    };
    elements.len() == 2
        && elements[0] == input
        && result_matches_lane(types, Some(elements[1]), input, BinaryType::Integer(32))
}

fn integer_type_matches_shape(
    types: &BinaryTypeTable,
    index: u64,
    width: u64,
    expected: Shape,
) -> bool {
    let Some(actual_shape) = shape(types, index) else {
        return false;
    };
    actual_shape == expected
        && matches!(numeric_kind(types, index), Some(NumericKind::Integer(actual)) if actual == width)
}

fn type_is_integer_width(types: &BinaryTypeTable, index: Option<u64>, width: u64) -> bool {
    index
        .and_then(|index| type_at(types, index))
        .is_some_and(|ty| matches!(ty, BinaryType::Integer(actual) if *actual == width))
}

fn bitcast_compatible(types: &BinaryTypeTable, source: u64, target: u64) -> bool {
    if let Some(valid) = function_bitcast_compatible(types, source, target) {
        return valid;
    }
    if matches!(
        type_at(types, source),
        Some(BinaryType::Function(_) | BinaryType::Closure { .. })
    ) || matches!(
        type_at(types, target),
        Some(BinaryType::Function(_) | BinaryType::Closure { .. })
    ) {
        return false;
    }
    match (bit_size(types, source), bit_size(types, target)) {
        (Some(source_size), Some(target_size)) => source_size == target_size,
        _ => true,
    }
}

fn function_bitcast_compatible(types: &BinaryTypeTable, source: u64, target: u64) -> Option<bool> {
    match (type_at(types, source), type_at(types, target)) {
        (Some(BinaryType::Function(left)), Some(BinaryType::Function(right))) => {
            Some(left == right)
        }
        (Some(BinaryType::Function(function)), Some(BinaryType::Pointer(0)))
        | (Some(BinaryType::Pointer(0)), Some(BinaryType::Function(function))) => {
            Some(function.foreign)
        }
        (Some(BinaryType::Function(_)), Some(BinaryType::Pointer(_)))
        | (Some(BinaryType::Pointer(_)), Some(BinaryType::Function(_))) => Some(false),
        _ => None,
    }
}

fn bit_size(types: &BinaryTypeTable, index: u64) -> Option<u64> {
    match type_at(types, index)? {
        BinaryType::Integer(bits) => Some(*bits),
        BinaryType::Float(float) => Some(u64::from(float.bits())),
        BinaryType::Pointer(_) => Some(64),
        BinaryType::FixedVector { lanes, element } => lanes.checked_mul(bit_size(types, *element)?),
        BinaryType::ScalableVector { .. } => None,
        _ => None,
    }
}

#[cfg(test)]
mod extended_integer_tests {
    use super::*;

    fn verify_width(opcode: u64, ty: BinaryType, element: Option<BinaryType>) -> bool {
        let mut types = BinaryTypeTable::default();
        let ty_index = if let Some(element) = element {
            types.entries.push(element);
            types.entries.push(ty);
            1
        } else {
            types.entries.push(ty);
            0
        };
        let operands = if opcode == 0x041 {
            vec![BinaryOperand::Value(0)]
        } else {
            vec![BinaryOperand::Value(0), BinaryOperand::Value(0)]
        };
        let instruction = BinaryInstruction {
            offset: 0,
            opcode,
            modifiers: 0,
            result_count: 1,
            terminator: false,
            operands,
        };
        verify_integer(&instruction, &[Some(ty_index), Some(ty_index)], 1, &types).is_none()
    }

    #[test]
    fn v2_bswap_enforces_element_width_in_binary() {
        for width in [8, 24, 31, 40] {
            assert!(!verify_width(0x041, BinaryType::Integer(width), None));
        }
        for width in [16, 32, 64, 128] {
            assert!(verify_width(0x041, BinaryType::Integer(width), None));
        }
        assert!(!verify_width(
            0x041,
            BinaryType::FixedVector {
                lanes: 4,
                element: 0
            },
            Some(BinaryType::Integer(8))
        ));
        assert!(verify_width(
            0x041,
            BinaryType::FixedVector {
                lanes: 4,
                element: 0
            },
            Some(BinaryType::Integer(32))
        ));
    }

    #[test]
    fn v2_clmul_and_clmulh_enforce_32_or_64_bit_elements() {
        for opcode in [0x043, 0x044] {
            for width in [1, 8, 16, 128] {
                assert!(!verify_width(opcode, BinaryType::Integer(width), None));
            }
            for width in [32, 64] {
                assert!(verify_width(opcode, BinaryType::Integer(width), None));
            }
            assert!(!verify_width(
                opcode,
                BinaryType::FixedVector {
                    lanes: 2,
                    element: 0
                },
                Some(BinaryType::Integer(16))
            ));
            assert!(!verify_width(
                opcode,
                BinaryType::FixedVector {
                    lanes: 2,
                    element: 0
                },
                Some(BinaryType::Integer(64))
            ));
        }
    }
}
