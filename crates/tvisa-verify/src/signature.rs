use std::collections::HashSet;

use tir_diag::{Area, Code, Diagnostic, Dialect, Span};
use tir_types::{FloatType, Type};
use tvisa_format::{
    CAggregateDescriptor, ParameterItem, ResultItem, TextCallingConvention, TextImage,
    TextSignature,
};

const VFY6: Code = Code::new(Dialect::Tvisa, Area::Verifier, 6);
const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify_image_signatures(image: &TextImage) -> Vec<Diagnostic> {
    image.signatures.iter().flat_map(verify_signature).collect()
}

fn verify_signature(signature: &TextSignature) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if signature.convention == TextCallingConvention::Rtcc
        && (signature
            .parameters
            .iter()
            .any(parameter_has_non_integer_class)
            || signature.results.iter().any(result_has_non_integer_class))
    {
        diagnostics.push(
            Diagnostic::error(
                VFY6,
                "rtcc parameters and results must use integer-class values",
            )
            .with_primary_span(signature.span),
        );
    }
    for parameter in &signature.parameters {
        match parameter {
            ParameterItem::Leaf(ty) => verify_signature_type(ty, signature, &mut diagnostics),
            ParameterItem::Group(group) => {
                for member in &group.members {
                    verify_signature_type(&member.ty, signature, &mut diagnostics);
                }
            }
            ParameterItem::ByRef { flags, .. } => verify_flags(
                flags,
                &["owned", "readonly"],
                "byref",
                signature,
                &mut diagnostics,
            ),
            ParameterItem::CAggregate(aggregate) => {
                verify_c_aggregate(aggregate, signature, &mut diagnostics)
            }
            ParameterItem::Sret { .. } | ParameterItem::Context => {}
        }
    }
    for result in &signature.results {
        match result {
            ResultItem::Leaf(ty) => verify_signature_type(ty, signature, &mut diagnostics),
            ResultItem::Group(group) => {
                for member in &group.members {
                    verify_signature_type(&member.ty, signature, &mut diagnostics);
                }
            }
        }
    }
    diagnostics
}

fn verify_signature_type(ty: &Type, signature: &TextSignature, diagnostics: &mut Vec<Diagnostic>) {
    if !is_tvisa_value_type(ty) {
        diagnostics.push(
            Diagnostic::error(VFY6, format!("{ty} is not a TVISA signature value type"))
                .with_primary_span(signature.span),
        );
        return;
    }
    let gpu = matches!(
        signature.convention,
        TextCallingConvention::Gpu | TextCallingConvention::Entry
    );
    if !gpu && is_gpu_only_type(ty) {
        diagnostics.push(
            Diagnostic::error(VFY6, format!("{ty} is only valid in GPU signatures"))
                .with_primary_span(signature.span),
        );
    }
}

fn is_tvisa_value_type(ty: &Type) -> bool {
    if ty.validate_tvisa().is_err() {
        return false;
    }
    match ty {
        Type::Integer(bits) => matches!(*bits, 1 | 8 | 16 | 32 | 64 | 128),
        Type::Float(kind) => !is_narrow_float(*kind),
        Type::Pointer(_) | Type::Code => true,
        Type::FixedVector { element, .. } => {
            is_vector_element(element)
                && crate::conversion_types::static_size(ty).is_some_and(|size| size <= 512)
        }
        Type::ScalableVector(element) => is_vector_element(element),
        Type::Texture { .. }
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::VisibleFunctionTable(_)
        | Type::Tensor { .. }
        | Type::RayQuery
        | Type::HitObject
        | Type::CooperativeMatrix { .. } => true,
        Type::Unit
        | Type::Never
        | Type::Tuple(_)
        | Type::Array { .. }
        | Type::Nominal { .. }
        | Type::Generic(_)
        | Type::Reference(_)
        | Type::Unique(_)
        | Type::Weak(_)
        | Type::Address { .. }
        | Type::Function(_)
        | Type::Closure(_)
        | Type::Coroutine { .. }
        | Type::Frame(_)
        | Type::Entry { .. } => false,
    }
}

