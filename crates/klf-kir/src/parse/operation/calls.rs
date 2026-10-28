//! Section 12.11 calls and section 12.12 unwinding.

use super::common::{
    Parsed, parse_index, parse_result_suffix, parse_type, parse_values, split_call,
    split_top_level_once,
};
use crate::parse::{ParseError, error, matching_close, parse_target, parse_value};
use crate::{Operation, Type};
use klf_core::Name;

pub(super) fn parse(text: &str, offset: usize) -> Parsed {
    if let Some(operation) = parse_dispatch(text, offset)? {
        return Ok(Some(operation));
    }
    if let Some(rest) = text.strip_prefix("try.call @") {
        return parse_try_call(rest, offset).map(Some);
    }
    if let Some(rest) = text.strip_prefix("invoke @") {
        return parse_invoke(rest, offset).map(Some);
    }
    if let Some(rest) = text.strip_prefix("call.variadic @") {
        return parse_variadic(rest, offset).map(Some);
    }
    if let Some(rest) = text.strip_prefix("call.foreign @") {
        let (symbol, arguments, _) = split_call(rest, offset, "call.foreign")?;
        return Ok(Some(Operation::CallForeign {
            symbol: crate::parse::symbol(symbol, offset)?,
            arguments,
        }));
    }
    if let Some(rest) = text.strip_prefix("call @") {
        let (head, arguments, _) = split_call(rest, offset, "call")?;
        let (callee, generic_args) = parse_call_head(head, offset)?;
        return Ok(Some(Operation::Call {
            callee,
            generic_args,
            arguments,
        }));
    }
    Ok(None)
}

