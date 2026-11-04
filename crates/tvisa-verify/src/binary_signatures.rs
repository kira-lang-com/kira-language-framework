use tir_diag::{Area, Code, Diagnostic, Dialect};
use tvisa_format::{
    CAggregateSummary, GroupSummary, ParameterItemSummary, ResultItemSummary, TvisaValueTypeRecord,
    TypeSection,
};

const VFY6: Code = Code::new(Dialect::Tvisa, Area::Verifier, 6);
const VFY21: Code = Code::new(Dialect::Tvisa, Area::Verifier, 21);

pub(super) fn verify_types(types: &TypeSection) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (index, signature) in types.signatures.iter().enumerate() {
        let gpu = matches!(signature.convention, 3 | 4);
        if gpu && signature.flags & 3 != 0 {
            diagnostics.push(Diagnostic::error(
                VFY21,
                format!("GPU signature {index} cannot fail or be variadic"),
            ));
        }
        if signature.convention == 2
            && (signature
                .parameters
                .iter()
                .any(|item| parameter_has_non_integer_class(item, types))
                || signature
                    .results
                    .iter()
                    .any(|item| result_has_non_integer_class(item, types)))
        {
            diagnostics.push(Diagnostic::error(
                VFY6,
                format!(
                    "rtcc signature {index} must use integer-class parameter and result values"
                ),
            ));
        }
        for parameter in &signature.parameters {
            match parameter {
                ParameterItemSummary::Leaf { type_index } => verify_signature_type(
                    types,
                    *type_index,
                    gpu,
                    index,
                    "parameter",
                    &mut diagnostics,
                ),
                ParameterItemSummary::Group(group) => {
                    verify_group(types, group, gpu, index, "parameter", &mut diagnostics)
                }
                ParameterItemSummary::CAggregate {
                    index: aggregate_index,
                } => {
                    if let Some(aggregate) = types.c_aggregates.get(*aggregate_index as usize) {
                        verify_c_aggregate(types, aggregate, index, &mut diagnostics);
                    }
                }
                ParameterItemSummary::ByRef { .. }
                | ParameterItemSummary::SRet { .. }
                | ParameterItemSummary::Context => {}
            }
        }
        for result in &signature.results {
            match result {
                ResultItemSummary::Leaf { type_index } => verify_signature_type(
                    types,
                    *type_index,
                    gpu,
                    index,
                    "result",
                    &mut diagnostics,
                ),
                ResultItemSummary::Group(group) => {
                    verify_group(types, group, gpu, index, "result", &mut diagnostics)
                }
            }
        }
    }
    diagnostics
}

fn verify_group(
    types: &TypeSection,
    group: &GroupSummary,
    gpu: bool,
    signature_index: usize,
    role: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for member in &group.members {
        verify_signature_type(
            types,
            member.type_index,
            gpu,
            signature_index,
            role,
            diagnostics,
        );
    }
}

fn verify_signature_type(
    types: &TypeSection,
    type_index: u64,
    gpu: bool,
    signature_index: usize,
    role: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(ty) = types.value_types.get(type_index as usize) else {
        return;
    };
    if !is_signature_type(types, ty) {
        diagnostics.push(Diagnostic::error(
            VFY6,
            format!("{role} type {type_index} in signature {signature_index} is not a TVISA signature value type"),
        ));
        return;
    }
    if !gpu && contains_gpu_only_type(types, type_index) {
        diagnostics.push(Diagnostic::error(
            VFY6,
            format!("{role} type {type_index} in signature {signature_index} is only valid in GPU signatures"),
        ));
    }
    if gpu {
        verify_gpu_type(
            types,
            type_index,
            signature_index,
            &mut Vec::new(),
            diagnostics,
        );
    }
}

