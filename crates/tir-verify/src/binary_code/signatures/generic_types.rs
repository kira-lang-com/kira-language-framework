use std::collections::HashSet;

use tir_format::{
    BinaryFunctionType, BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable,
};

use super::helpers::{contains_generic, frame_type, result_edge_types, type_at};

pub(super) fn call_type_arguments(
    instruction: &BinaryInstruction,
    generic_count: usize,
) -> Option<&[u64]> {
    let arguments = instruction
        .operands
        .iter()
        .find_map(|operand| match operand {
            BinaryOperand::Types(arguments) => Some(arguments.as_slice()),
            _ => None,
        })?;
    (arguments.len() == generic_count).then_some(arguments)
}

pub(super) fn specialized_result_edge_types(
    types: &BinaryTypeTable,
    result: u64,
    type_arguments: Option<&[u64]>,
) -> Vec<u64> {
    let result = match (type_at(types, result), type_arguments) {
        (Some(BinaryType::Generic { parameter }), Some(arguments)) => usize::try_from(*parameter)
            .ok()
            .and_then(|index| arguments.get(index))
            .copied()
            .unwrap_or(result),
        _ => result,
    };
    result_edge_types(types, result)
}

pub(super) fn call_result_edge_types(
    types: &BinaryTypeTable,
    function: &BinaryFunctionType,
    function_index: u64,
    type_arguments: Option<&[u64]>,
) -> (Vec<u64>, bool) {
    if function.asynchronous {
        frame_type(types, function_index)
            .map(|frame| (vec![frame], true))
            .unwrap_or_else(|| (vec![0], false))
    } else {
        (
            specialized_result_edge_types(types, function.result, type_arguments),
            true,
        )
    }
}

pub(super) fn type_matches_with_arguments(
    types: &BinaryTypeTable,
    expected: u64,
    actual: u64,
    type_arguments: Option<&[u64]>,
) -> bool {
    let Some(arguments) = type_arguments else {
        return !contains_generic(types, expected, &mut Vec::new()) || expected == actual;
    };
    matches_after_substitution(types, expected, actual, arguments, &mut HashSet::new())
}

