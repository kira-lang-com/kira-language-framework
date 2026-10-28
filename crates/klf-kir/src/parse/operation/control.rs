//! Section 12.13 terminators.

use super::common::Parsed;
use crate::Operation;
use crate::parse::{
    ParseError, error, matching_close, parse_target, parse_value, split_top_level, unquote,
};

pub(super) fn parse(text: &str, offset: usize) -> Parsed {
    let operation = if let Some(rest) = text.strip_prefix("br.indirect ") {
        return parse_branch_indirect(rest, offset).map(Some);
    } else if let Some(target) = text.strip_prefix("br ") {
        Operation::Branch(parse_target(target, offset)?)
    } else if let Some(rest) = text.strip_prefix("cond.br ") {
        let parts = split_top_level(rest, ',');
        if parts.len() != 3 {
            return Err(error(
                offset,
                "`cond.br` requires a condition and two targets",
            ));
        }
        Operation::ConditionalBranch(Box::new(crate::ConditionalBranch {
            condition: parse_value(parts[0], offset)?,
            then_target: parse_target(parts[1], offset)?,
            else_target: parse_target(parts[2], offset)?,
        }))
    } else if let Some(rest) = text.strip_prefix("switch.int ") {
        return parse_switch_int(rest, offset).map(Some);
    } else if let Some(rest) = text.strip_prefix("switch.enum ") {
        return parse_switch_enum(rest, offset).map(Some);
    } else if text == "return" {
        Operation::Return(None)
    } else if let Some(value) = text.strip_prefix("return ") {
        Operation::Return(Some(parse_value(value, offset)?))
    } else if let Some(value) = text.strip_prefix("fail ") {
        Operation::Fail(parse_value(value, offset)?)
    } else if let Some(value) = text.strip_prefix("resume ") {
        Operation::Resume(parse_value(value, offset)?)
    } else if let Some(value) = text.strip_prefix("throw ") {
        Operation::Throw(parse_value(value, offset)?)
    } else if let Some(code) = text.strip_prefix("trap ") {
        let code = code.trim();
        Operation::Trap(if code.starts_with('"') {
            unquote(code, offset)?
        } else {
            code.to_owned()
        })
    } else if text == "unreachable" {
        Operation::Unreachable
    } else {
        return Ok(None);
    };
    Ok(Some(operation))
}

fn parse_branch_indirect(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let (address, rest) = text
        .split_once(',')
        .ok_or_else(|| error(offset, "`br.indirect` requires a target list"))?;
    let (open, close) = bracketed_range(rest, offset, "br.indirect")?;
    let body = rest[open + 1..close].trim();
    let targets = if body.is_empty() {
        Vec::new()
    } else {
        split_top_level(body, ',')
            .into_iter()
            .map(|target| klf_core::Name::new(target.trim()))
            .collect()
    };
    Ok(Operation::BranchIndirect {
        address: parse_value(address, offset)?,
        targets,
    })
}

fn parse_switch_int(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let (value, cases_text, tail) = split_switch(text, offset, "switch.int")?;
    let cases = parse_cases(cases_text, offset, |literal| {
        literal
            .trim()
            .parse::<i128>()
            .map_err(|_| error(offset, "invalid switch case value"))
    })?;
    let default = tail
        .strip_prefix(',')
        .map_or(tail, str::trim_start)
        .strip_prefix("default ")
        .ok_or_else(|| error(offset, "`switch.int` requires a `default` target"))?;
    Ok(Operation::SwitchInt(Box::new(crate::IntegerSwitch {
        value: parse_value(value, offset)?,
        cases,
        default: parse_target(default, offset)?,
    })))
}

fn parse_switch_enum(text: &str, offset: usize) -> Result<Operation, ParseError> {
    let (value, cases_text, tail) = split_switch(text, offset, "switch.enum")?;
    let cases = parse_cases(cases_text, offset, |literal| {
        literal
            .trim()
            .parse::<u32>()
            .map_err(|_| error(offset, "invalid enum variant index"))
    })?;
    let default = tail
        .strip_prefix(',')
        .map(str::trim_start)
        .and_then(|tail| tail.strip_prefix("default "))
        .map(|target| parse_target(target, offset))
        .transpose()?;
    if !tail.is_empty() && default.is_none() {
        return Err(error(offset, "invalid switch.enum suffix"));
    }
    Ok(Operation::SwitchEnum(Box::new(crate::EnumSwitch {
        value: parse_value(value, offset)?,
        cases,
        default,
    })))
}

fn split_switch<'a>(
    text: &'a str,
    offset: usize,
    what: &str,
) -> Result<(&'a str, &'a str, &'a str), ParseError> {
    let (value, after) = text
        .split_once(',')
        .ok_or_else(|| error(offset, format!("`{what}` requires a value and cases")))?;
    let (open, close) = bracketed_range(after, offset, what)?;
    Ok((
        value,
        &after[open + 1..close],
        after[close + 1..].trim_start(),
    ))
}

fn bracketed_range(text: &str, offset: usize, what: &str) -> Result<(usize, usize), ParseError> {
    let open = text
        .find('[')
        .ok_or_else(|| error(offset, format!("`{what}` requires a `[...]` list")))?;
    let close = matching_close(text, open, '[', ']')
        .ok_or_else(|| error(offset, format!("unclosed `{what}` list")))?;
    Ok((open, close))
}

fn parse_cases<K>(
    text: &str,
    offset: usize,
    mut key: impl FnMut(&str) -> Result<K, ParseError>,
) -> Result<Vec<(K, crate::BlockTarget)>, ParseError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(text, ',')
        .into_iter()
        .map(|entry| {
            let (literal, target) = entry
                .split_once(':')
                .ok_or_else(|| error(offset, "switch case must be `value: target`"))?;
            Ok((key(literal)?, parse_target(target, offset)?))
        })
        .collect()
}
