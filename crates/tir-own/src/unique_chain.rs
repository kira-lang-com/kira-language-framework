//! Conservative ownership handoff through linear unconditional block-parameter
//! forwarding, without merges, loops, aliasing, or implicit copy resolution.
use tir_format::{FunctionBody, TextInstruction};
use tir_types::{Convention, Type};

use crate::{CanonicalizeError, error, unique_linear};

pub(super) fn remaining_at_exit(
    body: &FunctionBody,
    function: &str,
) -> Result<Vec<String>, CanonicalizeError> {
    if body.blocks.len() < 2 {
        return Err(error(
            "linear ownership forwarding requires multiple blocks",
        ));
    }
    let entry = &body.blocks[0];
    let mut owned = entry
        .parameters
        .iter()
        .filter(|p| matches!(p.ty, Type::Unique(_)) && p.convention == Some(Convention::Owned))
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    if owned.is_empty() {
        return Err(error(
            "linear ownership forwarding requires an owned unique entry parameter",
        ));
    }
    for pair in body.blocks.windows(2) {
        let from = &pair[0];
        let to = &pair[1];
        let [terminator] = from.instructions.as_slice() else {
            return Err(error(
                "ownership forwarding cannot consume before an intermediate branch",
            ));
        };
        if terminator.opcode != "br" {
            return Err(error(
                "ownership forwarding permits only unconditional branches",
            ));
        }
        let (target, arguments) = branch(terminator, function)?;
        if target != to.label || arguments.len() != to.parameters.len() {
            return Err(error(
                "linear ownership branch does not match the next block parameters",
            ));
        }
        let mut next = Vec::new();
        for (argument, parameter) in arguments.iter().zip(&to.parameters) {
            if matches!(parameter.ty, Type::Unique(_)) {
                if parameter.convention != Some(Convention::Owned) || !owned.contains(argument) {
                    return Err(error(
                        "owned unique branch parameter was not forwarded from its live owner",
                    ));
                }
                let passed_ty = from
                    .parameters
                    .iter()
                    .find(|p| p.name == *argument)
                    .or_else(|| entry.parameters.iter().find(|p| p.name == *argument))
                    .map(|p| &p.ty);
                if passed_ty != Some(&parameter.ty) {
                    return Err(error("owned unique forwarding changes the pointee type"));
                }
                if next.len() >= owned.len() {
                    return Err(error("owned unique forwarding duplicates a live owner"));
                }
                next.push((argument.clone(), parameter.name.clone()));
            } else if !crate::is_pure_value_type(&parameter.ty) {
                return Err(error(
                    "only owned unique and pure-value block parameters can be forwarded",
                ));
            }
        }
        if next.len() != owned.len()
            || owned
                .iter()
                .any(|name| next.iter().filter(|(arg, _)| arg == name).count() != 1)
        {
            return Err(error(
                "every live owned unique value must be forwarded exactly once",
            ));
        }
        owned = next.into_iter().map(|(_, parameter)| parameter).collect();
    }
    let last = body.blocks.last().expect("two or more blocks");
    let references = owned.iter().map(String::as_str).collect::<Vec<_>>();
    unique_linear::remaining_owned_values(last, &references)
}

fn branch(
    instruction: &TextInstruction,
    function: &str,
) -> Result<(String, Vec<String>), CanonicalizeError> {
    let line = function
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error("forwarding branch has an invalid source span"))?
        .trim();
    let Some(line) = line.strip_prefix("br ") else {
        return Err(error("forwarding branch must start with `br`"));
    };
    let Some((target, operands)) = line.split_once('(') else {
        return Err(error(
            "forwarding branch requires an explicit argument list",
        ));
    };
    let Some(operands) = operands.strip_suffix(')') else {
        return Err(error("forwarding branch must have a closed argument list"));
    };
    let target = target.trim();
    if target.is_empty()
        || !target
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(error("forwarding branch has an invalid destination label"));
    }
    let arguments = if operands.trim().is_empty() {
        Vec::new()
    } else {
        operands
            .split(',')
            .map(|operand| {
                let operand = operand.trim();
                let Some(name) = operand.strip_prefix('%') else {
                    return Err(error("forwarding branch requires SSA arguments"));
                };
                if name.is_empty()
                    || !name
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
                {
                    return Err(error("forwarding branch has an invalid SSA argument"));
                }
                Ok(name.to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok((target.to_owned(), arguments))
}
