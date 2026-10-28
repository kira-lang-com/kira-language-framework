//! V2 rules for constants, atomics, variadics, debug hints and generic
//! support: sections 12.1 and 12.14 through 12.16.

use super::{Context, require};
use klf_diag::Diagnostic;
use klf_kir::{Instruction, Operation, Type};

/// The last family in the dispatch chain, so it answers for every operation the
/// earlier families left alone.
pub(super) fn verify(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_constants_and_atomics(context, instruction, diagnostics)
        || verify_runtime(context, instruction, diagnostics)
}

/// Constant materialization and the atomic accesses of section 12.14.
fn verify_constants_and_atomics(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::ConstInt { ty, .. } => {
            require(
                ty.lane().is_integer(),
                "`const.int` requires an integer type",
                instruction,
                diagnostics,
            );
        }
        Operation::ConstFloat { ty, literal } => {
            require(
                ty.lane().is_float() && literal.parse::<f64>().is_ok(),
                "`const.float` requires a float type and a numeric literal",
                instruction,
                diagnostics,
            );
        }
        Operation::ConstZero(ty) => {
            require(
                *ty != Type::Never,
                "`const.zero` has no value of type `never`",
                instruction,
                diagnostics,
            );
        }
        Operation::AtomicLoad { ty, pointer, .. } => {
            verify_atomic_location(context, pointer, ty, instruction, diagnostics);
        }
        Operation::AtomicStore { value, pointer, .. } => {
            let Some(ty) = context.ty(value).cloned() else {
                return true;
            };
            verify_atomic_location(context, pointer, &ty, instruction, diagnostics);
        }
        Operation::AtomicRmw { pointer, value, .. } => {
            require(
                context.is_integer(value),
                "an atomic read-modify-write operand must be an integer",
                instruction,
                diagnostics,
            );
            require(
                is_location(context, pointer),
                "an atomic read-modify-write needs a pointer or address",
                instruction,
                diagnostics,
            );
        }
        Operation::AtomicCompareExchange {
            pointer,
            expected,
            replacement,
            ..
        } => {
            require(
                is_location(context, pointer),
                "`atomic.cmpxchg` needs a pointer or address",
                instruction,
                diagnostics,
            );
            require(
                context.ty(expected) == context.ty(replacement),
                "`atomic.cmpxchg` compares and stores values of one type",
                instruction,
                diagnostics,
            );
        }
        _ => return false,
    }
    true
}

/// Variadics, non-local jumps, optimizer hints and debug records.
fn verify_runtime(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::VaStart(list) | Operation::VaEnd(list) => {
            require(
                is_location(context, list),
                "a variadic list must be a pointer or address",
                instruction,
                diagnostics,
            );
        }
        Operation::VaArg { list, .. } => {
            require(
                is_location(context, list),
                "`va.arg` needs a variadic list pointer",
                instruction,
                diagnostics,
            );
        }
        Operation::VaCopy { dest, source } => {
            require(
                is_location(context, dest) && is_location(context, source),
                "`va.copy` needs two variadic list pointers",
                instruction,
                diagnostics,
            );
        }
        Operation::SetJmp(buffer) => {
            require(
                is_location(context, buffer),
                "`setjmp` needs a jump buffer address",
                instruction,
                diagnostics,
            );
        }
        Operation::LongJmp { buffer, value } => {
            require(
                is_location(context, buffer),
                "`longjmp` needs a jump buffer address",
                instruction,
                diagnostics,
            );
            require(
                context.is_integer(value),
                "`longjmp` passes an integer value",
                instruction,
                diagnostics,
            );
        }
        Operation::Assume(condition) => {
            require(
                context.ty(condition) == Some(&Type::Integer(1)),
                "`assume` requires an `i1` condition",
                instruction,
                diagnostics,
            );
        }
        Operation::Expect { value, .. } => {
            require(
                context.is_integer(value),
                "`expect` requires an integer value",
                instruction,
                diagnostics,
            );
        }
        Operation::DebugValue { value, variable } => {
            require(
                context.ty(value).is_none_or(|ty| *ty == variable.ty),
                "`debug.value` does not match the declared variable type",
                instruction,
                diagnostics,
            );
        }
        Operation::DebugAddr { address, variable } => {
            require(
                context.pointee(address).is_none_or(|ty| *ty == variable.ty),
                "`debug.addr` does not match the declared variable type",
                instruction,
                diagnostics,
            );
        }
        _ => {}
    }
    true
}

/// Atomics and variadics address memory through either a raw pointer or a
/// typed address.
fn is_location(context: &Context<'_>, name: &klf_core::Name) -> bool {
    context.is_pointer(name) || context.pointee(name).is_some()
}

fn verify_atomic_location(
    context: &Context<'_>,
    pointer: &klf_core::Name,
    ty: &Type,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    require(
        ty.is_integer() || ty.is_float() || *ty == Type::Pointer,
        "an atomic access needs a scalar type",
        instruction,
        diagnostics,
    );
    if let Some(pointee) = context.pointee(pointer) {
        require(
            pointee == ty,
            "an atomic access does not match the addressed type",
            instruction,
            diagnostics,
        );
    } else {
        require(
            context.is_pointer(pointer),
            "an atomic access needs a pointer or address",
            instruction,
            diagnostics,
        );
    }
}
