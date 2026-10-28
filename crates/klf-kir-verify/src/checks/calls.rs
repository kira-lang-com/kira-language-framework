//! V2 rules for the call and unwinding instructions of sections 12.11 and
//! 12.12.

use super::{Context, require};
use crate::type_error;
use klf_core::Name;
use klf_diag::Diagnostic;
use klf_kir::{Function, FunctionAbi, Instruction, Operation, Type};

pub(super) fn verify(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_direct_calls(context, instruction, diagnostics)
        || verify_dispatch(context, instruction, diagnostics)
}

/// Direct, foreign, variadic and indirect calls.
fn verify_direct_calls(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::Call {
            callee, arguments, ..
        } => {
            let target = context.function_named(callee);
            verify_declared_call(context, target, arguments, "call", instruction, diagnostics);
        }
        Operation::CallForeign { symbol, arguments } => {
            let target = context
                .function_named(symbol)
                .filter(|function| function.foreign);
            verify_declared_call(
                context,
                target,
                arguments,
                "call.foreign",
                instruction,
                diagnostics,
            );
        }
        Operation::CallVariadic {
            callee,
            fixed,
            variadic,
        } => {
            let Some(target) = context.function_named(callee) else {
                type_error(
                    instruction,
                    "`call.variadic` names an unknown function",
                    diagnostics,
                );
                return true;
            };
            require(
                target.parameters.len() == fixed.len(),
                "`call.variadic` fixed argument count does not match the callee",
                instruction,
                diagnostics,
            );
            for argument in variadic {
                require(
                    context.ty(argument).is_some(),
                    "`call.variadic` variadic argument has no known type",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::CallIndirect(call) => {
            let klf_kir::IndirectCall {
                target,
                params,
                result,
                arguments,
            } = &**call;
            verify_indirect_call(
                context,
                target,
                params,
                result,
                arguments,
                instruction,
                diagnostics,
            );
        }
        Operation::CallClosure(call) => {
            let klf_kir::ClosureCall {
                closure,
                params,
                arguments,
                ..
            } = &**call;
            require(
                matches!(context.ty(closure), Some(Type::Pointer | Type::Address(_))),
                "`call.closure` operand must point at a two-word closure record",
                instruction,
                diagnostics,
            );
            signature_arity(context, params, arguments, instruction, diagnostics);
        }
        _ => return false,
    }
    true
}

fn verify_indirect_call(
    context: &Context<'_>,
    target: &Name,
    params: &[Type],
    result: &Type,
    arguments: &[Name],
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(Type::Function(signature)) = context.ty(target) else {
        type_error(
            instruction,
            "`call.indirect` requires a function-pointer operand with a known ABI",
            diagnostics,
        );
        return;
    };
    require(
        signature.parameters.len() == params.len(),
        "`call.indirect` parameter count does not match the function-pointer type",
        instruction,
        diagnostics,
    );
    require(
        signature
            .parameters
            .iter()
            .filter(|parameter| parameter.context)
            .count()
            <= 1,
        "`call.indirect` function type has multiple context parameters",
        instruction,
        diagnostics,
    );
    for (declared, called) in signature.parameters.iter().zip(params) {
        if declared.context {
            require(
                matches!(
                    &declared.ty,
                    Type::Pointer
                        | Type::Address(_)
                        | Type::Reference(_)
                        | Type::Unique(_)
                        | Type::Weak(_)
                ),
                "`call.indirect` context parameter must have pointer type",
                instruction,
                diagnostics,
            );
        }
        require(
            declared.ty == *called,
            "`call.indirect` parameter type does not match the function-pointer type",
            instruction,
            diagnostics,
        );
    }
    require(
        signature.result.as_ref() == result,
        "`call.indirect` result type does not match the function-pointer type",
        instruction,
        diagnostics,
    );
    require(
        signature.failure.is_none() || signature.abi == FunctionAbi::Ridl,
        "`call.indirect` of a failing function pointer needs the RIDL convention",
        instruction,
        diagnostics,
    );
    require(
        instruction.results.len() == usize::from(*result != Type::Unit),
        "`call.indirect` result shape does not match the function-pointer result",
        instruction,
        diagnostics,
    );
    signature_arity(context, params, arguments, instruction, diagnostics);
}

/// Witness and vtable dispatch, and the unwinding operations of section 12.12.
fn verify_dispatch(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_witness_calls(context, instruction, diagnostics)
        || verify_unwinding(context, instruction, diagnostics)
}

/// Witness and vtable dispatch (section 12.11).
fn verify_witness_calls(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::CallWitness(call) => {
            let klf_kir::WitnessCall {
                ty,
                trait_name,
                arguments,
                ..
            } = &**call;
            require(
                context
                    .function
                    .generics
                    .iter()
                    .any(|generic| matches!(ty, Type::Generic(name) if *name == generic.name))
                    || !matches!(ty, Type::Generic(_)),
                "`call.witness` names a generic parameter the function does not declare",
                instruction,
                diagnostics,
            );
            let _ = trait_name;
            for argument in arguments {
                require(
                    context.ty(argument).is_some(),
                    "`call.witness` argument has no known type",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::CallVtable(call) => {
            let klf_kir::VtableCall {
                object, arguments, ..
            } = &**call;
            require(
                context.ty(object).is_some(),
                "`call.vtable` requires a receiver",
                instruction,
                diagnostics,
            );
            for argument in arguments {
                require(
                    context.ty(argument).is_some(),
                    "`call.vtable` argument has no known type",
                    instruction,
                    diagnostics,
                );
            }
        }
        _ => return false,
    }
    true
}

/// The try, invoke and unwinding operations of section 12.12.
fn verify_unwinding(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::TryCall(call) => {
            let klf_kir::FallibleCall {
                callee,
                arguments,
                normal,
                error,
                ..
            } = &**call;
            let target = context.function_named(callee);
            let Some(target) = target else {
                type_error(
                    instruction,
                    "`try.call` names an unknown function",
                    diagnostics,
                );
                return true;
            };
            arguments_match(
                context,
                target,
                arguments,
                "try.call",
                instruction,
                diagnostics,
            );
            require(
                target.failure.is_some(),
                "`try.call` requires a callee that declares a failure type",
                instruction,
                diagnostics,
            );
            let result: &[&Type] = if target.result == Type::Unit {
                &[]
            } else {
                &[&target.result]
            };
            super::control::verify_delivering_target(
                context,
                normal,
                result,
                instruction,
                diagnostics,
            );
            let failure: Vec<&Type> = target.failure.iter().collect();
            super::control::verify_delivering_target(
                context,
                error,
                &failure,
                instruction,
                diagnostics,
            );
        }
        Operation::Invoke(call) => {
            let klf_kir::UnwindingCall {
                callee,
                arguments,
                normal,
                unwind,
            } = &**call;
            let Some(target) = context.function_named(callee) else {
                type_error(
                    instruction,
                    "`invoke` names an unknown function",
                    diagnostics,
                );
                return true;
            };
            arguments_match(
                context,
                target,
                arguments,
                "invoke",
                instruction,
                diagnostics,
            );
            super::control::verify_target(context, normal, instruction, diagnostics);
            super::control::verify_target(context, unwind, instruction, diagnostics);
        }
        Operation::LandingPad { cleanup, catches } => {
            require(
                *cleanup || !catches.is_empty(),
                "`landingpad` must be a cleanup or list at least one catch",
                instruction,
                diagnostics,
            );
        }
        Operation::Resume(value) | Operation::Throw(value) => {
            require(
                context.definitions.contains_key(value),
                "unwinding requires an exception operand",
                instruction,
                diagnostics,
            );
        }
        _ => return false,
    }
    true
}

/// Checks a call against a declared callee: argument arity, argument types and
/// the result shape implied by the callee's result type.
fn verify_declared_call(
    context: &Context<'_>,
    target: Option<&Function>,
    arguments: &[Name],
    what: &str,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(target) = target else {
        type_error(
            instruction,
            format!("`{what}` names an unknown function"),
            diagnostics,
        );
        return;
    };
    if !arguments_match(context, target, arguments, what, instruction, diagnostics) {
        return;
    }
    let expected = usize::from(target.result != Type::Unit);
    require(
        instruction.results.len() == expected,
        format!("`{what}` result shape does not match the callee result"),
        instruction,
        diagnostics,
    );
}

fn arguments_match(
    context: &Context<'_>,
    target: &Function,
    arguments: &[Name],
    what: &str,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    if !require(
        target.parameters.len() == arguments.len(),
        format!(
            "`{what}` argument count does not match `@{}`: expected {}, found {}",
            target.name,
            target.parameters.len(),
            arguments.len()
        ),
        instruction,
        diagnostics,
    ) {
        return false;
    }
    let mut ok = true;
    for (parameter, argument) in target.parameters.iter().zip(arguments) {
        ok &= require(
            context.ty(argument) == Some(&parameter.ty),
            format!("`{what}` argument type does not match the callee parameter"),
            instruction,
            diagnostics,
        );
    }
    ok
}

fn signature_arity(
    context: &Context<'_>,
    params: &[Type],
    arguments: &[Name],
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !require(
        params.len() == arguments.len(),
        "indirect call argument count does not match its signature",
        instruction,
        diagnostics,
    ) {
        return;
    }
    for (expected, argument) in params.iter().zip(arguments) {
        require(
            context.ty(argument) == Some(expected),
            "indirect call argument does not match its signature",
            instruction,
            diagnostics,
        );
    }
}
