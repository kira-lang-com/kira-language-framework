//! The operand rules of every expression kind (sections 11.1, 11.4, 11.5),
//! shared by the builder's eager checks and validation rule SV6.

use crate::arena::Handle;
use crate::ir::{BinaryOp, Expression, MathFunction, RelationalFunction, UnaryOp};
use crate::module::Module;
use crate::resolve::Resolution;
use crate::types::{ArraySize, ImageClass, ScalarKind, TypeInner};

/// Checks `expression`'s operands, given the types of every earlier
/// expression of its arena.
///
/// # Errors
/// Returns which operand rule the expression breaks.
pub fn check_expression(
    module: &Module,
    resolved: &[Resolution],
    expression: &Expression,
) -> Result<(), String> {
    let types = &module.types;
    let of = |handle: Handle<Expression>| -> Result<&TypeInner, String> {
        resolved
            .get(handle.index())
            .map(|resolution| resolution.inner(types))
            .ok_or_else(|| format!("operand {handle:?} does not precede its use"))
    };
    match expression {
        Expression::Compose { ty, components } => compose(module, resolved, *ty, components),
        Expression::Access { base, index } => {
            match of(*index)? {
                TypeInner::Scalar(scalar) if scalar.kind.is_integer() && scalar.width == 4 => {}
                _ => return Err("an index is an i32 or u32".to_owned()),
            }
            indexable(module, of(*base)?)
        }
        Expression::AccessIndex { base, index } => {
            let inner = of(*base)?;
            let pointee = match inner {
                TypeInner::Pointer { base, .. } => &types[*base].inner,
                other => other,
            };
            let count = match pointee {
                TypeInner::Vector { size, .. } => Some(u32::from(*size)),
                TypeInner::Matrix { columns, .. } => Some(u32::from(*columns)),
                TypeInner::Array {
                    size: ArraySize::Constant(count),
                    ..
                } => Some(*count),
                TypeInner::Struct { members, .. } => Some(members.len() as u32),
                TypeInner::Array { .. } => None,
                _ => return Err("indexing a value that has no elements".to_owned()),
            };
            match count {
                Some(count) if *index >= count => Err(format!("index {index} is out of range")),
                _ => Ok(()),
            }
        }
        Expression::Splat { size, value } => match of(*value)? {
            TypeInner::Scalar(_) if (2..=4).contains(size) => Ok(()),
            _ => Err("a splat fills a 2- to 4-wide vector from a scalar".to_owned()),
        },
        Expression::Swizzle { vector, pattern } => match of(*vector)? {
            TypeInner::Vector { size, .. }
                if (1..=4).contains(&pattern.len())
                    && pattern.iter().all(|component| component < size) =>
            {
                Ok(())
            }
            _ => Err("a swizzle picks 1 to 4 components the vector has".to_owned()),
        },
        Expression::Load { pointer } => match of(*pointer)? {
            TypeInner::Pointer { space, .. } if space.loads() => Ok(()),
            TypeInner::Pointer { .. } => {
                Err("a load through a pointer that cannot be read".to_owned())
            }
            _ => Err("a load through a value that is not a pointer".to_owned()),
        },
        Expression::Unary { op, expr } => {
            let (_, scalar) = of(*expr)?
                .scalar_or_vector()
                .ok_or("a unary operator on a value that is not a scalar or vector")?;
            let fits = match op {
                UnaryOp::Negate => matches!(scalar.kind, ScalarKind::Sint | ScalarKind::Float),
                UnaryOp::LogicalNot => scalar.kind == ScalarKind::Bool,
                UnaryOp::BitwiseNot => scalar.kind.is_integer(),
            };
            fits.then_some(())
                .ok_or_else(|| format!("`{}` does not apply to {}", op.name(), scalar.kind.name()))
        }
        Expression::Binary { op, left, right } => binary(*op, of(*left)?, of(*right)?),
        Expression::Select {
            condition,
            accept,
            reject,
        } => {
            let accept_type = &resolved[accept.index()];
            if !accept_type.same(&resolved[reject.index()], types) {
                return Err("both values of a select have one type".to_owned());
            }
            match (of(*condition)?, accept_type.inner(types)) {
                (TypeInner::Scalar(scalar), _) if scalar.kind == ScalarKind::Bool => Ok(()),
                (TypeInner::Vector { size, scalar }, TypeInner::Vector { size: wanted, .. })
                    if scalar.kind == ScalarKind::Bool && size == wanted =>
                {
                    Ok(())
                }
                _ => Err(
                    "a select's condition is a bool or a bool vector of the value's size"
                        .to_owned(),
                ),
            }
        }
        Expression::Derivative { expr, .. } => float_operand(of(*expr)?),
        Expression::Relational { fun, argument } => {
            let (size, scalar) = of(*argument)?
                .scalar_or_vector()
                .ok_or("a relational function of a value that is not a scalar or vector")?;
            let fits = match fun {
                RelationalFunction::All | RelationalFunction::Any => {
                    size.is_some() && scalar.kind == ScalarKind::Bool
                }
                RelationalFunction::IsNan | RelationalFunction::IsInf => {
                    scalar.kind == ScalarKind::Float
                }
            };
            fits.then_some(())
                .ok_or_else(|| format!("`{}` does not apply here", fun.name()))
        }
        Expression::Math { fun, args } => math(*fun, args, &of),
        Expression::Convert { expr, .. } => of(*expr)?
            .scalar_or_vector()
            .map(|_| ())
            .ok_or_else(|| "a conversion of a value that is not a scalar or vector".to_owned()),
        Expression::ImageSample {
            image,
            sampler,
            depth_ref,
            ..
        } => match (of(*image)?, of(*sampler)?) {
            (TypeInner::Image { class, .. }, TypeInner::Sampler { comparison }) => {
                if matches!(class, ImageClass::Storage { .. }) {
                    return Err("a storage image is not sampled".to_owned());
                }
                if *comparison != depth_ref.is_some() {
                    return Err(
                        "a comparison sampler goes with a depth reference, and only it".to_owned(),
                    );
                }
                Ok(())
            }
            _ => Err("a sample takes an image and a sampler".to_owned()),
        },
        Expression::ImageLoad { image, .. } | Expression::ImageQuery { image, .. } => {
            match of(*image)? {
                TypeInner::Image { .. } => Ok(()),
                _ => Err("an image operation on a value that is not an image".to_owned()),
            }
        }
        Expression::ArrayLength(pointer) => match of(*pointer)? {
            TypeInner::Pointer { base, .. }
                if matches!(
                    types[*base].inner,
                    TypeInner::Array {
                        size: ArraySize::Runtime,
                        ..
                    }
                ) =>
            {
                Ok(())
            }
            _ => Err("array_length takes a pointer to a runtime-sized array".to_owned()),
        },
        _ => Ok(()),
    }
}

