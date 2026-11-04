use std::fmt;

use crate::{Abi, AddressSpace, FloatType, FunctionType, Type};

const MAX_TYPE_DEPTH: usize = 256;
const MAX_ARRAY_COUNT: u64 = 1 << 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeError {
    InvalidIntegerWidth(u16),
    InvalidVectorLaneCount(u32),
    InvalidVectorElement,
    InvalidScalableElement,
    ArrayTooLarge(u64),
    ScalableVectorInAggregate,
    EmptyName,
    TooManyParameters(usize),
    ForeignFunctionMustUseCAbi,
    InvalidCooperativeMatrixShape,
    InvalidTensorRank(u32),
    InvalidTextureComponent,
    InvalidPackedFloatContext,
    DuplicateContextParameter,
    InvalidContextParameter,
    TvisaOnlyType,
    UnsubstitutedGeneric,
    UnqualifiedNominalPath,
    NestingTooDeep,
}

impl fmt::Display for TypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIntegerWidth(bits) => {
                write!(formatter, "integer width {bits} is outside 1..=16384")
            }
            Self::InvalidVectorLaneCount(lanes) => {
                write!(formatter, "vector lane count {lanes} is not permitted")
            }
            Self::InvalidVectorElement => formatter.write_str("type is not a fixed-vector element"),
            Self::InvalidScalableElement => {
                formatter.write_str("type has no integral scalable-vector lane count")
            }
            Self::ArrayTooLarge(count) => {
                write!(formatter, "array count {count} must be less than 2^48")
            }
            Self::ScalableVectorInAggregate => {
                formatter.write_str("scalable vectors cannot be aggregate fields")
            }
            Self::EmptyName => formatter.write_str("type names and paths must not be empty"),
            Self::TooManyParameters(count) => {
                write!(
                    formatter,
                    "function has {count} parameters; the limit is below 2^16"
                )
            }
            Self::ForeignFunctionMustUseCAbi => {
                formatter.write_str("foreign function types must use the C ABI")
            }
            Self::InvalidCooperativeMatrixShape => {
                formatter.write_str("cooperative matrix dimensions must be nonzero")
            }
            Self::InvalidTensorRank(rank) => {
                write!(
                    formatter,
                    "tensor rank {rank} is not one of 1, 2, 3, 4, or 8"
                )
            }
            Self::InvalidTextureComponent => {
                formatter.write_str("texture component must be f16, f32, i32, or vec<4 x T>")
            }
            Self::InvalidPackedFloatContext => formatter.write_str(
                "f6 and f4 types are only valid as vector, array, or cooperative-matrix elements",
            ),
            Self::DuplicateContextParameter => {
                formatter.write_str("function type has more than one context parameter")
            }
            Self::InvalidContextParameter => {
                formatter.write_str("context parameter must be a raw pointer in the RIDL ABI")
            }
            Self::TvisaOnlyType => formatter.write_str("type is only valid in TVISA"),
            Self::UnsubstitutedGeneric => formatter
                .write_str("type identifier requires all generic parameters to be substituted"),
            Self::UnqualifiedNominalPath => {
                formatter.write_str("type identifier requires module-qualified nominal names")
            }
            Self::NestingTooDeep => {
                write!(
                    formatter,
                    "type nesting exceeds the limit of {MAX_TYPE_DEPTH}"
                )
            }
        }
    }
}

impl std::error::Error for TypeError {}

pub(crate) fn validate_type(ty: &Type) -> Result<(), TypeError> {
    validate_type_at(ty, 0, false, false)
}

pub(crate) fn validate_tvisa_type(ty: &Type) -> Result<(), TypeError> {
    validate_type_at(ty, 0, false, true)
}