fn is_signature_type(types: &TypeSection, ty: &TvisaValueTypeRecord) -> bool {
    match ty {
        TvisaValueTypeRecord::Float { tag } => !is_narrow_float(*tag),
        TvisaValueTypeRecord::FixedVector { element_type, .. }
        | TvisaValueTypeRecord::ScalableVector { element_type } => types
            .value_types
            .get(*element_type as usize)
            .is_some_and(is_vector_element),
        TvisaValueTypeRecord::Integer { .. }
        | TvisaValueTypeRecord::Pointer { .. }
        | TvisaValueTypeRecord::Code
        | TvisaValueTypeRecord::Texture { .. }
        | TvisaValueTypeRecord::Sampler
        | TvisaValueTypeRecord::AccelerationStructure
        | TvisaValueTypeRecord::VisibleFunctionTable { .. }
        | TvisaValueTypeRecord::Tensor { .. }
        | TvisaValueTypeRecord::RayQuery
        | TvisaValueTypeRecord::HitObject
        | TvisaValueTypeRecord::CooperativeMatrix { .. } => true,
    }
}

fn is_vector_element(ty: &TvisaValueTypeRecord) -> bool {
    matches!(
        ty,
        TvisaValueTypeRecord::Integer { .. }
            | TvisaValueTypeRecord::Float { .. }
            | TvisaValueTypeRecord::Pointer { .. }
    )
}

fn is_narrow_float(tag: u8) -> bool {
    matches!(tag, 0x07..=0x0c)
}

fn contains_gpu_only_type(types: &TypeSection, type_index: u64) -> bool {
    let Some(ty) = types.value_types.get(type_index as usize) else {
        return false;
    };
    match ty {
        TvisaValueTypeRecord::Pointer { address_space } => *address_space >= 3,
        TvisaValueTypeRecord::FixedVector { element_type, .. }
        | TvisaValueTypeRecord::ScalableVector { element_type } => {
            contains_gpu_only_type(types, *element_type)
        }
        TvisaValueTypeRecord::Texture { .. }
        | TvisaValueTypeRecord::Sampler
        | TvisaValueTypeRecord::AccelerationStructure
        | TvisaValueTypeRecord::VisibleFunctionTable { .. }
        | TvisaValueTypeRecord::Tensor { .. }
        | TvisaValueTypeRecord::RayQuery
        | TvisaValueTypeRecord::HitObject
        | TvisaValueTypeRecord::CooperativeMatrix { .. } => true,
        TvisaValueTypeRecord::Integer { .. }
        | TvisaValueTypeRecord::Float { .. }
        | TvisaValueTypeRecord::Code => false,
    }
}

fn verify_gpu_type(
    types: &TypeSection,
    type_index: u64,
    signature_index: usize,
    visited_signatures: &mut Vec<u64>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(ty) = types.value_types.get(type_index as usize) else {
        return;
    };
    match ty {
        TvisaValueTypeRecord::Integer { bits } if *bits > 64 => gpu_error(
            diagnostics,
            signature_index,
            type_index,
            "GPU signatures cannot use integers wider than 64 bits",
        ),
        TvisaValueTypeRecord::Float { tag: 0x06 } => gpu_error(
            diagnostics,
            signature_index,
            type_index,
            "GPU signatures cannot use f128",
        ),
        TvisaValueTypeRecord::Pointer { address_space: 0 } => gpu_error(
            diagnostics,
            signature_index,
            type_index,
            "GPU signatures must use explicit pointer address spaces",
        ),
        TvisaValueTypeRecord::Code => gpu_error(
            diagnostics,
            signature_index,
            type_index,
            "GPU signatures cannot use CPU code pointers",
        ),
        TvisaValueTypeRecord::FixedVector {
            lanes,
            element_type,
        } => {
            if !matches!(*lanes, 2 | 3 | 4) {
                gpu_error(
                    diagnostics,
                    signature_index,
                    type_index,
                    "GPU fixed vectors must have 2, 3, or 4 lanes",
                );
            }
            verify_gpu_type(
                types,
                *element_type,
                signature_index,
                visited_signatures,
                diagnostics,
            );
        }
        TvisaValueTypeRecord::ScalableVector { element_type } => {
            gpu_error(
                diagnostics,
                signature_index,
                type_index,
                "GPU signatures cannot use scalable vectors",
            );
            verify_gpu_type(
                types,
                *element_type,
                signature_index,
                visited_signatures,
                diagnostics,
            );
        }
        TvisaValueTypeRecord::Texture { component_type, .. }
        | TvisaValueTypeRecord::Tensor {
            element_type: component_type,
            ..
        }
        | TvisaValueTypeRecord::CooperativeMatrix {
            element_type: component_type,
            ..
        } => verify_gpu_type(
            types,
            *component_type,
            signature_index,
            visited_signatures,
            diagnostics,
        ),
        TvisaValueTypeRecord::VisibleFunctionTable { signature } => {
            if !visited_signatures.contains(signature) {
                visited_signatures.push(*signature);
                if types
                    .signatures
                    .get(*signature as usize)
                    .is_some_and(|referenced| referenced.convention != 3)
                {
                    gpu_error(
                        diagnostics,
                        signature_index,
                        type_index,
                        "visible function tables must reference a GPU function signature",
                    );
                }
            }
        }
        TvisaValueTypeRecord::Integer { .. }
        | TvisaValueTypeRecord::Float { .. }
        | TvisaValueTypeRecord::Pointer { .. }
        | TvisaValueTypeRecord::Sampler
        | TvisaValueTypeRecord::AccelerationStructure
        | TvisaValueTypeRecord::RayQuery
        | TvisaValueTypeRecord::HitObject => {}
    }
}

