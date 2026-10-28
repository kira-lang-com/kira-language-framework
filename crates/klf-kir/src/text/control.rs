//! Printing for calls, terminators and the system instructions of sections
//! 12.11 through 12.15.

use super::emit::{arguments, debug_variable, generic_arguments, type_list, values_list};
use super::function::write_target;
use super::spelling::Symbol;
use crate::Operation;
use std::fmt;

pub(super) fn write_call(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::Call {
            callee,
            generic_args,
            arguments: values,
        } => {
            write!(formatter, "call @{}", Symbol(callee.as_str()))?;
            generic_arguments(formatter, generic_args)?;
            arguments(formatter, values)
        }
        Operation::CallForeign {
            symbol,
            arguments: values,
        } => {
            write!(formatter, "call.foreign @{}", Symbol(symbol.as_str()))?;
            arguments(formatter, values)
        }
        Operation::CallIndirect(call) => {
            let crate::IndirectCall {
                target,
                params,
                result,
                arguments: values,
            } = &**call;
            write!(formatter, "call.indirect %{target}")?;
            arguments(formatter, values)?;
            formatter.write_str(" : (")?;
            type_list(formatter, params)?;
            write!(formatter, ") -> {result}")
        }
        Operation::CallClosure(call) => {
            let crate::ClosureCall {
                closure,
                params,
                result,
                arguments: values,
            } = &**call;
            write!(formatter, "call.closure %{closure}")?;
            arguments(formatter, values)?;
            formatter.write_str(" : (")?;
            type_list(formatter, params)?;
            write!(formatter, ") -> {result}")
        }
        Operation::CallWitness(call) => {
            let crate::WitnessCall {
                ty,
                trait_name,
                method,
                arguments: values,
                result,
            } = &**call;
            write!(formatter, "call.witness {ty}, %{trait_name}.{method}")?;
            arguments(formatter, values)?;
            write!(formatter, " -> {result}")
        }
        Operation::CallVtable(call) => {
            let crate::VtableCall {
                object,
                vtable,
                slot,
                arguments: values,
                result,
            } = &**call;
            write!(
                formatter,
                "call.vtable %{object}, @{}, {slot}",
                Symbol(vtable.as_str())
            )?;
            arguments(formatter, values)?;
            write!(formatter, " -> {result}")
        }
        Operation::CallVariadic {
            callee,
            fixed,
            variadic,
        } => {
            write!(formatter, "call.variadic @{}(", Symbol(callee.as_str()))?;
            values_list(formatter, fixed)?;
            formatter.write_str("; ")?;
            values_list(formatter, variadic)?;
            formatter.write_str(")")
        }
        _ => return Ok(false),
    }?;
    Ok(true)
}

pub(super) fn write_terminator(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    if write_call_terminator(formatter, operation)? {
        return Ok(true);
    }
    match operation {
        Operation::Branch(target) => {
            formatter.write_str("br ")?;
            write_target(formatter, target)
        }
        Operation::ConditionalBranch(branch) => {
            let crate::ConditionalBranch {
                condition,
                then_target,
                else_target,
            } = &**branch;
            write!(formatter, "cond.br %{condition}, ")?;
            write_target(formatter, then_target)?;
            formatter.write_str(", ")?;
            write_target(formatter, else_target)
        }
        Operation::SwitchInt(switch) => {
            let crate::IntegerSwitch {
                value,
                cases,
                default,
            } = &**switch;
            write!(formatter, "switch.int %{value}, [")?;
            for (index, (key, target)) in cases.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "{key}: ")?;
                write_target(formatter, target)?;
            }
            formatter.write_str("], default ")?;
            write_target(formatter, default)
        }
        Operation::SwitchEnum(switch) => {
            let crate::EnumSwitch {
                value,
                cases,
                default,
            } = &**switch;
            write!(formatter, "switch.enum %{value}, [")?;
            for (index, (variant, target)) in cases.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "{variant}: ")?;
                write_target(formatter, target)?;
            }
            formatter.write_str("]")?;
            if let Some(default) = default {
                formatter.write_str(", default ")?;
                write_target(formatter, default)?;
            }
            Ok(())
        }
        Operation::BranchIndirect { address, targets } => {
            write!(formatter, "br.indirect %{address}, [")?;
            for (index, target) in targets.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "{target}")?;
            }
            formatter.write_str("]")
        }
        Operation::Return(None) => formatter.write_str("return"),
        Operation::Return(Some(value)) => write!(formatter, "return %{value}"),
        Operation::Fail(value) => write!(formatter, "fail %{value}"),
        Operation::Trap(code) if is_bare_code(code) => write!(formatter, "trap {code}"),
        Operation::Trap(code) => write!(formatter, "trap {code:?}"),
        Operation::Unreachable => formatter.write_str("unreachable"),
        Operation::Resume(value) => write!(formatter, "resume %{value}"),
        Operation::Throw(value) => write!(formatter, "throw %{value}"),
        _ => return Ok(false),
    }?;
    Ok(true)
}

