use tir_types::{FloatType, Type};

pub(super) fn is_integer(ty: &Type) -> bool {
    match ty {
        Type::Integer(_) => true,
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            matches!(element.as_ref(), Type::Integer(_))
        }
        _ => false,
    }
}

pub(super) fn is_float(ty: &Type) -> bool {
    match ty {
        Type::Float(kind) => !is_narrow_float(*kind),
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => {
            matches!(element.as_ref(), Type::Float(_))
        }
        _ => false,
    }
}

pub(super) fn integer_element_width(ty: &Type) -> Option<u16> {
    match ty {
        Type::Integer(bits) => Some(*bits),
        Type::FixedVector { element, .. } | Type::ScalableVector(element) => match element.as_ref()
        {
            Type::Integer(bits) => Some(*bits),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn is_pointer(ty: &Type) -> bool {
    matches!(ty, Type::Pointer(_))
}

pub(super) fn is_memory_type(ty: &Type) -> bool {
    match ty {
        Type::Integer(1)
        | Type::Code
        | Type::Unit
        | Type::Never
        | Type::ScalableVector(_)
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
        | Type::RayQuery
        | Type::HitObject
        | Type::CooperativeMatrix { .. }
        | Type::Entry { .. } => false,
        _ => true,
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