fn matches_after_substitution(
    types: &BinaryTypeTable,
    expected: u64,
    actual: u64,
    arguments: &[u64],
    seen: &mut HashSet<(u64, u64)>,
) -> bool {
    if let Some(BinaryType::Generic { parameter }) = type_at(types, expected) {
        return usize::try_from(*parameter)
            .ok()
            .and_then(|index| arguments.get(index))
            == Some(&actual);
    }
    if !contains_generic(types, expected, &mut Vec::new()) {
        return expected == actual;
    }
    if !seen.insert((expected, actual)) {
        return true;
    }
    let (Some(expected), Some(actual)) = (type_at(types, expected), type_at(types, actual)) else {
        return false;
    };
    let same = |left: u64, right: u64, seen: &mut HashSet<(u64, u64)>| {
        matches_after_substitution(types, left, right, arguments, seen)
    };
    match (expected, actual) {
        (
            BinaryType::Address {
                pointee: expected,
                space: expected_space,
            },
            BinaryType::Address {
                pointee: actual,
                space: actual_space,
            },
        ) => expected_space == actual_space && same(*expected, *actual, seen),
        (
            BinaryType::FixedVector {
                lanes: expected_lanes,
                element: expected,
            },
            BinaryType::FixedVector {
                lanes: actual_lanes,
                element: actual,
            },
        ) => expected_lanes == actual_lanes && same(*expected, *actual, seen),
        (
            BinaryType::ScalableVector { element: expected },
            BinaryType::ScalableVector { element: actual },
        )
        | (
            BinaryType::Reference { pointee: expected },
            BinaryType::Reference { pointee: actual },
        )
        | (BinaryType::Unique { pointee: expected }, BinaryType::Unique { pointee: actual })
        | (BinaryType::Weak { pointee: expected }, BinaryType::Weak { pointee: actual }) => {
            same(*expected, *actual, seen)
        }
        (
            BinaryType::Array {
                count: expected_count,
                element: expected,
            },
            BinaryType::Array {
                count: actual_count,
                element: actual,
            },
        ) => expected_count == actual_count && same(*expected, *actual, seen),
        (BinaryType::Tuple(expected), BinaryType::Tuple(actual)) => {
            index_lists_match(types, expected, actual, arguments, seen)
        }
        (
            BinaryType::Nominal {
                declaration: expected_declaration,
                arguments: expected,
            },
            BinaryType::Nominal {
                declaration: actual_declaration,
                arguments: actual,
            },
        ) => {
            expected_declaration == actual_declaration
                && index_lists_match(types, expected, actual, arguments, seen)
        }
        (BinaryType::Function(expected), BinaryType::Function(actual)) => {
            function_types_match(types, expected, actual, arguments, seen)
        }
        (
            BinaryType::Closure {
                function_type: expected,
                borrowed: expected_borrowed,
            },
            BinaryType::Closure {
                function_type: actual,
                borrowed: actual_borrowed,
            },
        ) => expected_borrowed == actual_borrowed && same(*expected, *actual, seen),
        (
            BinaryType::Coroutine {
                yield_type: expected_yield,
                resume_type: expected_resume,
                result_type: expected_result,
            },
            BinaryType::Coroutine {
                yield_type: actual_yield,
                resume_type: actual_resume,
                result_type: actual_result,
            },
        ) => {
            same(*expected_yield, *actual_yield, seen)
                && same(*expected_resume, *actual_resume, seen)
                && same(*expected_result, *actual_result, seen)
        }
        (
            BinaryType::Texture {
                dimension: expected_dimension,
                component: expected_component,
                access: expected_access,
                multisampled: expected_multisampled,
            },
            BinaryType::Texture {
                dimension: actual_dimension,
                component: actual_component,
                access: actual_access,
                multisampled: actual_multisampled,
            },
        ) => {
            expected_dimension == actual_dimension
                && expected_access == actual_access
                && expected_multisampled == actual_multisampled
                && same(*expected_component, *actual_component, seen)
        }
        (
            BinaryType::VisibleFunctionTable {
                function_type: expected,
            },
            BinaryType::VisibleFunctionTable {
                function_type: actual,
            },
        ) => same(*expected, *actual, seen),
        (
            BinaryType::Tensor {
                rank: expected_rank,
                element: expected,
            },
            BinaryType::Tensor {
                rank: actual_rank,
                element: actual,
            },
        ) => expected_rank == actual_rank && same(*expected, *actual, seen),
        (
            BinaryType::CooperativeMatrix {
                scope: expected_scope,
                rows: expected_rows,
                columns: expected_columns,
                element: expected,
                use_kind: expected_use,
            },
            BinaryType::CooperativeMatrix {
                scope: actual_scope,
                rows: actual_rows,
                columns: actual_columns,
                element: actual,
                use_kind: actual_use,
            },
        ) => {
            expected_scope == actual_scope
                && expected_rows == actual_rows
                && expected_columns == actual_columns
                && expected_use == actual_use
                && same(*expected, *actual, seen)
        }
        (
            BinaryType::Entry {
                kind: expected_kind,
                root: expected_root,
                immediate: expected_immediate,
            },
            BinaryType::Entry {
                kind: actual_kind,
                root: actual_root,
                immediate: actual_immediate,
            },
        ) => {
            expected_kind == actual_kind
                && same(*expected_root, *actual_root, seen)
                && same(*expected_immediate, *actual_immediate, seen)
        }
        _ => false,
    }
}

fn index_lists_match(
    types: &BinaryTypeTable,
    expected: &[u64],
    actual: &[u64],
    arguments: &[u64],
    seen: &mut HashSet<(u64, u64)>,
) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(expected, actual)| {
            matches_after_substitution(types, *expected, *actual, arguments, seen)
        })
}