pub(super) fn write_system(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::LandingPad { cleanup, catches } => {
            formatter.write_str("landingpad")?;
            if *cleanup {
                formatter.write_str(" cleanup")?;
            }
            for catch in catches {
                write!(formatter, " catch %{catch}")?;
            }
            Ok(())
        }
        Operation::AtomicLoad { ty, pointer, order } => {
            write!(
                formatter,
                "atomic.load {ty}, %{pointer} {}",
                order.spelling()
            )
        }
        Operation::AtomicStore {
            value,
            pointer,
            order,
        } => write!(
            formatter,
            "atomic.store %{value}, %{pointer} {}",
            order.spelling()
        ),
        Operation::AtomicRmw {
            operator,
            pointer,
            value,
            order,
        } => write!(
            formatter,
            "atomic.rmw {} %{pointer}, %{value} {}",
            operator.spelling(),
            order.spelling()
        ),
        Operation::AtomicCompareExchange {
            weak,
            pointer,
            expected,
            replacement,
            success,
            failure,
        } => {
            formatter.write_str("atomic.cmpxchg")?;
            if *weak {
                formatter.write_str(" [weak]")?;
            }
            write!(
                formatter,
                " %{pointer}, %{expected}, %{replacement} {} {}",
                success.spelling(),
                failure.spelling()
            )
        }
        Operation::Fence(order) => write!(formatter, "fence {}", order.spelling()),
        Operation::VaStart(list) => write!(formatter, "va.start %{list}"),
        Operation::VaArg { ty, list } => write!(formatter, "va.arg {ty}, %{list}"),
        Operation::VaCopy { dest, source } => write!(formatter, "va.copy %{dest}, %{source}"),
        Operation::VaEnd(list) => write!(formatter, "va.end %{list}"),
        Operation::SetJmp(buffer) => write!(formatter, "setjmp %{buffer}"),
        Operation::LongJmp { buffer, value } => {
            write!(formatter, "longjmp %{buffer}, %{value}")
        }
        Operation::Asm(asm) => {
            write!(formatter, "asm {:?}", asm.template)?;
            if asm.constraints.is_empty() && asm.arguments.is_empty() {
                Ok(())
            } else {
                write!(formatter, ", {:?}", asm.constraints)?;
                super::emit::arguments(formatter, &asm.arguments)
            }
        }
        Operation::DebugValue { value, variable } => {
            write!(formatter, "debug.value %{value}, ")?;
            debug_variable(formatter, variable)
        }
        Operation::DebugAddr { address, variable } => {
            write!(formatter, "debug.addr %{address}, ")?;
            debug_variable(formatter, variable)
        }
        Operation::Assume(value) => write!(formatter, "assume %{value}"),
        Operation::Expect { value, expected } => {
            write!(formatter, "expect %{value}, {expected}")
        }
        Operation::Nop => formatter.write_str("nop"),
        _ => return Ok(false),
    }?;
    Ok(true)
}

fn is_bare_code(code: &str) -> bool {
    !code.is_empty()
        && code.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

/// `try.call` and `invoke`, the calls that end a block.
fn write_call_terminator(
    formatter: &mut fmt::Formatter<'_>,
    operation: &Operation,
) -> Result<bool, fmt::Error> {
    match operation {
        Operation::TryCall(call) => {
            let crate::FallibleCall {
                callee,
                generic_args,
                arguments: values,
                normal,
                error,
            } = &**call;
            write!(formatter, "try.call @{}", Symbol(callee.as_str()))?;
            generic_arguments(formatter, generic_args)?;
            arguments(formatter, values)?;
            formatter.write_str(" normal ")?;
            write_target(formatter, normal)?;
            formatter.write_str(" error ")?;
            write_target(formatter, error)
        }
        Operation::Invoke(call) => {
            let crate::UnwindingCall {
                callee,
                arguments: values,
                normal,
                unwind,
            } = &**call;
            write!(formatter, "invoke @{}", Symbol(callee.as_str()))?;
            arguments(formatter, values)?;
            formatter.write_str(" to ")?;
            write_target(formatter, normal)?;
            formatter.write_str(" unwind ")?;
            write_target(formatter, unwind)
        }
        _ => return Ok(false),
    }?;
    Ok(true)
}
