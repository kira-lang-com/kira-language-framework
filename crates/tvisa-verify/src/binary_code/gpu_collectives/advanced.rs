use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    BinaryFunctionBody, BinaryInstruction, BinaryOperand, TvisaValueTypeRecord, TypeSection,
};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(in crate::binary_code) fn infer_result_types(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<Vec<Option<u64>>> {
    if !is_advanced_opcode(instruction.opcode) {
        return None;
    }
    let mut results = vec![None; usize::from(instruction.result_count)];
    if let Some(result) = results.first_mut() {
        *result = infer_result_type(instruction, value_types, types);
    }
    Some(results)
}

pub(super) fn infer_result_type(
    instruction: &BinaryInstruction,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<u64> {
    let values = value_operands(instruction);
    match instruction.opcode {
        0x560 | 0x561 => explicit_type(instruction),
        0x563 => value_type_index(*values.get(2)?, value_types),
        0x564 => explicit_type(instruction),
        0x565 => integer_type(types, 32),
        0x566 => matrix_element_type(*values.first()?, value_types, types),
        0x567 | 0x568 => value_type_index(*values.first()?, value_types),
        0x569 => tensor_element_type(*values.first()?, value_types, types),
        0x56b => integer_type(types, 64),
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
        if !is_advanced_opcode(instruction.opcode) {
            continue;
        }
        let values = value_operands(instruction);
        if values.len() != expected_values(instruction.opcode)
            || invalid_operands(instruction, &values, value_types, types)
        {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU cooperative/tensor opcode 0x{:03X} has invalid operands or types",
                    instruction.opcode
                ),
            ));
        }
        if instruction.result_count != expected_results(instruction.opcode) {
            diagnostics.push(Diagnostic::error(
                VFY3,
                format!(
                    "GPU cooperative/tensor opcode 0x{:03X} has an invalid result count",
                    instruction.opcode
                ),
            ));
        }
    }
    diagnostics
}

fn invalid_operands(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let at = |index| value_record(values, index, value_types, types);
    match instruction.opcode {
        0x560 => explicit_type(instruction)
            .and_then(|index| matrix_type(types, index))
            .is_none_or(|matrix| {
                values
                    .first()
                    .and_then(|value| value_type_index(*value, value_types))
                    != Some(matrix.element_type)
            }),
        0x561 => invalid_load(instruction, values, value_types, types),
        0x562 => {
            let Some(matrix) = at(0).and_then(|ty| matrix_record(ty, types)) else {
                return true;
            };
            !at(1).is_some_and(|ty| memory_pointer(ty))
                || !at(2).is_some_and(is_integer)
                || !matches!(matrix.scope, 0 | 1)
        }
        0x563 => invalid_mma(instruction, values, value_types, types),
        0x564 => {
            let Some(source) = at(0).and_then(|ty| matrix_record(ty, types)) else {
                return true;
            };
            explicit_type(instruction)
                .and_then(|index| matrix_type(types, index))
                .is_none_or(|target| {
                    (source.scope, source.rows, source.columns)
                        != (target.scope, target.rows, target.columns)
                })
        }
        0x565 => explicit_type(instruction)
            .and_then(|index| matrix_type(types, index))
            .is_none(),
        0x566 => {
            at(0).and_then(|ty| matrix_record(ty, types)).is_none()
                || !at(1).is_some_and(is_integer)
        }
        0x567 => {
            let Some(matrix) = at(0).and_then(|ty| matrix_record(ty, types)) else {
                return true;
            };
            !at(1).is_some_and(is_integer)
                || type_record(types, matrix.element_type)
                    .is_none_or(|element| at(2) != Some(element))
        }
        0x568 => invalid_reduce(instruction, values, value_types, types),
        0x569 => {
            let Some((rank, element)) = at(0).and_then(|ty| tensor_record(ty)) else {
                return true;
            };
            !at(1).is_some_and(|ty| coordinate_type(ty, rank, types))
                || element as usize >= types.value_types.len()
        }
        0x56a => {
            let Some((rank, element)) = at(0).and_then(|ty| tensor_record(ty)) else {
                return true;
            };
            !at(1).is_some_and(|ty| coordinate_type(ty, rank, types))
                || at(2) != type_record(types, element)
        }
        0x56b => {
            let Some((rank, _)) = at(0).and_then(|ty| tensor_record(ty)) else {
                return true;
            };
            let dimension = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    BinaryOperand::Integer(value) if !value.negative => {
                        u32::try_from(value.magnitude).ok()
                    }
                    _ => None,
                });
            dimension.is_none_or(|dimension| dimension >= rank)
        }
        _ => true,
    }
}

fn invalid_load(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let Some(target) = explicit_type(instruction).and_then(|index| matrix_type(types, index))
    else {
        return true;
    };
    let Some(source_type) =
        value_type_index(values[0], value_types).and_then(|index| type_record(types, index))
    else {
        return true;
    };
    if let Some((rank, element)) = tensor_record(source_type) {
        return element != target.element_type
            || !value_record(values, 1, value_types, types)
                .is_some_and(|coordinates| coordinate_type(coordinates, rank, types))
            || selector(instruction) != 0;
    }
    !memory_pointer(source_type)
        || !value_record(values, 1, value_types, types).is_some_and(is_integer)
        || target.scope > 1
}