pub(super) fn verify_body_value_type(ty: &Type, gpu: bool, span: Span) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    if !is_tvisa_value_type(ty) {
        diagnostics.push(
            Diagnostic::error(VFY3, format!("{ty} is not a TVISA value type"))
                .with_primary_span(span),
        );
    } else if gpu {
        diagnostics.extend(crate::gpu::verify_value_type(ty, span));
    } else if is_gpu_only_type(ty) {
        diagnostics.push(
            Diagnostic::error(VFY3, format!("{ty} is only valid in GPU code"))
                .with_primary_span(span),
        );
    }
    diagnostics
}

fn is_narrow_float(kind: FloatType) -> bool {
    matches!(
        kind,
        FloatType::F8E4M3
            | FloatType::F8E5M2
            | FloatType::F6E2M3
            | FloatType::F6E3M2
            | FloatType::F4E2M1
            | FloatType::E8M0
    )
}

fn is_gpu_only_type(ty: &Type) -> bool {
    match ty {
        Type::Texture { .. }
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::VisibleFunctionTable(_)
        | Type::Tensor { .. }
        | Type::RayQuery
        | Type::HitObject
        | Type::CooperativeMatrix { .. } => true,
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            is_gpu_only_type(element)
        }
        Type::Pointer(
            tir_types::AddressSpace::Thread
            | tir_types::AddressSpace::Threadgroup
            | tir_types::AddressSpace::ObjectPayload
            | tir_types::AddressSpace::RayPayload
            | tir_types::AddressSpace::HitAttribute
            | tir_types::AddressSpace::Tile
            | tir_types::AddressSpace::NodeRecord,
        ) => true,
        _ => false,
    }
}

fn is_vector_element(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => matches!(*bits, 1 | 8 | 16 | 32 | 64 | 128),
        Type::Float(_) | Type::Pointer(_) => true,
        _ => false,
    }
}

fn verify_c_aggregate(
    aggregate: &CAggregateDescriptor,
    signature: &TextSignature,
    diagnostics: &mut Vec<Diagnostic>,
) {
    verify_flags(
        &aggregate.flags,
        &["union", "unaligned", "nontrivial", "flexible", "empty"],
        "cagg",
        signature,
        diagnostics,
    );
    for member in &aggregate.members {
        if !is_tvisa_value_type(&member.ty) {
            diagnostics.push(
                Diagnostic::error(
                    VFY6,
                    format!("{} is not a TVISA cagg member type", member.ty),
                )
                .with_primary_span(signature.span),
            );
            continue;
        }
        if !matches!(
            member.ty,
            Type::Integer(_) | Type::Float(_) | Type::Pointer(_)
        ) {
            diagnostics.push(
                Diagnostic::error(VFY6, "cagg members must be scalar C value types")
                    .with_primary_span(signature.span),
            );
        }
    }
}

fn verify_flags(
    flags: &[String],
    allowed: &[&str],
    item: &str,
    signature: &TextSignature,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen = HashSet::new();
    for flag in flags {
        if !seen.insert(flag) {
            diagnostics.push(
                Diagnostic::error(VFY6, format!("duplicate {item} flag {flag}"))
                    .with_primary_span(signature.span),
            );
        } else if !allowed.contains(&flag.as_str()) {
            diagnostics.push(
                Diagnostic::error(VFY6, format!("unknown {item} flag {flag}"))
                    .with_primary_span(signature.span),
            );
        }
    }
}

fn parameter_has_non_integer_class(parameter: &ParameterItem) -> bool {
    match parameter {
        ParameterItem::Leaf(ty) => has_non_integer_class(ty),
        ParameterItem::Group(group) => group
            .members
            .iter()
            .any(|member| has_non_integer_class(&member.ty)),
        ParameterItem::CAggregate(aggregate) => aggregate
            .members
            .iter()
            .any(|member| has_non_integer_class(&member.ty)),
        ParameterItem::ByRef { .. } | ParameterItem::Sret { .. } | ParameterItem::Context => false,
    }
}

fn result_has_non_integer_class(result: &ResultItem) -> bool {
    match result {
        ResultItem::Leaf(ty) => has_non_integer_class(ty),
        ResultItem::Group(group) => group
            .members
            .iter()
            .any(|member| has_non_integer_class(&member.ty)),
    }
}

fn has_non_integer_class(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Float(_)
            | Type::FixedVector { .. }
            | Type::ScalableVector(_)
            | Type::RayQuery
            | Type::HitObject
            | Type::CooperativeMatrix { .. }
    )
}
