//! Sections 12.14 and 12.15: atomics, variadics, jumps, assembly and hints.

use super::common::{Parsed, parse_type, split_call, split_top_level_once};
use super::constants::unquote;
use crate::parse::tables::{atomic_operators, parse_memory_order};
use crate::parse::{ParseError, error, parse_two_values, parse_value, split_top_level};
use crate::{DebugVariable, InlineAsm, Operation};

pub(super) fn parse(text: &str, offset: usize) -> Parsed {
    if let Some(operation) = parse_atomic(text, offset)? {
        return Ok(Some(operation));
    }
    if let Some(operation) = parse_variadic(text, offset)? {
        return Ok(Some(operation));
    }
    parse_hint(text, offset)
}

fn parse_atomic(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("atomic.load ") {
        let (ty, rest) = required_split(rest, offset)?;
        let (pointer, order) = split_order(rest, offset)?;
        Operation::AtomicLoad {
            ty: parse_type(ty, offset)?,
            pointer: parse_value(pointer, offset)?,
            order,
        }
    } else if let Some(rest) = text.strip_prefix("atomic.store ") {
        let (value, rest) = required_split(rest, offset)?;
        let (pointer, order) = split_order(rest, offset)?;
        Operation::AtomicStore {
            value: parse_value(value, offset)?,
            pointer: parse_value(pointer, offset)?,
            order,
        }
    } else if let Some(rest) = text.strip_prefix("atomic.rmw ") {
        return parse_rmw(rest, offset).map(Some);
    } else if let Some(rest) = text.strip_prefix("atomic.cmpxchg ") {
        return parse_compare_exchange(rest, offset).map(Some);
    } else if let Some(order) = text.strip_prefix("fence ") {
        Operation::Fence(parse_memory_order(order, offset)?)
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_rmw(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let (spelling, rest) = text
        .split_once(' ')
        .ok_or_else(|| error(offset, "`atomic.rmw` needs an operator"))?;
    let operator = atomic_operators()
        .into_iter()
        .find(|candidate| candidate.spelling() == spelling)
        .ok_or_else(|| error(offset, "unknown atomic operator"))?;
    let (pointer, rest) = required_split(rest, offset)?;
    let (value, order) = split_order(rest, offset)?;
    Ok(Operation::AtomicRmw {
        operator,
        pointer: parse_value(pointer, offset)?,
        value: parse_value(value, offset)?,
        order,
    })
}

fn parse_compare_exchange(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let (weak, rest) = match text.trim_start().strip_prefix("[weak] ") {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let parts = split_top_level(rest, ',');
    if parts.len() != 3 {
        return Err(error(
            offset,
            "`atomic.cmpxchg` needs `%pointer, %expected, %new`",
        ));
    }
    let (replacement, orders) = parts[2]
        .trim()
        .split_once(' ')
        .ok_or_else(|| error(offset, "`atomic.cmpxchg` needs two memory orders"))?;
    let (success, failure) = orders
        .trim()
        .split_once(' ')
        .ok_or_else(|| error(offset, "`atomic.cmpxchg` needs two memory orders"))?;
    Ok(Operation::AtomicCompareExchange {
        weak,
        pointer: parse_value(parts[0], offset)?,
        expected: parse_value(parts[1], offset)?,
        replacement: parse_value(replacement, offset)?,
        success: parse_memory_order(success, offset)?,
        failure: parse_memory_order(failure, offset)?,
    })
}

fn parse_variadic(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("va.start ") {
        Operation::VaStart(parse_value(rest, offset)?)
    } else if let Some(rest) = text.strip_prefix("va.arg ") {
        let (ty, list) = required_split(rest, offset)?;
        Operation::VaArg {
            ty: parse_type(ty, offset)?,
            list: parse_value(list, offset)?,
        }
    } else if let Some(rest) = text.strip_prefix("va.copy ") {
        let (dest, source) = parse_two_values(rest, offset)?;
        Operation::VaCopy { dest, source }
    } else if let Some(rest) = text.strip_prefix("va.end ") {
        Operation::VaEnd(parse_value(rest, offset)?)
    } else if let Some(rest) = text.strip_prefix("setjmp ") {
        Operation::SetJmp(parse_value(rest, offset)?)
    } else if let Some(rest) = text.strip_prefix("longjmp ") {
        let (buffer, value) = parse_two_values(rest, offset)?;
        Operation::LongJmp { buffer, value }
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_hint(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("asm ") {
        Operation::Asm(Box::new(parse_asm(rest, offset)?))
    } else if let Some(rest) = text.strip_prefix("landingpad") {
        return super::calls::parse_landing_pad(rest, offset).map(Some);
    } else if let Some(rest) = text.strip_prefix("debug.value ") {
        let (value, variable) = required_split(rest, offset)?;
        Operation::DebugValue {
            value: parse_value(value, offset)?,
            variable: Box::new(parse_debug_variable(variable, offset)?),
        }
    } else if let Some(rest) = text.strip_prefix("debug.addr ") {
        let (address, variable) = required_split(rest, offset)?;
        Operation::DebugAddr {
            address: parse_value(address, offset)?,
            variable: Box::new(parse_debug_variable(variable, offset)?),
        }
    } else if let Some(rest) = text.strip_prefix("assume ") {
        Operation::Assume(parse_value(rest, offset)?)
    } else if let Some(rest) = text.strip_prefix("expect ") {
        let (value, expected) = required_split(rest, offset)?;
        Operation::Expect {
            value: parse_value(value, offset)?,
            expected: expected
                .trim()
                .parse()
                .map_err(|_| error(offset, "`expect` needs an integer"))?,
        }
    } else if text == "nop" {
        Operation::Nop
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_debug_variable(text: &str, offset: usize) -> Result<DebugVariable, ParseError> {
    let body = text
        .trim()
        .strip_prefix("!var(")
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(|| error(offset, "expected `!var(name, type, scope)`"))?;
    let parts = split_top_level(body, ',');
    if parts.len() != 3 {
        return Err(error(offset, "`!var` needs a name, a type and a scope"));
    }
    Ok(DebugVariable {
        name: unquote(parts[0], offset)?,
        ty: parse_type(parts[1], offset)?,
        scope: unquote(parts[2], offset)?,
    })
}

/// Splits a trailing memory order from `%value seq_cst`.
fn split_order(text: &str, offset: usize) -> Result<(&str, crate::MemoryOrder), ParseError> {
    let (value, order) = text
        .trim()
        .rsplit_once(' ')
        .ok_or_else(|| error(offset, "expected a memory order"))?;
    Ok((value, parse_memory_order(order, offset)?))
}

fn required_split(text: &str, offset: usize) -> Result<(&str, &str), ParseError> {
    split_top_level_once(text, ',').ok_or_else(|| error(offset, "expected `,`"))
}

/// `"template"`, or `"template", "constraints"(args)`.
fn parse_asm(text: &str, offset: usize) -> Result<InlineAsm, ParseError> {
    let comma = crate::parse::unquoted(text).find(|&(_, character)| character == ',');
    let Some((comma, _)) = comma else {
        return Ok(InlineAsm {
            template: unquote(text, offset)?,
            constraints: String::new(),
            arguments: Vec::new(),
        });
    };
    let (constraints, arguments, tail) = split_call(&text[comma + 1..], offset, "asm")?;
    if !tail.is_empty() {
        return Err(error(offset, "unexpected text after `asm` arguments"));
    }
    Ok(InlineAsm {
        template: unquote(&text[..comma], offset)?,
        constraints: unquote(constraints, offset)?,
        arguments,
    })
}