fn verify_c_aggregate(
    types: &TypeSection,
    aggregate: &CAggregateSummary,
    signature_index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for member in &aggregate.members {
        let Some(ty) = types.value_types.get(member.type_index as usize) else {
            continue;
        };
        if !is_signature_type(types, ty)
            || matches!(ty, TvisaValueTypeRecord::Pointer { address_space } if *address_space >= 3)
        {
            diagnostics.push(Diagnostic::error(
                VFY6,
                format!("C aggregate member type {} in signature {signature_index} is not valid for a C signature", member.type_index),
            ));
            continue;
        }
        if !matches!(
            ty,
            TvisaValueTypeRecord::Integer { .. }
                | TvisaValueTypeRecord::Pointer { .. }
                | TvisaValueTypeRecord::Float { tag: 0x02..=0x06 }
        ) {
            diagnostics.push(Diagnostic::error(
                VFY6,
                format!("C aggregate member type {} in signature {signature_index} is not a scalar C value type", member.type_index),
            ));
        }
    }
}

fn parameter_has_non_integer_class(item: &ParameterItemSummary, types: &TypeSection) -> bool {
    match item {
        ParameterItemSummary::Leaf { type_index } => has_non_integer_class(types, *type_index),
        ParameterItemSummary::Group(group) => group
            .members
            .iter()
            .any(|member| has_non_integer_class(types, member.type_index)),
        ParameterItemSummary::CAggregate { index } => types
            .c_aggregates
            .get(*index as usize)
            .is_some_and(|aggregate| {
                aggregate
                    .members
                    .iter()
                    .any(|member| has_non_integer_class(types, member.type_index))
            }),
        ParameterItemSummary::ByRef { .. }
        | ParameterItemSummary::SRet { .. }
        | ParameterItemSummary::Context => false,
    }
}

fn result_has_non_integer_class(item: &ResultItemSummary, types: &TypeSection) -> bool {
    match item {
        ResultItemSummary::Leaf { type_index } => has_non_integer_class(types, *type_index),
        ResultItemSummary::Group(group) => group
            .members
            .iter()
            .any(|member| has_non_integer_class(types, member.type_index)),
    }
}

fn has_non_integer_class(types: &TypeSection, type_index: u64) -> bool {
    let Some(ty) = types.value_types.get(type_index as usize) else {
        return false;
    };
    matches!(
        ty,
        TvisaValueTypeRecord::Float { .. }
            | TvisaValueTypeRecord::FixedVector { .. }
            | TvisaValueTypeRecord::ScalableVector { .. }
            | TvisaValueTypeRecord::RayQuery
            | TvisaValueTypeRecord::HitObject
            | TvisaValueTypeRecord::CooperativeMatrix { .. }
    )
}

fn gpu_error(
    diagnostics: &mut Vec<Diagnostic>,
    signature_index: usize,
    type_index: u64,
    message: &str,
) {
    diagnostics.push(Diagnostic::error(
        VFY21,
        format!("{message} (type {type_index} in signature {signature_index})"),
    ));
}
