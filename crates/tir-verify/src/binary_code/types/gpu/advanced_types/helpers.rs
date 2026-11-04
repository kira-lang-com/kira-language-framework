use tir_diag::Diagnostic;
use tir_format::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};

use super::{Matrix, V2};

pub(super) fn matrix_for_value(
    instruction: &BinaryInstruction,
    values: &[u64],
    operand_index: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Matrix> {
    let type_index = value_type(*values.get(operand_index)?, value_types)?;
    let matrix = matrix_at(types, type_index);
    if matrix.is_none() {
        report(
            diagnostics,
            function_name,
            instruction,
            "cooperative instruction requires a cooperative matrix",
        );
    }
    matrix
}

pub(super) fn tensor_for_value(
    instruction: &BinaryInstruction,
    values: &[u64],
    operand_index: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(u64, u64)> {
    let type_index = value_type(*values.get(operand_index)?, value_types)?;
    let tensor = match type_at(types, type_index) {
        Some(BinaryType::Tensor { rank, element }) => Some((*rank, *element)),
        _ => None,
    };
    if tensor.is_none() {
        report(
            diagnostics,
            function_name,
            instruction,
            "tensor instruction requires a tensor handle",
        );
    }
    tensor
}

pub(super) fn declared_matrix(
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Matrix> {
    let matrix = instruction_type(instruction).and_then(|index| matrix_at(types, index));
    if matrix.is_none() {
        report(
            diagnostics,
            function_name,
            instruction,
            "cooperative instruction requires a cooperative matrix type",
        );
    }
    matrix
}

pub(super) fn matrix_at(types: &BinaryTypeTable, index: u64) -> Option<Matrix> {
    match type_at(types, index)? {
        BinaryType::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        } => Some(Matrix {
            scope: *scope,
            rows: *rows,
            columns: *columns,
            element: *element,
            use_kind: *use_kind,
        }),
        _ => None,
    }
}

pub(super) fn instruction_type(instruction: &BinaryInstruction) -> Option<u64> {
    instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Type(index) => Some(*index),
            _ => None,
        })
}

pub(super) fn require_values<'a>(
    instruction: &'a BinaryInstruction,
    expected: usize,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [u64]> {
    let values = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Values(values) => Some(values.as_slice()),
            _ => None,
        });
    if values.is_none_or(|values| values.len() != expected) {
        report(
            diagnostics,
            function_name,
            instruction,
            "cooperative or tensor instruction has an invalid number of value operands",
        );
        return None;
    }
    values
}

pub(super) fn check_value_type(
    instruction: &BinaryInstruction,
    values: &[u64],
    operand_index: usize,
    value_types: &[Option<u64>],
    types: &BinaryTypeTable,
    predicate: impl FnOnce(&BinaryTypeTable, u64) -> bool,
    message: &str,
    function_name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if values
        .get(operand_index)
        .and_then(|value| value_type(*value, value_types))
        .is_some_and(|actual| !predicate(types, actual))
    {
        report(diagnostics, function_name, instruction, message);
    }
}

pub(super) fn tensor_coordinates(types: &BinaryTypeTable, index: u64, rank: u64) -> bool {
    match type_at(types, index) {
        Some(BinaryType::Integer(_)) => rank == 1,
        Some(BinaryType::FixedVector { lanes, element }) => {
            *lanes == rank && is_i32(types, *element)
        }
        _ => false,
    }
}

pub(super) fn is_integer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(_)))
}

pub(super) fn is_i32(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Integer(32)))
}

pub(super) fn is_float(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Float(_)))
}

pub(super) fn is_memory_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(type_at(types, index), Some(BinaryType::Pointer(1 | 3)))
}

pub(super) fn same_type(types: &BinaryTypeTable, left: u64, right: u64) -> bool {
    left == right
        || matches!(
            (type_at(types, left), type_at(types, right)),
            (Some(left), Some(right)) if left == right
        )
}

pub(super) fn value_type(value: u64, value_types: &[Option<u64>]) -> Option<u64> {
    usize::try_from(value)
        .ok()
        .and_then(|index| value_types.get(index))
        .copied()
        .flatten()
}

pub(super) fn type_at(types: &BinaryTypeTable, index: u64) -> Option<&BinaryType> {
    usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
}

pub(super) fn report(
    diagnostics: &mut Vec<Diagnostic>,
    function_name: &str,
    instruction: &BinaryInstruction,
    message: &str,
) {
    diagnostics.push(Diagnostic::error(
        V2,
        format!(
            "function @{function_name} CODE offset {}: {message}",
            instruction.offset
        ),
    ));
}