fn function_types_match(
    types: &BinaryTypeTable,
    expected: &BinaryFunctionType,
    actual: &BinaryFunctionType,
    arguments: &[u64],
    seen: &mut HashSet<(u64, u64)>,
) -> bool {
    expected.parameters.len() == actual.parameters.len()
        && expected.asynchronous == actual.asynchronous
        && expected.c_abi == actual.c_abi
        && expected.foreign == actual.foreign
        && expected
            .parameters
            .iter()
            .zip(&actual.parameters)
            .all(|(expected, actual)| {
                expected.convention == actual.convention
                    && expected.context == actual.context
                    && matches_after_substitution(types, expected.ty, actual.ty, arguments, seen)
            })
        && matches_after_substitution(types, expected.result, actual.result, arguments, seen)
        && match (expected.error, actual.error) {
            (Some(expected), Some(actual)) => {
                matches_after_substitution(types, expected, actual, arguments, seen)
            }
            (None, None) => true,
            _ => false,
        }
}

pub(super) fn infer_type_arguments(
    types: &BinaryTypeTable,
    requirement: u64,
    witness: u64,
    arguments: &mut [Option<u64>],
) -> bool {
    infer_type_indices(types, requirement, witness, arguments, &mut HashSet::new())
}

fn infer_type_indices(
    types: &BinaryTypeTable,
    requirement: u64,
    witness: u64,
    arguments: &mut [Option<u64>],
    seen: &mut HashSet<(u64, u64)>,
) -> bool {
    if let Some(BinaryType::Generic { parameter }) = type_at(types, requirement) {
        let Some(argument) = usize::try_from(*parameter)
            .ok()
            .and_then(|parameter| arguments.get_mut(parameter))
        else {
            return false;
        };
        return match argument {
            Some(previous) => *previous == witness,
            None => {
                *argument = Some(witness);
                true
            }
        };
    }
    if !contains_generic(types, requirement, &mut Vec::new()) {
        return requirement == witness;
    }
    if !seen.insert((requirement, witness)) {
        return true;
    }
    let (Some(required), Some(actual)) = (type_at(types, requirement), type_at(types, witness))
    else {
        return false;
    };
    let same = |left: u64, right: u64, arguments: &mut [Option<u64>], seen: &mut HashSet<_>| {
        infer_type_indices(types, left, right, arguments, seen)
    };
    match (required, actual) {
        (
            BinaryType::Address {
                pointee: required,
                space: required_space,
            },
            BinaryType::Address {
                pointee: actual,
                space: actual_space,
            },
        ) => required_space == actual_space && same(*required, *actual, arguments, seen),
        (
            BinaryType::FixedVector {
                lanes: required_lanes,
                element: required,
            },
            BinaryType::FixedVector {
                lanes: actual_lanes,
                element: actual,
            },
        ) => required_lanes == actual_lanes && same(*required, *actual, arguments, seen),
        (
            BinaryType::ScalableVector { element: required },
            BinaryType::ScalableVector { element: actual },
        )
        | (
            BinaryType::Reference { pointee: required },
            BinaryType::Reference { pointee: actual },
        )
        | (BinaryType::Unique { pointee: required }, BinaryType::Unique { pointee: actual })
        | (BinaryType::Weak { pointee: required }, BinaryType::Weak { pointee: actual }) => {
            same(*required, *actual, arguments, seen)
        }
        (
            BinaryType::Array {
                count: required_count,
                element: required,
            },
            BinaryType::Array {
                count: actual_count,
                element: actual,
            },
        ) => required_count == actual_count && same(*required, *actual, arguments, seen),
        (BinaryType::Tuple(required), BinaryType::Tuple(actual)) => {
            infer_type_lists(types, required, actual, arguments, seen)
        }
        (
            BinaryType::Nominal {
                declaration: required_declaration,
                arguments: required,
            },
            BinaryType::Nominal {
                declaration: actual_declaration,
                arguments: actual,
            },
        ) => {
            required_declaration == actual_declaration
                && infer_type_lists(types, required, actual, arguments, seen)
        }
        (BinaryType::Function(required), BinaryType::Function(actual)) => {
            infer_function_types(types, required, actual, arguments, seen)
        }
        (
            BinaryType::Closure {
                function_type: required,
                borrowed: required_borrowed,
            },
            BinaryType::Closure {
                function_type: actual,
                borrowed: actual_borrowed,
            },
        ) => required_borrowed == actual_borrowed && same(*required, *actual, arguments, seen),
        (
            BinaryType::Coroutine {
                yield_type: required_yield,
                resume_type: required_resume,
                result_type: required_result,
            },
            BinaryType::Coroutine {
                yield_type: actual_yield,
                resume_type: actual_resume,
                result_type: actual_result,
            },
        ) => {
            same(*required_yield, *actual_yield, arguments, seen)
                && same(*required_resume, *actual_resume, arguments, seen)
                && same(*required_result, *actual_result, arguments, seen)
        }
        (
            BinaryType::Texture {
                dimension: required_dimension,
                component: required_component,
                access: required_access,
                multisampled: required_multisampled,
            },
            BinaryType::Texture {
                dimension: actual_dimension,
                component: actual_component,
                access: actual_access,
                multisampled: actual_multisampled,
            },
        ) => {
            required_dimension == actual_dimension
                && required_access == actual_access
                && required_multisampled == actual_multisampled
                && same(*required_component, *actual_component, arguments, seen)
        }
        (
            BinaryType::VisibleFunctionTable {
                function_type: required,
            },
            BinaryType::VisibleFunctionTable {
                function_type: actual,
            },
        ) => same(*required, *actual, arguments, seen),
        (
            BinaryType::Tensor {
                rank: required_rank,
                element: required,
            },
            BinaryType::Tensor {
                rank: actual_rank,
                element: actual,
            },
        ) => required_rank == actual_rank && same(*required, *actual, arguments, seen),
        (
            BinaryType::CooperativeMatrix {
                scope: required_scope,
                rows: required_rows,
                columns: required_columns,
                element: required_element,
                use_kind: required_use,
            },
            BinaryType::CooperativeMatrix {
                scope: actual_scope,
                rows: actual_rows,
                columns: actual_columns,
                element: actual_element,
                use_kind: actual_use,
            },
        ) => {
            required_scope == actual_scope
                && required_rows == actual_rows
                && required_columns == actual_columns
                && required_use == actual_use
                && same(*required_element, *actual_element, arguments, seen)
        }
        (
            BinaryType::Entry {
                kind: required_kind,
                root: required_root,
                immediate: required_immediate,
            },
            BinaryType::Entry {
                kind: actual_kind,
                root: actual_root,
                immediate: actual_immediate,
            },
        ) => {
            required_kind == actual_kind
                && same(*required_root, *actual_root, arguments, seen)
                && same(*required_immediate, *actual_immediate, arguments, seen)
        }
        _ => false,
    }
}

