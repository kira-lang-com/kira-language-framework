use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::TextInstruction;
use tir_layout::LayoutEngine;
use tir_types::{AddressSpace, FloatType, Type};

use crate::type_classes::{is_float, is_integer};

const VFY3: Code = Code::new(Dialect::Tvisa, Area::Verifier, 3);

pub(super) fn verify(
    source: &str,
    instruction: &TextInstruction,
    operands: &[Type],
) -> Vec<Diagnostic> {
    let Some(actual) = operands.first() else {
        return Vec::new();
    };
    let Some(target) = crate::instruction_types::explicit_type(source, instruction) else {
        return Vec::new();
    };
    let valid = match instruction.opcode.as_str() {
        "trunc" => integer_conversion(actual, &target, |source, target| source > target),
        "zext" | "sext" => integer_conversion(actual, &target, |source, target| source < target),
        "fptrunc" => float_conversion(actual, &target, |source, target| source > target),
        "fpext" => float_conversion(actual, &target, |source, target| source < target),
        "fcvt" => is_float(actual) && is_float(&target) && same_shape(actual, &target),
        "sitofp" | "uitofp" => {
            is_integer(actual) && is_float(&target) && same_shape(actual, &target)
        }
        "fptosi" | "fptoui" => {
            is_float(actual) && is_integer(&target) && same_shape(actual, &target)
        }
        "ptrtoint" => matches!(actual, Type::Pointer(_)) && target == Type::Integer(64),
        "inttoptr" => {
            actual == &Type::Integer(64)
                && matches!(
                    target,
                    Type::Pointer(
                        AddressSpace::Generic | AddressSpace::Device | AddressSpace::Constant
                    )
                )
        }
        "bitcast" => bitcast_is_valid(actual, &target),
        _ => true,
    };
    if valid {
        Vec::new()
    } else {
        vec![
            Diagnostic::error(
                VFY3,
                format!(
                    "{} cannot convert {} to {}",
                    instruction.opcode, actual, target
                ),
            )
            .with_primary_span(instruction.span),
        ]
    }
}

fn integer_conversion(source: &Type, target: &Type, width_rule: fn(u16, u16) -> bool) -> bool {
    if !is_integer(source) || !is_integer(target) || !same_shape(source, target) {
        return false;
    }
    element_width(source)
        .zip(element_width(target))
        .is_some_and(|(source, target)| width_rule(source, target))
}

fn float_conversion(source: &Type, target: &Type, width_rule: fn(u16, u16) -> bool) -> bool {
    if !is_float(source) || !is_float(target) || !same_shape(source, target) {
        return false;
    }
    element_width(source)
        .zip(element_width(target))
        .is_some_and(|(source, target)| width_rule(source, target))
}

fn element_width(ty: &Type) -> Option<u16> {
    match vector_element(ty) {
        Type::Integer(bits) => Some(*bits),
        Type::Float(kind) => Some(kind.bits()),
        _ => None,
    }
}

fn vector_element(ty: &Type) -> &Type {
    match ty {
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => element,
        _ => ty,
    }
}

fn same_shape(left: &Type, right: &Type) -> bool {
    match (left, right) {
        (Type::FixedVector { lanes: left, .. }, Type::FixedVector { lanes: right, .. }) => {
            left == right
        }
        (Type::ScalableVector(_), Type::ScalableVector(_)) => true,
        (Type::FixedVector { .. } | Type::ScalableVector(_), _)
        | (_, Type::FixedVector { .. } | Type::ScalableVector(_)) => false,
        _ => true,
    }
}

fn bitcast_is_valid(source: &Type, target: &Type) -> bool {
    if !is_trivial_value(source) || !is_trivial_value(target) {
        return false;
    }
    if matches!(source, Type::ScalableVector(_)) || matches!(target, Type::ScalableVector(_)) {
        return matches!(source, Type::ScalableVector(_))
            && matches!(target, Type::ScalableVector(_));
    }
    static_size(source)
        .zip(static_size(target))
        .is_some_and(|(source, target)| source == target)
}

pub(super) fn static_size(ty: &Type) -> Option<u64> {
    LayoutEngine::new()
        .layout(ty)
        .ok()
        .map(|layout| layout.size)
}

fn is_trivial_value(ty: &Type) -> bool {
    match ty {
        Type::Integer(_)
        | Type::Pointer(_)
        | Type::FixedVector { .. }
        | Type::ScalableVector(_)
        | Type::Texture { .. }
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::VisibleFunctionTable(_)
        | Type::Tensor { .. }
        | Type::Entry { .. } => true,
        Type::Float(kind) => !is_narrow_float(*kind),
        _ => false,
    }
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