fn invalid_mma(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let Some(a) =
        value_record(values, 0, value_types, types).and_then(|ty| matrix_record(ty, types))
    else {
        return true;
    };
    let Some(b) =
        value_record(values, 1, value_types, types).and_then(|ty| matrix_record(ty, types))
    else {
        return true;
    };
    let Some(c) =
        value_record(values, 2, value_types, types).and_then(|ty| matrix_record(ty, types))
    else {
        return true;
    };
    a.use_kind != 0
        || b.use_kind != 1
        || c.use_kind != 2
        || a.scope != b.scope
        || a.scope != c.scope
        || a.columns != b.rows
        || a.rows != c.rows
        || b.columns != c.columns
        || a.element_type != b.element_type
        || (instruction.modifiers & (1 << 46) != 0
            && !type_record(types, c.element_type).is_some_and(is_integer))
        || (instruction.modifiers & (1 << 2) != 0
            && !type_record(types, a.element_type).is_some_and(is_integer))
}

fn invalid_reduce(
    instruction: &BinaryInstruction,
    values: &[u64],
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> bool {
    let Some(matrix) =
        value_record(values, 0, value_types, types).and_then(|ty| matrix_record(ty, types))
    else {
        return true;
    };
    let operation = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Index(index) => Some(*index),
            _ => None,
        });
    let Some(operation) = operation.filter(|operation| *operation <= 14) else {
        return true;
    };
    let element = type_record(types, matrix.element_type);
    let operation_type_valid = match operation {
        2 | 5 | 8 | 11 => element.is_some_and(is_float),
        0 | 1 | 3 | 4 | 6 | 7 | 9 | 10 | 12 | 13 | 14 => element.is_some_and(is_integer),
        _ => false,
    };
    let axis = selector(instruction);
    !operation_type_valid
        || axis > 2
        || (axis == 2 && (matrix.scope != 0 || matrix.rows % 2 != 0 || matrix.columns % 2 != 0))
}

#[derive(Clone, Copy)]
struct Matrix {
    scope: u8,
    rows: u64,
    columns: u64,
    element_type: u64,
    use_kind: u8,
}

fn matrix_record(ty: &TvisaValueTypeRecord, types: &TypeSection) -> Option<Matrix> {
    match ty {
        TvisaValueTypeRecord::CooperativeMatrix {
            scope,
            rows,
            columns,
            element_type,
            use_kind,
        } if *scope <= 1
            && *rows > 0
            && *columns > 0
            && type_record(types, *element_type).is_some() =>
        {
            Some(Matrix {
                scope: *scope,
                rows: *rows,
                columns: *columns,
                element_type: *element_type,
                use_kind: *use_kind,
            })
        }
        _ => None,
    }
}

fn matrix_type(types: &TypeSection, index: u64) -> Option<Matrix> {
    matrix_record(type_record(types, index)?, types)
}

fn tensor_record(ty: &TvisaValueTypeRecord) -> Option<(u32, u64)> {
    match ty {
        TvisaValueTypeRecord::Tensor { rank, element_type } => {
            Some((u32::try_from(*rank).ok()?, *element_type))
        }
        _ => None,
    }
}

fn tensor_element_type(
    value: u64,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<u64> {
    let type_index = value_type_index(value, value_types)?;
    let ty = type_record(types, type_index)?;
    tensor_record(ty).map(|(_, element)| element)
}

fn matrix_element_type(
    value: u64,
    value_types: &[Option<u64>],
    types: &TypeSection,
) -> Option<u64> {
    let type_index = value_type_index(value, value_types)?;
    matrix_record(type_record(types, type_index)?, types).map(|matrix| matrix.element_type)
}

fn coordinate_type(ty: &TvisaValueTypeRecord, rank: u32, types: &TypeSection) -> bool {
    if rank == 1 {
        return matches!(ty, TvisaValueTypeRecord::Integer { bits: 32 });
    }
    matches!(ty, TvisaValueTypeRecord::FixedVector { lanes, element_type }
        if *lanes == u64::from(rank)
            && matches!(type_record(types, *element_type), Some(TvisaValueTypeRecord::Integer { bits: 32 })))
}

fn explicit_type(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

fn expected_values(opcode: u64) -> usize {
    match opcode {
        0x560 => 1,
        0x561 => 2,
        0x562 | 0x563 | 0x567 | 0x56a => 3,
        0x564 | 0x566 => 2,
        0x568 | 0x56b => 1,
        0x569 => 2,
        0x565 => 0,
        _ => 0,
    }
}

fn expected_results(opcode: u64) -> u8 {
    if matches!(opcode, 0x562 | 0x56a) {
        0
    } else {
        1
    }
}

fn is_advanced_opcode(opcode: u64) -> bool {
    (0x560..=0x56b).contains(&opcode)
}

fn selector(instruction: &BinaryInstruction) -> u64 {
    (instruction.modifiers >> 48) & 0xf
}

fn memory_pointer(ty: &TvisaValueTypeRecord) -> bool {
    matches!(
        ty,
        TvisaValueTypeRecord::Pointer {
            address_space: 4 | 6
        }
    )
}

fn is_integer(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Integer { .. })
}

fn is_float(ty: &TvisaValueTypeRecord) -> bool {
    matches!(ty, TvisaValueTypeRecord::Float { .. })
}

fn type_record(types: &TypeSection, index: u64) -> Option<&TvisaValueTypeRecord> {
    types.value_types.get(usize::try_from(index).ok()?)
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

fn value_type_index(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    value_types
        .get(usize::try_from(value).ok()?)
        .copied()
        .flatten()
}

fn value_record<'a>(
    values: &[u64],
    index: usize,
    value_types: &[Option<u64>],
    types: &'a TypeSection,
) -> Option<&'a TvisaValueTypeRecord> {
    let value = *values.get(index)?;
    type_record(types, value_type_index(value, value_types)?)
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