fn parse_dispatch(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("call.indirect %") {
        let (target, arguments, tail) = split_call(rest, offset, "call.indirect")?;
        let (params, result) = parse_signature(tail, offset, "call.indirect")?;
        Operation::CallIndirect(Box::new(crate::IndirectCall {
            target: Name::new(target),
            params,
            result,
            arguments,
        }))
    } else if let Some(rest) = text.strip_prefix("call.closure %") {
        let (closure, arguments, tail) = split_call(rest, offset, "call.closure")?;
        let (params, result) = parse_signature(tail, offset, "call.closure")?;
        Operation::CallClosure(Box::new(crate::ClosureCall {
            closure: Name::new(closure),
            params,
            result,
            arguments,
        }))
    } else if let Some(rest) = text.strip_prefix("call.witness ") {
        let (head, arguments, tail) = split_call(rest, offset, "call.witness")?;
        let (ty, method) = split_top_level_once(head, ',')
            .ok_or_else(|| error(offset, "`call.witness` needs `<type>, %Trait.method`"))?;
        let (trait_name, method) = method
            .trim()
            .trim_start_matches('%')
            .split_once('.')
            .ok_or_else(|| error(offset, "`call.witness` needs `%Trait.method`"))?;
        Operation::CallWitness(Box::new(crate::WitnessCall {
            ty: parse_type(ty, offset)?,
            trait_name: Name::new(trait_name),
            method: Name::new(method),
            arguments,
            result: parse_result_suffix(tail, offset, "call.witness")?,
        }))
    } else if let Some(rest) = text.strip_prefix("call.vtable %") {
        let (head, arguments, tail) = split_call(rest, offset, "call.vtable")?;
        let parts = crate::parse::split_top_level(head, ',');
        if parts.len() != 3 {
            return Err(error(
                offset,
                "`call.vtable` needs `%object, @vtable, <slot>`",
            ));
        }
        Operation::CallVtable(Box::new(crate::VtableCall {
            object: Name::new(parts[0].trim()),
            vtable: crate::parse::symbol(parts[1].trim().trim_start_matches('@'), offset)?,
            slot: parse_index(parts[2], offset)?,
            arguments,
            result: parse_result_suffix(tail, offset, "call.vtable")?,
        }))
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_signature(text: &str, offset: usize, what: &str) -> Result<(Vec<Type>, Type), ParseError> {
    let rest = text
        .trim_start()
        .strip_prefix(':')
        .ok_or_else(|| error(offset, format!("`{what}` requires `: (params) -> result`")))?
        .trim_start();
    let open = rest
        .find('(')
        .ok_or_else(|| error(offset, format!("`{what}` signature needs a parameter list")))?;
    let close = matching_close(rest, open, '(', ')')
        .ok_or_else(|| error(offset, format!("unclosed `{what}` parameter list")))?;
    let body = &rest[open + 1..close];
    let params = if body.trim().is_empty() {
        Vec::new()
    } else {
        crate::type_parse::parse_type_list(body, offset)?
    };
    Ok((
        params,
        parse_result_suffix(&rest[close + 1..], offset, what)?,
    ))
}

fn parse_variadic(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let open = crate::parse::find_unquoted(text, '(')
        .ok_or_else(|| error(offset, "`call.variadic` requires an argument list"))?;
    let close = matching_close(text, open, '(', ')')
        .ok_or_else(|| error(offset, "unclosed call.variadic arguments"))?;
    let body = &text[open + 1..close];
    let (fixed, variadic) = split_top_level_once(body, ';')
        .ok_or_else(|| error(offset, "`call.variadic` separates fixed arguments with `;`"))?;
    Ok(Operation::CallVariadic {
        callee: crate::parse::symbol(&text[..open], offset)?,
        fixed: parse_values(fixed, offset)?,
        variadic: parse_values(variadic, offset)?,
    })
}

fn parse_try_call(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let (head, normal, error_target) = split_two_targets(text, offset, " normal ", " error ")?;
    let (callee, generic_args, arguments) = parse_callee(head, offset, "try.call")?;
    Ok(Operation::TryCall(Box::new(crate::FallibleCall {
        callee,
        generic_args,
        arguments,
        normal: parse_target(normal, offset)?,
        error: parse_target(error_target, offset)?,
    })))
}

fn parse_invoke(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let (head, normal, unwind) = split_two_targets(text, offset, " to ", " unwind ")?;
    let (callee, _, arguments) = parse_callee(head, offset, "invoke")?;
    Ok(Operation::Invoke(Box::new(crate::UnwindingCall {
        callee,
        arguments,
        normal: parse_target(normal, offset)?,
        unwind: parse_target(unwind, offset)?,
    })))
}

fn split_two_targets<'a>(
    text: &'a str,
    offset: usize,
    first: &str,
    second: &str,
) -> Result<(&'a str, &'a str, &'a str), ParseError> {
    let second_at = text
        .rfind(second)
        .ok_or_else(|| error(offset, format!("expected `{}`", second.trim())))?;
    let before = &text[..second_at];
    let first_at = before
        .rfind(first)
        .ok_or_else(|| error(offset, format!("expected `{}`", first.trim())))?;
    Ok((
        &before[..first_at],
        &before[first_at + first.len()..],
        &text[second_at + second.len()..],
    ))
}

fn parse_callee(
    text: &str,
    offset: usize,
    what: &str,
) -> Result<(Name, Vec<Type>, Vec<Name>), ParseError> {
    let (head, arguments, _) = split_call(text, offset, what)?;
    let (callee, generic_args) = parse_call_head(head, offset)?;
    Ok((callee, generic_args, arguments))
}

/// Parses `callee` or `callee<T, U>`.
fn parse_call_head(text: &str, offset: usize) -> Result<(Name, Vec<Type>), ParseError> {
    let Some(open) = crate::parse::find_unquoted(text, '<') else {
        return Ok((crate::parse::symbol(text, offset)?, Vec::new()));
    };
    let close = matching_close(text, open, '<', '>')
        .ok_or_else(|| error(offset, "unclosed generic argument list"))?;
    let generic_args = crate::type_parse::parse_type_list(&text[open + 1..close], offset)?;
    Ok((crate::parse::symbol(&text[..open], offset)?, generic_args))
}

pub(super) fn parse_landing_pad(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let mut cleanup = false;
    let mut catches = Vec::new();
    let mut rest = text.trim();
    if let Some(after) = rest.strip_prefix("cleanup") {
        cleanup = true;
        rest = after.trim_start();
    }
    while let Some(after) = rest.strip_prefix("catch ") {
        let (value, tail) = after
            .split_once(' ')
            .map_or((after, ""), |(value, tail)| (value, tail));
        catches.push(parse_value(value, offset)?);
        rest = tail.trim_start();
    }
    if !rest.is_empty() {
        return Err(error(offset, "invalid `landingpad` clause"));
    }
    Ok(Operation::LandingPad { cleanup, catches })
}