pub(crate) fn check(
    module: &Module,
    resolved: &[Resolution],
    expression: &Expression,
    _resolution: &Resolution,
) -> Result<(), String> {
    check_expression(module, resolved, expression)
}

fn compose(
    module: &Module,
    resolved: &[Resolution],
    ty: crate::arena::Handle<crate::types::Type>,
    components: &[Handle<Expression>],
) -> Result<(), String> {
    let types = &module.types;
    let inner = |handle: Handle<Expression>| -> Result<&TypeInner, String> {
        resolved
            .get(handle.index())
            .map(|resolution| resolution.inner(types))
            .ok_or_else(|| "a component does not precede the compose".to_owned())
    };
    let same = |handle: Handle<Expression>, wanted| -> Result<bool, String> {
        Ok(resolved
            .get(handle.index())
            .ok_or("a component does not precede the compose")?
            .same(&Resolution::Handle(wanted), types))
    };
    match &types[ty].inner {
        TypeInner::Vector { size, scalar } => {
            let mut total = 0;
            for component in components {
                match inner(*component)? {
                    TypeInner::Scalar(part) if part == scalar => total += 1,
                    TypeInner::Vector { size, scalar: part } if part == scalar => total += size,
                    _ => {
                        return Err(
                            "a vector is composed of its scalar and vectors of it".to_owned()
                        );
                    }
                }
            }
            (total == *size)
                .then_some(())
                .ok_or_else(|| format!("{total} components compose a vector of {size}"))
        }
        TypeInner::Matrix {
            columns,
            rows,
            scalar,
        } => {
            let columns_fit = components.len() == usize::from(*columns)
                && components.iter().all(|component| {
                    matches!(inner(*component), Ok(TypeInner::Vector { size, scalar: part }) if size == rows && part == scalar)
                });
            columns_fit
                .then_some(())
                .ok_or_else(|| "a matrix is composed of its columns".to_owned())
        }
        TypeInner::Array {
            base,
            size: ArraySize::Constant(count),
            ..
        } => {
            if components.len() != *count as usize {
                return Err(format!(
                    "{} elements compose an array of {count}",
                    components.len()
                ));
            }
            for component in components {
                if !same(*component, *base)? {
                    return Err("an array element has the wrong type".to_owned());
                }
            }
            Ok(())
        }
        TypeInner::Struct { members, .. } => {
            if components.len() != members.len() {
                return Err(format!(
                    "{} values compose a struct of {} members",
                    components.len(),
                    members.len()
                ));
            }
            for (component, member) in components.iter().zip(members) {
                if !same(*component, member.ty)? {
                    return Err(format!("member `{}` has the wrong type", member.name));
                }
            }
            Ok(())
        }
        _ => Err("only vectors, matrices, fixed arrays and structs are composed".to_owned()),
    }
}

