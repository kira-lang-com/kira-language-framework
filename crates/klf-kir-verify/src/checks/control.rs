//! V2 rules for the terminators of section 12.13.

use super::{Context, require};
use crate::{V1, type_error};
use klf_diag::Diagnostic;
use klf_kir::{BlockTarget, Instruction, Operation, Type};

pub(super) fn verify(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    verify_branches(context, instruction, diagnostics)
        || verify_exits(context, instruction, diagnostics)
}

/// Branches and switches: the terminators that stay inside the function.
fn verify_branches(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::Branch(target) => verify_target(context, target, instruction, diagnostics),
        Operation::ConditionalBranch(branch) => {
            let klf_kir::ConditionalBranch {
                condition,
                then_target,
                else_target,
            } = &**branch;
            require(
                context.ty(condition) == Some(&Type::Integer(1)),
                "`cond.br` condition must have type `i1`",
                instruction,
                diagnostics,
            );
            verify_target(context, then_target, instruction, diagnostics);
            verify_target(context, else_target, instruction, diagnostics);
        }
        Operation::SwitchInt(switch) => {
            let klf_kir::IntegerSwitch {
                value,
                cases,
                default,
            } = &**switch;
            require(
                context.is_integer(value),
                "`switch.int` requires an integer selector",
                instruction,
                diagnostics,
            );
            for (_, target) in cases {
                verify_target(context, target, instruction, diagnostics);
            }
            verify_target(context, default, instruction, diagnostics);
        }
        Operation::SwitchEnum(switch) => {
            let klf_kir::EnumSwitch {
                value,
                cases,
                default,
            } = &**switch;
            let Some(declaration) = context.enum_declaration(context.ty(value)) else {
                type_error(
                    instruction,
                    "`switch.enum` requires an address of a declared enum",
                    diagnostics,
                );
                return true;
            };
            for (variant, target) in cases {
                require(
                    declaration.variants.get(*variant as usize).is_some(),
                    "`switch.enum` names an invalid variant",
                    instruction,
                    diagnostics,
                );
                verify_target(context, target, instruction, diagnostics);
            }
            if let Some(default) = default {
                verify_target(context, default, instruction, diagnostics);
            } else {
                require(
                    cases.len() == declaration.variants.len(),
                    "`switch.enum` without a default must list every variant",
                    instruction,
                    diagnostics,
                );
            }
        }
        Operation::BranchIndirect { address, targets } => {
            require(
                context.ty(address).is_some(),
                "`br.indirect` requires a block address",
                instruction,
                diagnostics,
            );
            for target in targets {
                require(
                    context.blocks.contains_key(target),
                    "`br.indirect` names an unknown block",
                    instruction,
                    diagnostics,
                );
            }
        }
        _ => return false,
    }
    true
}

/// Returns, failures and the operations that name a block or function.
fn verify_exits(
    context: &Context<'_>,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    match &instruction.operation {
        Operation::Return(value) => {
            let actual = value.as_ref().and_then(|name| context.ty(name));
            let valid = if context.function.result == Type::Unit {
                value.is_none()
            } else {
                actual == Some(&context.function.result)
            };
            require(
                valid,
                format!(
                    "return does not match function result `{}`",
                    context.function.result
                ),
                instruction,
                diagnostics,
            );
        }
        Operation::Fail(value) => {
            require(
                context.function.failure.as_ref() == context.ty(value),
                "`fail` value does not match the function failure type",
                instruction,
                diagnostics,
            );
        }
        Operation::BlockAddress(name) => {
            require(
                context.blocks.contains_key(name),
                "`block.addr` names an unknown block",
                instruction,
                diagnostics,
            );
        }
        Operation::FunctionReference(name) => {
            require(
                context.function_named(name).is_some(),
                "`func.ref` names an unknown function",
                instruction,
                diagnostics,
            );
        }
        Operation::ClosureCallback { user_data, ty } => {
            let user_data = ty.parameters.get(*user_data as usize);
            require(
                ty.abi == klf_kir::FunctionAbi::Platform
                    && ty.failure.is_none()
                    && ty.parameters.iter().all(|parameter| !parameter.context),
                "`closure.callback` produces a non-failing platform function without context parameters",
                instruction,
                diagnostics,
            );
            require(
                user_data.is_some_and(|parameter| parameter.ty == klf_kir::Type::Pointer),
                "`closure.callback` user-data parameter must exist and have type `ptr`",
                instruction,
                diagnostics,
            );
        }
        Operation::Trap(_) | Operation::Unreachable => {}
        _ => return false,
    }
    true
}

/// A branch's arguments must match the target block's parameters. The mismatch
/// is an SSA-shape failure, so it carries V1 rather than V2.
pub(super) fn verify_target(
    context: &Context<'_>,
    target: &BlockTarget,
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    verify_delivering_target(context, target, &[], instruction, diagnostics);
}

/// A target whose leading block parameters receive `delivered`, the values
/// the instruction itself hands over (the result or error of `try.call`),
/// ahead of the arguments the target names.
pub(super) fn verify_delivering_target(
    context: &Context<'_>,
    target: &BlockTarget,
    delivered: &[&Type],
    instruction: &Instruction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(block_index) = context.blocks.get(&target.block) else {
        return;
    };
    let parameters = &context.function.blocks[*block_index].parameters;
    if parameters.len() != delivered.len() + target.arguments.len() {
        diagnostics.push(
            Diagnostic::error(
                V1,
                format!(
                    "branch to `{}` delivers {} values but target expects {}",
                    target.block,
                    delivered.len() + target.arguments.len(),
                    parameters.len()
                ),
            )
            .with_label(instruction.span, "block argument count does not match"),
        );
        return;
    }
    let (received, passed) = parameters.split_at(delivered.len());
    for (ty, parameter) in delivered.iter().zip(received) {
        if **ty != parameter.ty {
            diagnostics.push(
                Diagnostic::error(
                    V1,
                    format!(
                        "block parameter `%{}` does not match the value the instruction delivers",
                        parameter.name
                    ),
                )
                .with_label(instruction.span, format!("delivers `{ty}`")),
            );
        }
    }
    for (argument, parameter) in target.arguments.iter().zip(passed) {
        if context.ty(argument) != Some(&parameter.ty) {
            diagnostics.push(
                Diagnostic::error(
                    V1,
                    format!(
                        "argument `%{argument}` does not match block parameter `%{}`",
                        parameter.name
                    ),
                )
                .with_label(
                    instruction.span,
                    format!("expected type `{}`", parameter.ty),
                ),
            );
        }
    }
}
