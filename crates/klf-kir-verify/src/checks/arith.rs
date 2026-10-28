//! V2 rules for the arithmetic, conversion and vector instructions of sections
//! 12.2 through 12.5. Arithmetic, comparisons, `select`, `fma` and
//! conversions accept vectors elementwise (section 12.5).

use super::{Context, require};
use crate::type_error;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{BinaryOperator, ConvertKind, Instruction, Operation, Type};

pub(super) fn verify(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_scalar(context, instruction, diagnostics)
        || verify_vector(context, instruction, diagnostics)
}

/// Integer and float arithmetic, comparison and selection.
fn verify_scalar(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::Binary {
            operator,
            overflow,
            lhs,
            rhs,
            ..
        } => {
            same_integer(
                context,
                lhs,
                rhs,
                "integer binary",
                instruction,
                diagnostics,
            );
            require(
                operator.accepts_overflow(*overflow),
                "only `add`, `sub` and `mul` accept a non-wrapping overflow mode, and `sdiv` and `srem` accept `[trap]`",
                instruction,
                diagnostics,
            );
        }
        Operation::BinaryOverflow {
            operator, lhs, rhs, ..
        } => {
            same_integer(context, lhs, rhs, "`.ovf`", instruction, diagnostics);
            require(
                !matches!(context.ty(lhs), Some(Type::Vector(..))),
                "`.ovf` takes scalar operands",
                instruction,
                diagnostics,
            );
            require(
                matches!(
                    operator,
                    BinaryOperator::Add | BinaryOperator::Sub | BinaryOperator::Mul
                ),
                "only `add`, `sub` and `mul` have an `.ovf` form",
                instruction,
                diagnostics,
            );
        }
        Operation::IntegerUnary { value, .. } => {
            require(
                lanes_are(context, value, Type::is_integer),
                "integer unary operand must have integer type",
                instruction,
                diagnostics,
            );
        }
        Operation::IntegerCompare { lhs, rhs, .. } => {
            same_integer(context, lhs, rhs, "`icmp`", instruction, diagnostics);
        }
        Operation::Select {
            condition,
            then_value,
            else_value,
        } => verify_select(
            context,
            condition,
            then_value,
            else_value,
            instruction,
            diagnostics,
        ),
        Operation::FloatBinary { lhs, rhs, .. } => {
            same_float(
                context,
                lhs,
                rhs,
                "floating-point",
                instruction,
                diagnostics,
            );
        }
        Operation::FloatCompare { lhs, rhs, .. } => {
            same_float(context, lhs, rhs, "`fcmp`", instruction, diagnostics);
        }
        Operation::FloatUnary { value, .. } => {
            require(
                lanes_are(context, value, Type::is_float),
                "floating-point unary operand must have float type",
                instruction,
                diagnostics,
            );
        }
        Operation::FloatFma {
            first,
            second,
            addend,
        } => {
            same_float(context, first, second, "`fma`", instruction, diagnostics);
            same_float(context, second, addend, "`fma`", instruction, diagnostics);
        }
        _ => return false,
    }
    true
}

fn verify_select(
    context: &Context<'_>,
    condition: &Name,
    then_value: &Name,
    else_value: &Name,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let then_ty = context.ty(then_value);
    let condition_ok = match (context.ty(condition), then_ty) {
        (Some(Type::Integer(1)), _) => true,
        (Some(Type::Vector(count, lane)), Some(Type::Vector(arms, _))) => {
            count == arms && **lane == Type::Integer(1)
        }
        _ => false,
    };
    require(
        condition_ok,
        "`select` condition must have type `i1`, or `vec<N x i1>` for arms of N lanes",
        instruction,
        diagnostics,
    );
    require(
        then_ty.is_some() && then_ty == context.ty(else_value),
        "`select` arms must have the same type",
        instruction,
        diagnostics,
    );
}