fn indexable(module: &Module, inner: &TypeInner) -> Result<(), String> {
    let pointee = match inner {
        TypeInner::Pointer { base, .. } => &module.types[*base].inner,
        other => other,
    };
    match pointee {
        TypeInner::Vector { .. } | TypeInner::Matrix { .. } | TypeInner::Array { .. } => Ok(()),
        _ => Err("only vectors, matrices and arrays are indexed".to_owned()),
    }
}

fn float_operand(inner: &TypeInner) -> Result<(), String> {
    match inner.scalar_or_vector() {
        Some((_, scalar)) if scalar.kind == ScalarKind::Float => Ok(()),
        _ => Err("this operation takes a float scalar or vector".to_owned()),
    }
}

fn binary(op: BinaryOp, left: &TypeInner, right: &TypeInner) -> Result<(), String> {
    let problem = || Err(format!("`{}` does not take these operand types", op.name()));
    let numeric = |inner: &TypeInner| matches!(inner.scalar_or_vector(), Some((_, scalar)) if scalar.kind.is_numeric());
    let shape_matches = left == right
        || matches!((left, right),
            (TypeInner::Vector { scalar, .. }, TypeInner::Scalar(other)) | (TypeInner::Scalar(other), TypeInner::Vector { scalar, .. })
                if scalar == other);
    match op {
        BinaryOp::Add | BinaryOp::Subtract => {
            let matrices = matches!(left, TypeInner::Matrix { .. }) && left == right;
            if matrices || (numeric(left) && shape_matches) {
                Ok(())
            } else {
                problem()
            }
        }
        BinaryOp::Multiply => match (left, right) {
            (
                TypeInner::Matrix {
                    columns, scalar, ..
                },
                TypeInner::Matrix {
                    rows,
                    scalar: other,
                    ..
                },
            ) if columns == rows && scalar == other => Ok(()),
            (
                TypeInner::Matrix {
                    columns, scalar, ..
                },
                TypeInner::Vector {
                    size,
                    scalar: other,
                },
            ) if columns == size && scalar == other => Ok(()),
            (
                TypeInner::Vector { size, scalar },
                TypeInner::Matrix {
                    rows,
                    scalar: other,
                    ..
                },
            ) if size == rows && scalar == other => Ok(()),
            (TypeInner::Matrix { scalar, .. }, TypeInner::Scalar(other))
            | (TypeInner::Scalar(other), TypeInner::Matrix { scalar, .. })
                if scalar == other =>
            {
                Ok(())
            }
            _ if numeric(left) && shape_matches => Ok(()),
            _ => problem(),
        },
        BinaryOp::Divide | BinaryOp::Modulo => {
            if numeric(left) && shape_matches {
                Ok(())
            } else {
                problem()
            }
        }
        BinaryOp::Equal | BinaryOp::NotEqual => {
            if left == right && left.scalar_or_vector().is_some() {
                Ok(())
            } else {
                problem()
            }
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
            if left == right && numeric(left) {
                Ok(())
            } else {
                problem()
            }
        }
        BinaryOp::And | BinaryOp::InclusiveOr | BinaryOp::ExclusiveOr => {
            match left.scalar_or_vector() {
                Some((_, scalar))
                    if left == right
                        && (scalar.kind.is_integer() || scalar.kind == ScalarKind::Bool) =>
                {
                    Ok(())
                }
                _ => problem(),
            }
        }
        BinaryOp::LogicalAnd | BinaryOp::LogicalOr => match (left, right) {
            (TypeInner::Scalar(left), TypeInner::Scalar(right))
                if left.kind == ScalarKind::Bool && right.kind == ScalarKind::Bool =>
            {
                Ok(())
            }
            _ => problem(),
        },
        BinaryOp::ShiftLeft | BinaryOp::ShiftRight => {
            match (left.scalar_or_vector(), right.scalar_or_vector()) {
                (Some((size, scalar)), Some((amount_size, amount)))
                    if scalar.kind.is_integer()
                        && amount.kind == ScalarKind::Uint
                        && size == amount_size =>
                {
                    Ok(())
                }
                _ => problem(),
            }
        }
    }
}

