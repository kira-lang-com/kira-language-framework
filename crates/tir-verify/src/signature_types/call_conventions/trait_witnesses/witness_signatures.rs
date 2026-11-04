use std::collections::HashMap;

use tir_types::{FunctionType, Type};

pub(super) fn specialize(
    requirement: &FunctionType,
    witness: &FunctionType,
    parameters: &[String],
    substitutions: &mut HashMap<String, Type>,
) -> Option<FunctionType> {
    if !unify_function(requirement, witness, parameters, substitutions) {
        return None;
    }
    let Type::Function(specialized) =
        Type::Function(requirement.clone()).substitute(&substitutions)
    else {
        return None;
    };
    (specialized == *witness).then_some(specialized)
}

fn unify_function(
    requirement: &FunctionType,
    witness: &FunctionType,
    parameters: &[String],
    substitutions: &mut HashMap<String, Type>,
) -> bool {
    requirement.asynchronous == witness.asynchronous
        && requirement.abi == witness.abi
        && requirement.foreign == witness.foreign
        && requirement.parameters.len() == witness.parameters.len()
        && requirement
            .parameters
            .iter()
            .zip(&witness.parameters)
            .all(|(required, actual)| {
                required.context == actual.context
                    && required.convention == actual.convention
                    && unify_type(&required.ty, &actual.ty, parameters, substitutions)
            })
        && unify_type(
            &requirement.result,
            &witness.result,
            parameters,
            substitutions,
        )
        && unify_optional_type(
            requirement.failure.as_deref(),
            witness.failure.as_deref(),
            parameters,
            substitutions,
        )
}

fn unify_optional_type(
    requirement: Option<&Type>,
    witness: Option<&Type>,
    parameters: &[String],
    substitutions: &mut HashMap<String, Type>,
) -> bool {
    match (requirement, witness) {
        (Some(required), Some(actual)) => unify_type(required, actual, parameters, substitutions),
        (None, None) => true,
        _ => false,
    }
}

fn unify_type(
    requirement: &Type,
    witness: &Type,
    parameters: &[String],
    substitutions: &mut HashMap<String, Type>,
) -> bool {
    if let Type::Generic(name) = requirement
        && parameters.iter().any(|parameter| parameter == name)
    {
        if let Some(previous) = substitutions.get(name) {
            return previous == witness;
        }
        substitutions.insert(name.clone(), witness.clone());
        return true;
    }
    match (requirement, witness) {
        (
            Type::FixedVector {
                lanes: required_lanes,
                element: required_element,
            },
            Type::FixedVector {
                lanes: actual_lanes,
                element: actual_element,
            },
        ) => {
            required_lanes == actual_lanes
                && unify_type(required_element, actual_element, parameters, substitutions)
        }
        (Type::ScalableVector(required), Type::ScalableVector(actual))
        | (Type::Reference(required), Type::Reference(actual))
        | (Type::Unique(required), Type::Unique(actual))
        | (Type::Weak(required), Type::Weak(actual))
        | (
            Type::Tensor {
                element: required, ..
            },
            Type::Tensor {
                element: actual, ..
            },
        ) => {
            unify_type(required, actual, parameters, substitutions)
                && match (requirement, witness) {
                    (Type::Tensor { count: left, .. }, Type::Tensor { count: right, .. }) => {
                        left == right
                    }
                    _ => true,
                }
        }
        (Type::Tuple(required), Type::Tuple(actual)) => {
            unify_many(required, actual, parameters, substitutions)
        }
        (
            Type::Array {
                count: required_count,
                element: required_element,
            },
            Type::Array {
                count: actual_count,
                element: actual_element,
            },
        ) => {
            required_count == actual_count
                && unify_type(required_element, actual_element, parameters, substitutions)
        }
        (
            Type::Nominal {
                path: required_path,
                arguments: required_arguments,
            },
            Type::Nominal {
                path: actual_path,
                arguments: actual_arguments,
            },
        ) => {
            required_path == actual_path
                && unify_many(
                    required_arguments,
                    actual_arguments,
                    parameters,
                    substitutions,
                )
        }
        (
            Type::Address {
                pointee: required,
                space: required_space,
            },
            Type::Address {
                pointee: actual,
                space: actual_space,
            },
        ) => {
            required_space == actual_space
                && unify_type(required, actual, parameters, substitutions)
        }
        (Type::Function(required), Type::Function(actual)) => {
            unify_function(required, actual, parameters, substitutions)
        }
        (Type::Closure(required), Type::Closure(actual)) => {
            required.borrowed == actual.borrowed
                && unify_function(
                    &required.signature,
                    &actual.signature,
                    parameters,
                    substitutions,
                )
        }
        (
            Type::Coroutine {
                yield_type: required_yield,
                resume_type: required_resume,
                result_type: required_result,
            },
            Type::Coroutine {
                yield_type: actual_yield,
                resume_type: actual_resume,
                result_type: actual_result,
            },
        ) => {
            unify_type(required_yield, actual_yield, parameters, substitutions)
                && unify_type(required_resume, actual_resume, parameters, substitutions)
                && unify_type(required_result, actual_result, parameters, substitutions)
        }
        (
            Type::Texture {
                dimension: required_dimension,
                component: required_component,
                access: required_access,
                multisampled: required_multisampled,
            },
            Type::Texture {
                dimension: actual_dimension,
                component: actual_component,
                access: actual_access,
                multisampled: actual_multisampled,
            },
        ) => {
            required_dimension == actual_dimension
                && required_access == actual_access
                && required_multisampled == actual_multisampled
                && unify_type(
                    required_component,
                    actual_component,
                    parameters,
                    substitutions,
                )
        }
        (Type::VisibleFunctionTable(required), Type::VisibleFunctionTable(actual)) => {
            unify_function(required, actual, parameters, substitutions)
        }
        (
            Type::CooperativeMatrix {
                scope: required_scope,
                rows: required_rows,
                columns: required_columns,
                element: required_element,
                use_kind: required_use,
            },
            Type::CooperativeMatrix {
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
                && unify_type(required_element, actual_element, parameters, substitutions)
        }
        (
            Type::Entry {
                kind: required_kind,
                root: required_root,
                immediate: required_immediate,
            },
            Type::Entry {
                kind: actual_kind,
                root: actual_root,
                immediate: actual_immediate,
            },
        ) => {
            required_kind == actual_kind
                && unify_type(required_root, actual_root, parameters, substitutions)
                && unify_type(
                    required_immediate,
                    actual_immediate,
                    parameters,
                    substitutions,
                )
        }
        _ => requirement == witness,
    }
}

fn unify_many(
    requirement: &[Type],
    witness: &[Type],
    parameters: &[String],
    substitutions: &mut HashMap<String, Type>,
) -> bool {
    requirement.len() == witness.len()
        && requirement
            .iter()
            .zip(witness)
            .all(|(required, actual)| unify_type(required, actual, parameters, substitutions))
}