/// Conversions, vector operations and intrinsics.
fn verify_vector(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::Convert { kind, value, ty } => {
            verify_convert(context, *kind, value, ty, instruction, diagnostics);
        }
        Operation::VectorSplat { ty, value } => {
            let Type::Vector(_, element) = ty else {
                type_error(
                    instruction,
                    "`vec.splat` requires a vector type",
                    diagnostics,
                );
                return true;
            };
            require(
                context.ty(value) == Some(element.as_ref()),
                "`vec.splat` operand must have the vector's element type",
                instruction,
                diagnostics,
            );
        }
        Operation::VectorExtract { vector, lane } => {
            lane_in_range(context, vector, *lane, instruction, diagnostics);
        }
        Operation::VectorInsert {
            vector,
            lane,
            value,
        } => {
            if lane_in_range(context, vector, *lane, instruction, diagnostics) {
                let element = context.ty(vector).map(Type::lane);
                require(
                    context.ty(value) == element,
                    "`vec.insert` value must have the vector's element type",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::VectorShuffle { lhs, rhs, mask } => {
            let lhs_ty = context.ty(lhs);
            if !require(
                matches!(lhs_ty, Some(Type::Vector(_, _))) && lhs_ty == context.ty(rhs),
                "`vec.shuffle` operands must be vectors of the same type",
                instruction,
                diagnostics,
            ) {
                return true;
            }
            let Some(Type::Vector(count, _)) = lhs_ty else {
                return true;
            };
            require(
                mask.iter().all(|lane| *lane < count * 2),
                "`vec.shuffle` mask selects a lane outside both operands",
                instruction,
                diagnostics,
            );
        }
        Operation::Intrinsic(call) => {
            let klf_kir::IntrinsicCall { name, .. } = &**call;
            require(
                !name.is_empty(),
                "`intrinsic` requires a name",
                instruction,
                diagnostics,
            );
        }
        _ => return false,
    }
    true
}

fn lane_in_range(
    context: &Context<'_>,
    vector: &Name,
    lane: u32,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(Type::Vector(count, _)) = context.ty(vector) else {
        type_error(
            instruction,
            "vector lane access requires a vector operand",
            diagnostics,
        );
        return false;
    };
    require(
        lane < *count,
        "vector lane index is out of range",
        instruction,
        diagnostics,
    )
}

fn same_integer(
    context: &Context<'_>,
    lhs: &Name,
    rhs: &Name,
    what: &str,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let lhs_ty = context.ty(lhs);
    require(
        lhs_ty.is_some() && lhs_ty == context.ty(rhs) && lanes_are(context, lhs, Type::is_integer),
        format!("{what} operands must have the same integer type"),
        instruction,
        diagnostics,
    );
}

fn same_float(
    context: &Context<'_>,
    lhs: &Name,
    rhs: &Name,
    what: &str,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let lhs_ty = context.ty(lhs);
    require(
        lhs_ty.is_some() && lhs_ty == context.ty(rhs) && lanes_are(context, lhs, Type::is_float),
        format!("{what} operands must have the same float type"),
        instruction,
        diagnostics,
    );
}

/// Whether `value` is a scalar of the kind `test` accepts, or a vector of
/// such lanes.
fn lanes_are(context: &Context<'_>, value: &Name, test: fn(&Type) -> bool) -> bool {
    context.ty(value).is_some_and(|ty| test(ty.lane()))
}

fn verify_convert(
    context: &Context<'_>,
    kind: ConvertKind,
    value: &Name,
    ty: &Type,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let source = context.ty(value);
    if matches!(
        ty,
        Type::Function(destination)
            if destination.failure.is_some() && destination.abi == klf_kir::FunctionAbi::Platform
    ) {
        type_error(
            instruction,
            "a failing function cannot convert to the platform convention, which has no failure channel",
            diagnostics,
        );
        return;
    }
    if matches!(
        (source, ty),
        (Some(Type::Function(source)), Type::Function(destination))
            if source.abi != destination.abi
    ) {
        type_error(
            instruction,
            "function conversion cannot change ABI without an adapter thunk",
            diagnostics,
        );
        return;
    }
    let lanes_match = match (source, ty) {
        (Some(Type::Vector(from, _)), Type::Vector(to, _)) => from == to,
        (Some(Type::Vector(..)), _) | (_, Type::Vector(..)) => kind == ConvertKind::Bitcast,
        _ => true,
    };
    let (source, ty) = if kind == ConvertKind::Bitcast {
        (source, ty)
    } else {
        (source.map(Type::lane), ty.lane())
    };
    let is_int = source.is_some_and(Type::is_integer);
    let is_float = source.is_some_and(Type::is_float);
    let is_ptr = matches!(source, Some(Type::Pointer));
    let ok = match kind {
        ConvertKind::Truncate | ConvertKind::ZeroExtend | ConvertKind::SignExtend => {
            is_int && ty.is_integer()
        }
        ConvertKind::FloatTruncate | ConvertKind::FloatExtend => is_float && ty.is_float(),
        ConvertKind::SignedToFloat | ConvertKind::UnsignedToFloat => is_int && ty.is_float(),
        ConvertKind::FloatToSigned
        | ConvertKind::FloatToUnsigned
        | ConvertKind::FloatToSignedSaturating
        | ConvertKind::FloatToUnsignedSaturating => is_float && ty.is_integer(),
        ConvertKind::PointerToInt => is_ptr && ty.is_integer(),
        ConvertKind::IntToPointer => is_int && matches!(ty, Type::Pointer),
        ConvertKind::Bitcast => source.is_some(),
    };
    require(
        ok && lanes_match,
        format!(
            "`{}` operand and result types are incompatible",
            kind.spelling()
        ),
        instruction,
        diagnostics,
    );
}