fn math<'a>(
    fun: MathFunction,
    args: &[Handle<Expression>],
    of: &impl Fn(Handle<Expression>) -> Result<&'a TypeInner, String>,
) -> Result<(), String> {
    if args.len() != fun.arity() {
        return Err(format!(
            "`{}` takes {} arguments, not {}",
            fun.name(),
            fun.arity(),
            args.len()
        ));
    }
    let first = of(args[0])?;
    match fun {
        MathFunction::Transpose | MathFunction::Determinant => {
            return match first {
                TypeInner::Matrix { columns, rows, .. }
                    if fun == MathFunction::Transpose || columns == rows =>
                {
                    Ok(())
                }
                _ => Err(format!("`{}` takes a square matrix", fun.name())),
            };
        }
        MathFunction::Pack4x8Snorm
        | MathFunction::Pack4x8Unorm
        | MathFunction::Pack2x16Snorm
        | MathFunction::Pack2x16Unorm
        | MathFunction::Pack2x16Float
        | MathFunction::Unpack4x8Snorm
        | MathFunction::Unpack4x8Unorm
        | MathFunction::Unpack2x16Snorm
        | MathFunction::Unpack2x16Unorm
        | MathFunction::Unpack2x16Float
        | MathFunction::ExtractBits
        | MathFunction::InsertBits => return Ok(()),
        _ => {}
    }
    let Some((size, scalar)) = first.scalar_or_vector() else {
        return Err(format!("`{}` takes scalars or vectors", fun.name()));
    };
    if !scalar.kind.is_numeric() {
        return Err(format!("`{}` takes numbers", fun.name()));
    }
    for (position, argument) in args.iter().enumerate().skip(1) {
        let inner = of(*argument)?;
        let scalar_blend = fun == MathFunction::Mix
            && position == 2
            && *inner == TypeInner::Scalar(scalar)
            && size.is_some();
        if inner != first && !scalar_blend {
            return Err(format!("the arguments of `{}` have one type", fun.name()));
        }
    }
    Ok(())
}
