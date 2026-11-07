//! TIR wait/notify atomics: LibTessera/Tarwin runtime-lowered instructions.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct WaitNotify {
    pub(crate) result: String,
    pub(crate) opcode: u64,
    pub(crate) args: Vec<String>,
}

pub(crate) fn handles(op: &str) -> bool {
    matches!(op, "atomic.wait" | "atomic.notify")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<WaitNotify, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "wait/notify requires one i32 result",
        ));
    };
    let opcode = if instruction.opcode == "atomic.wait" {
        0x205
    } else {
        0x206
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid wait/notify source span"))?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let operands = text
        .strip_prefix(&prefix)
        .ok_or_else(|| error(item, instruction.span, "invalid wait/notify spelling"))?;
    let args = operands
        .split(',')
        .map(|item_text| {
            let name = item_text
                .trim()
                .strip_prefix('%')
                .filter(|name| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
                })
                .ok_or_else(|| error(item, instruction.span, "wait/notify requires SSA values"))?;
            Ok(name.to_owned())
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    let count = if opcode == 0x205 { 3 } else { 2 };
    if args.len() != count {
        return Err(error(
            item,
            instruction.span,
            "wrong wait/notify operand count",
        ));
    }
    Ok(WaitNotify {
        result: result.clone(),
        opcode,
        args,
    })
}

pub(crate) fn validate(
    op: &WaitNotify,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    let args = op
        .args
        .iter()
        .map(|name| {
            values
                .get(name)
                .ok_or_else(|| error(item, span, "wait/notify operand is undefined"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(args[0], Type::Pointer(_)) {
        return Err(error(item, span, "atomic.wait/notify requires a pointer"));
    }
    if op.opcode == 0x205 {
        if !matches!(args[1], Type::Integer(32 | 64)) || *args[2] != Type::Integer(64) {
            return Err(error(
                item,
                span,
                "atomic.wait expects i32/i64 and an i64 timeout",
            ));
        }
    } else if *args[1] != Type::Integer(32) {
        return Err(error(item, span, "atomic.notify requires i32 count"));
    }
    Ok((op.result.clone(), Type::Integer(32)))
}

pub(crate) fn encode(
    op: &WaitNotify,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(op.opcode)?;
    write(0)?;
    write(1)?;
    for operand in &op.args {
        let position = values
            .iter()
            .position(|name| *name == operand)
            .ok_or_else(|| error(item, span, "wait/notify operand SSA value is undefined"))?;
        write((values.len() - position) as u64)?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "invalid atomic.wait/notify modifiers or result count",
        ));
    }
    let count = if instruction.opcode == 0x205 { 3 } else { 2 };
    let [BinaryOperand::Values(args)] = instruction.operands.as_slice() else {
        return Err(malformed("wait/notify must have a counted SSA value list"));
    };
    if args.len() != count {
        return Err(malformed("incorrect wait/notify binary operand count"));
    }
    let arguments = args
        .iter()
        .map(|index| {
            let (name, ty) = usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .ok_or_else(|| malformed("wait/notify SSA index is invalid"))?;
            Ok((name.clone(), *ty))
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    if !matches!(
        types.entries.get(arguments[0].1 as usize),
        Some(BinaryType::Pointer(_))
    ) {
        return Err(malformed("wait/notify requires pointer input"));
    }
    let word = |ty| {
        matches!(
            types.entries.get(ty as usize),
            Some(BinaryType::Integer(32 | 64))
        )
    };
    if instruction.opcode == 0x205 {
        if !word(arguments[1].1)
            || !matches!(
                types.entries.get(arguments[2].1 as usize),
                Some(BinaryType::Integer(64))
            )
        {
            return Err(malformed("wait expected value or timeout type is invalid"));
        }
    } else if !matches!(
        types.entries.get(arguments[1].1 as usize),
        Some(BinaryType::Integer(32))
    ) {
        return Err(malformed("notify count type is invalid"));
    }
    let ty = types
        .entries
        .iter()
        .position(|ty| matches!(ty, BinaryType::Integer(32)))
        .ok_or_else(|| malformed("wait/notify i32 result type is not interned"))?;
    let name = format!("v{}", values.len());
    let opcode = if instruction.opcode == 0x205 {
        "atomic.wait"
    } else {
        "atomic.notify"
    };
    let body = arguments
        .iter()
        .map(|(name, _)| format!("%{name}"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(text, "    %{name} = {opcode} {body}")
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, ty as u64));
    Ok(())
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