fn validate_type_at(
    ty: &Type,
    depth: usize,
    packed_float_allowed: bool,
    allow_tvisa_only: bool,
) -> Result<(), TypeError> {
    if depth >= MAX_TYPE_DEPTH {
        return Err(TypeError::NestingTooDeep);
    }
    match ty {
        Type::Integer(bits) if *bits == 0 || *bits > 16_384 => {
            Err(TypeError::InvalidIntegerWidth(*bits))
        }
        Type::Code if allow_tvisa_only => Ok(()),
        Type::Code => Err(TypeError::TvisaOnlyType),
        Type::Integer(_) | Type::Pointer(_) | Type::Unit | Type::Never => Ok(()),
        Type::Float(FloatType::F6E2M3 | FloatType::F6E3M2 | FloatType::F4E2M1)
            if !packed_float_allowed =>
        {
            Err(TypeError::InvalidPackedFloatContext)
        }
        Type::Float(_) => Ok(()),
        Type::FixedVector { lanes, element } => {
            if !matches!(*lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64) {
                return Err(TypeError::InvalidVectorLaneCount(*lanes));
            }
            validate_type_at(element, depth + 1, true, allow_tvisa_only)?;
            if !is_fixed_vector_element(element) {
                return Err(TypeError::InvalidVectorElement);
            }
            Ok(())
        }
        Type::ScalableVector(element) => {
            validate_type_at(element, depth + 1, true, allow_tvisa_only)?;
            let bits = element_bits(element).ok_or(TypeError::InvalidScalableElement)?;
            if bits == 0 || 128 % bits != 0 {
                return Err(TypeError::InvalidScalableElement);
            }
            Ok(())
        }
        Type::Tuple(elements) => {
            for element in elements {
                if matches!(element, Type::ScalableVector(_)) {
                    return Err(TypeError::ScalableVectorInAggregate);
                }
                validate_type_at(element, depth + 1, false, allow_tvisa_only)?;
            }
            Ok(())
        }
        Type::Array { count, element } => {
            if *count >= MAX_ARRAY_COUNT {
                return Err(TypeError::ArrayTooLarge(*count));
            }
            if matches!(element.as_ref(), Type::ScalableVector(_)) {
                return Err(TypeError::ScalableVectorInAggregate);
            }
            validate_type_at(element, depth + 1, true, allow_tvisa_only)
        }
        Type::Nominal { path, arguments } => {
            validate_path(path)?;
            validate_many(arguments, depth, allow_tvisa_only)
        }
        Type::Generic(name) => validate_name(name),
        Type::Reference(inner) | Type::Unique(inner) | Type::Weak(inner) => {
            validate_type_at(inner, depth + 1, false, allow_tvisa_only)
        }
        Type::Address { pointee, .. } => {
            validate_type_at(pointee, depth + 1, false, allow_tvisa_only)
        }
        Type::Function(signature) => validate_signature(signature, depth, allow_tvisa_only),
        Type::Closure(closure) => validate_signature(&closure.signature, depth, allow_tvisa_only),
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            validate_type_at(yield_type, depth + 1, false, allow_tvisa_only)?;
            validate_type_at(resume_type, depth + 1, false, allow_tvisa_only)?;
            validate_type_at(result_type, depth + 1, false, allow_tvisa_only)
        }
        Type::Frame(path) => validate_path(path),
        Type::Texture { component, .. } => {
            validate_type_at(component, depth + 1, false, allow_tvisa_only)?;
            if !is_texture_component(component) {
                return Err(TypeError::InvalidTextureComponent);
            }
            Ok(())
        }
        Type::Sampler | Type::AccelerationStructure | Type::RayQuery | Type::HitObject => Ok(()),
        Type::VisibleFunctionTable(signature) => {
            validate_signature(signature, depth, allow_tvisa_only)
        }
        Type::Tensor { count, element } => {
            if !matches!(*count, 1 | 2 | 3 | 4 | 8) {
                return Err(TypeError::InvalidTensorRank(*count));
            }
            validate_type_at(element, depth + 1, false, allow_tvisa_only)
        }
        Type::CooperativeMatrix {
            rows,
            columns,
            element,
            ..
        } => {
            if *rows == 0 || *columns == 0 {
                return Err(TypeError::InvalidCooperativeMatrixShape);
            }
            validate_type_at(element, depth + 1, true, allow_tvisa_only)
        }
        Type::Entry {
            root, immediate, ..
        } => {
            validate_type_at(root, depth + 1, false, allow_tvisa_only)?;
            validate_type_at(immediate, depth + 1, false, allow_tvisa_only)
        }
    }
}

fn validate_signature(
    signature: &FunctionType,
    depth: usize,
    allow_tvisa_only: bool,
) -> Result<(), TypeError> {
    if signature.parameters.len() >= 1 << 16 {
        return Err(TypeError::TooManyParameters(signature.parameters.len()));
    }
    if signature.foreign && signature.abi != Abi::C {
        return Err(TypeError::ForeignFunctionMustUseCAbi);
    }
    let mut has_context = false;
    for parameter in &signature.parameters {
        validate_type_at(&parameter.ty, depth + 1, false, allow_tvisa_only)?;
        if parameter.context {
            if has_context {
                return Err(TypeError::DuplicateContextParameter);
            }
            has_context = true;
            if signature.abi != Abi::Ridl || !matches!(parameter.ty, Type::Pointer(_)) {
                return Err(TypeError::InvalidContextParameter);
            }
        }
    }
    validate_type_at(&signature.result, depth + 1, false, allow_tvisa_only)?;
    if let Some(failure) = &signature.failure {
        validate_type_at(failure, depth + 1, false, allow_tvisa_only)?;
    }
    Ok(())
}

fn validate_many(types: &[Type], depth: usize, allow_tvisa_only: bool) -> Result<(), TypeError> {
    for ty in types {
        validate_type_at(ty, depth + 1, false, allow_tvisa_only)?;
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), TypeError> {
    if name.trim().is_empty() {
        Err(TypeError::EmptyName)
    } else {
        Ok(())
    }
}

fn validate_path(path: &str) -> Result<(), TypeError> {
    if path.split('.').any(str::is_empty) {
        Err(TypeError::EmptyName)
    } else {
        validate_name(path)
    }
}

fn is_fixed_vector_element(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => (1..=64).contains(bits),
        Type::Float(_) => true,
        Type::Pointer(AddressSpace::Generic) => true,
        _ => false,
    }
}

fn element_bits(ty: &Type) -> Option<u16> {
    match ty {
        Type::Integer(bits) if (1..=128).contains(bits) => Some(*bits),
        Type::Float(kind) => Some(kind.bits()),
        Type::Pointer(AddressSpace::Generic) => Some(64),
        _ => None,
    }
}

fn is_texture_component(ty: &Type) -> bool {
    match ty {
        Type::Integer(32) | Type::Float(FloatType::F16 | FloatType::F32) => true,
        Type::FixedVector { lanes: 4, element } => {
            matches!(
                element.as_ref(),
                Type::Integer(32) | Type::Float(FloatType::F16 | FloatType::F32)
            )
        }
        _ => false,
    }
}
