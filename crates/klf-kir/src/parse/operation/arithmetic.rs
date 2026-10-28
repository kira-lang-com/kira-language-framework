//! Sections 12.2, 12.3 and 12.4: integer, float and conversion instructions.

use super::common::{Parsed, parse_type};
use crate::parse::tables::{
    binary_operators, convert_kinds, float_binary_operators, float_unary_operators,
    integer_unary_operators, parse_float_predicate, parse_overflow, parse_predicate, saturating,
};
use crate::parse::{
    ParseError, error, parse_two_values, parse_value, split_once_required, split_top_level,
};
use crate::{Operation, Overflow};

pub(super) fn parse(text: &str, offset: usize) -> Parsed {
    if let Some(operation) = parse_compare(text, offset)? {
        return Ok(Some(operation));
    }
    if let Some(operation) = parse_float(text, offset)? {
        return Ok(Some(operation));
    }
    if let Some(operation) = parse_convert(text, offset)? {
        return Ok(Some(operation));
    }
    parse_integer(text, offset)
}

fn parse_compare(text: &str, offset: usize) -> Parsed {
    if let Some(rest) = text.strip_prefix("icmp ") {
        let (predicate, arguments) = split_once_required(rest, " ", offset)?;
        let (lhs, rhs) = parse_two_values(arguments, offset)?;
        return Ok(Some(Operation::IntegerCompare {
            predicate: parse_predicate(predicate, offset)?,
            lhs,
            rhs,
        }));
    }
    if let Some(rest) = text.strip_prefix("fcmp ") {
        let (predicate, arguments) = split_once_required(rest, " ", offset)?;
        let (lhs, rhs) = parse_two_values(arguments, offset)?;
        return Ok(Some(Operation::FloatCompare {
            predicate: parse_float_predicate(predicate, offset)?,
            lhs,
            rhs,
        }));
    }
    if let Some(rest) = text.strip_prefix("select ") {
        let parts = split_top_level(rest, ',');
        if parts.len() != 3 {
            return Err(error(offset, "`select` requires three operands"));
        }
        return Ok(Some(Operation::Select {
            condition: parse_value(parts[0], offset)?,
            then_value: parse_value(parts[1], offset)?,
            else_value: parse_value(parts[2], offset)?,
        }));
    }
    Ok(None)
}

fn parse_float(text: &str, offset: usize) -> Parsed {
    if let Some(rest) = text.strip_prefix("fma ") {
        let parts = split_top_level(rest, ',');
        if parts.len() != 3 {
            return Err(error(offset, "`fma` requires three operands"));
        }
        return Ok(Some(Operation::FloatFma {
            first: parse_value(parts[0], offset)?,
            second: parse_value(parts[1], offset)?,
            addend: parse_value(parts[2], offset)?,
        }));
    }
    for (spelling, operator) in float_binary_operators() {
        if let Some(rest) = strip_opcode(text, spelling) {
            let (lhs, rhs) = parse_two_values(rest, offset)?;
            return Ok(Some(Operation::FloatBinary { operator, lhs, rhs }));
        }
    }
    for (spelling, operator) in float_unary_operators() {
        if let Some(value) = strip_opcode(text, spelling) {
            return Ok(Some(Operation::FloatUnary {
                operator,
                value: parse_value(value, offset)?,
            }));
        }
    }
    Ok(None)
}

fn parse_convert(text: &str, offset: usize) -> Parsed {
    for kind in convert_kinds() {
        let Some(rest) = strip_opcode(text, kind.spelling()) else {
            continue;
        };
        let (kind, rest) = match rest.strip_prefix("[sat] ") {
            Some(rest) => (
                saturating(kind)
                    .ok_or_else(|| error(offset, "`[sat]` applies to `fptosi` and `fptoui`"))?,
                rest,
            ),
            None => (kind, rest),
        };
        let (value, ty) = rest
            .split_once(" to ")
            .ok_or_else(|| error(offset, "conversion requires `%value to <type>`"))?;
        return Ok(Some(Operation::Convert {
            kind,
            value: parse_value(value, offset)?,
            ty: parse_type(ty, offset)?,
        }));
    }
    Ok(None)
}

fn parse_integer(text: &str, offset: usize) -> Parsed {
    for operator in integer_unary_operators() {
        if let Some(value) = strip_opcode(text, operator.spelling()) {
            return Ok(Some(Operation::IntegerUnary {
                operator,
                value: parse_value(value, offset)?,
            }));
        }
    }
    for (spelling, operator) in binary_operators() {
        if let Some(rest) = text
            .strip_prefix(spelling)
            .and_then(|rest| rest.strip_prefix(".ovf "))
        {
            let (signed, rest) = parse_signedness(rest);
            let (lhs, rhs) = parse_two_values(rest, offset)?;
            if !operator.has_overflow_form() {
                return Err(error(
                    offset,
                    "`.ovf` applies to `add`, `sub` and `mul` only",
                ));
            }
            return Ok(Some(Operation::BinaryOverflow {
                operator,
                signed,
                lhs,
                rhs,
            }));
        }
        let Some(rest) = strip_opcode(text, spelling) else {
            continue;
        };
        let (overflow, rest) = parse_overflow_mode(rest, offset)?;
        let (signed, rest) = parse_signedness(rest);
        let (lhs, rhs) = parse_two_values(rest, offset)?;
        return Ok(Some(Operation::Binary {
            operator,
            overflow,
            signed,
            lhs,
            rhs,
        }));
    }
    Ok(None)
}

fn parse_overflow_mode(text: &str, offset: usize) -> Result<(Overflow, &str), ParseError> {
    if !text.starts_with('[') {
        return Ok((Overflow::Wrap, text));
    }
    let end = text
        .find(']')
        .ok_or_else(|| error(offset, "unclosed overflow mode"))?;
    Ok((
        parse_overflow(&text[1..end], offset)?,
        text[end + 1..].trim_start(),
    ))
}

/// The optional `s`/`u` signedness token of section 12.2; signed by default.
fn parse_signedness(text: &str) -> (bool, &str) {
    if let Some(rest) = text.strip_prefix("s ") {
        (true, rest.trim_start())
    } else if let Some(rest) = text.strip_prefix("u ") {
        (false, rest.trim_start())
    } else {
        (true, text)
    }
}

fn strip_opcode<'a>(text: &'a str, opcode: &str) -> Option<&'a str> {
    text.strip_prefix(opcode)
        .and_then(|rest| rest.strip_prefix(' '))
        .map(str::trim_start)
}
