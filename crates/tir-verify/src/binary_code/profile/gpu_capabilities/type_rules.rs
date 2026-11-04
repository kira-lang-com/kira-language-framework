use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{BinaryCopyPolicy, BinaryDestroyPolicy, BinaryType, BinaryTypeTable};
use tir_types::FloatType;

const V15: Code = Code::new(Dialect::Tir, Area::Verifier, 15);

pub(super) fn verify(
    types: &BinaryTypeTable,
    type_index: u64,
    entry_name: &str,
    checked: &mut HashSet<u64>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    verify_inner(types, type_index, entry_name, checked, diagnostics);
}

fn verify_inner(
    types: &BinaryTypeTable,
    type_index: u64,
    entry_name: &str,
    checked: &mut HashSet<u64>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !checked.insert(type_index) {
        return;
    }
    let Some(ty) = usize::try_from(type_index)
        .ok()
        .and_then(|index| types.entries.get(index))
    else {
        return;
    };
    let violation = match ty {
        BinaryType::Integer(bits) if *bits > 64 => Some("integer wider than 64 bits"),
        BinaryType::Float(FloatType::F128) => Some("f128"),
        BinaryType::FixedVector { lanes, .. } if !matches!(*lanes, 2..=4) => {
            Some("fixed vector outside the 2-to-4 lane range")
        }
        BinaryType::ScalableVector { .. } => Some("scalable vector"),
        BinaryType::Reference { .. } => Some("ref type"),
        BinaryType::Unique { .. } => Some("uniq type"),
        BinaryType::Weak { .. } => Some("weak type"),
        BinaryType::Closure { .. } => Some("closure type"),
        BinaryType::Coroutine { .. } => Some("coroutine type"),
        BinaryType::Generic { .. } => Some("unspecialized generic type"),
        BinaryType::Nominal { declaration, .. } => Some(
            types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)
                .map_or("nominal type without a verified policy", |definition| {
                    if !matches!(definition.policy.copy, BinaryCopyPolicy::Trivial)
                        || !matches!(definition.policy.destroy, BinaryDestroyPolicy::Trivial)
                    {
                        "non-trivial nominal type policy"
                    } else {
                        ""
                    }
                }),
        )
        .filter(|violation| !violation.is_empty()),
        _ => None,
    };
    if let Some(violation) = violation {
        diagnostics.push(Diagnostic::error(
            V15,
            format!("GPU entry @{entry_name} uses prohibited {violation}"),
        ));
    }
    match ty {
        BinaryType::Address { pointee, .. }
        | BinaryType::FixedVector {
            element: pointee, ..
        }
        | BinaryType::ScalableVector { element: pointee }
        | BinaryType::Array {
            element: pointee, ..
        }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee }
        | BinaryType::Texture {
            component: pointee, ..
        }
        | BinaryType::Tensor {
            element: pointee, ..
        }
        | BinaryType::CooperativeMatrix {
            element: pointee, ..
        } => verify_inner(types, *pointee, entry_name, checked, diagnostics),
        BinaryType::Tuple(elements) => {
            for element in elements {
                verify_inner(types, *element, entry_name, checked, diagnostics);
            }
        }
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            for argument in arguments {
                verify_inner(types, *argument, entry_name, checked, diagnostics);
            }
            if let Some(definition) = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)
            {
                for field_type in definition.fields.iter().map(|field| field.ty).chain(
                    definition
                        .variants
                        .iter()
                        .flat_map(|variant| variant.payload.iter().copied()),
                ) {
                    if let Some(field_type) = super::super::super::types::specialize_nominal_type(
                        types, field_type, arguments,
                    ) {
                        verify_inner(types, field_type, entry_name, checked, diagnostics);
                    }
                }
            }
        }
        BinaryType::Function(function) => {
            for parameter in &function.parameters {
                verify_inner(types, parameter.ty, entry_name, checked, diagnostics);
            }
            verify_inner(types, function.result, entry_name, checked, diagnostics);
            if let Some(error) = function.error {
                verify_inner(types, error, entry_name, checked, diagnostics);
            }
        }
        BinaryType::Closure { function_type, .. }
        | BinaryType::VisibleFunctionTable { function_type } => {
            verify_inner(types, *function_type, entry_name, checked, diagnostics);
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            verify_inner(types, *yield_type, entry_name, checked, diagnostics);
            verify_inner(types, *resume_type, entry_name, checked, diagnostics);
            verify_inner(types, *result_type, entry_name, checked, diagnostics);
        }
        BinaryType::Entry {
            root, immediate, ..
        } => {
            verify_inner(types, *root, entry_name, checked, diagnostics);
            verify_inner(types, *immediate, entry_name, checked, diagnostics);
        }
        _ => {}
    }
}