fn infer_type_lists(
    types: &BinaryTypeTable,
    required: &[u64],
    actual: &[u64],
    arguments: &mut [Option<u64>],
    seen: &mut HashSet<(u64, u64)>,
) -> bool {
    required.len() == actual.len()
        && required.iter().zip(actual).all(|(required, actual)| {
            infer_type_indices(types, *required, *actual, arguments, seen)
        })
}

fn infer_function_types(
    types: &BinaryTypeTable,
    required: &BinaryFunctionType,
    actual: &BinaryFunctionType,
    arguments: &mut [Option<u64>],
    seen: &mut HashSet<(u64, u64)>,
) -> bool {
    required.parameters.len() == actual.parameters.len()
        && required.asynchronous == actual.asynchronous
        && required.c_abi == actual.c_abi
        && required.foreign == actual.foreign
        && required
            .parameters
            .iter()
            .zip(&actual.parameters)
            .all(|(required, actual)| {
                required.convention == actual.convention
                    && required.context == actual.context
                    && infer_type_indices(types, required.ty, actual.ty, arguments, seen)
            })
        && infer_type_indices(types, required.result, actual.result, arguments, seen)
        && match (required.error, actual.error) {
            (Some(required), Some(actual)) => {
                infer_type_indices(types, required, actual, arguments, seen)
            }
            (None, None) => true,
            _ => false,
        }
}
