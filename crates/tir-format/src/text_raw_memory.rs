//! Restricted raw pointer and byte-range memory CODE forms (0x148–0x14D).
//! Only declared scalar SSA pointer operands are supported; no aliasing rewrite.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct RawMemory {
    pub(crate) result: Option<String>,
    pub(crate) opcode: u64,
    pub(crate) modifiers: u64,
    pub(crate) args: Vec<String>,
}

pub(crate) fn opcode(name: &str) -> Option<(u64, usize, bool)> {
    Some(match name {
        "ptr.offset" => (0x148, 2, true),
        "ptr.diff" => (0x149, 2, true),
        "mem.copy" => (0x14a, 3, false),
        "mem.move" => (0x14b, 3, false),
        "mem.set" => (0x14c, 3, false),
        "prefetch" => (0x14d, 1, false),
        _ => return None,
    })
}

pub(crate) fn name(code: u64) -> Option<&'static str> {
    Some(match code {
        0x148 => "ptr.offset",
        0x149 => "ptr.diff",
        0x14a => "mem.copy",
        0x14b => "mem.move",
        0x14c => "mem.set",
        0x14d => "prefetch",
        _ => return None,
    })
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<RawMemory, TextParseError> {
    let (opcode, count, result_expected) = opcode(&instruction.opcode)
        .ok_or_else(|| error(item, instruction.span, "unsupported raw memory opcode"))?;
    let result = match instruction.results.as_slice() {
        [name] if result_expected => Some(name.clone()),
        [] if !result_expected => None,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "memory opcode has the wrong SSA result count",
            ));
        }
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "raw memory source span is invalid"))?
        .trim();
    let prefix = match &result {
        Some(name) => format!("%{name} = {} ", instruction.opcode),
        None => format!("{} ", instruction.opcode),
    };
    let mut rest = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "invalid raw memory instruction spelling",
        )
    })?;
    let mut modifiers = 0;
    if opcode == 0x148 && rest.starts_with("[inbounds] ") {
        modifiers |= 1 << 5;
        rest = rest.strip_prefix("[inbounds] ").unwrap_or(rest);
    }
    if (0x14a..=0x14c).contains(&opcode) {
        if let Some(value) = rest.strip_suffix(" [nooverlap]") {
            if opcode != 0x14a {
                return Err(error(
                    item,
                    instruction.span,
                    "nooverlap is only valid for mem.copy",
                ));
            }
            modifiers |= 1 << 31;
            rest = value;
        }
        if let Some(value) = rest.strip_suffix(" [volatile]") {
            modifiers |= 1 << 18;
            rest = value;
        }
    }
    if opcode == 0x14d {
        if let Some(value) = rest.strip_suffix(" [instruction]") {
            modifiers |= 1 << 2;
            rest = value;
        } else if let Some(value) = rest.strip_suffix(" [data]") {
            rest = value;
        }
        for locality in 0..=3u64 {
            let suffix = format!(" [locality {locality}]");
            if let Some(value) = rest.strip_suffix(&suffix) {
                modifiers |= locality;
                rest = value;
                break;
            }
        }
        if let Some(value) = rest.strip_suffix(" [write]") {
            modifiers |= 1 << 48;
            rest = value;
        } else if let Some(value) = rest.strip_suffix(" [read]") {
            rest = value;
        }
    }
    let args = rest
        .split(',')
        .map(|value| {
            let name = value
                .trim()
                .strip_prefix('%')
                .filter(|name| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
                })
                .ok_or_else(|| {
                    error(item, instruction.span, "memory operands must be SSA names")
                })?;
            Ok(name.to_owned())
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    if args.len() != count {
        return Err(error(
            item,
            instruction.span,
            "raw memory operand count is invalid",
        ));
    }
    Ok(RawMemory {
        result,
        opcode,
        modifiers,
        args,
    })
}

pub(crate) fn validate(
    op: &RawMemory,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let args = op
        .args
        .iter()
        .map(|name| {
            values
                .get(name)
                .ok_or_else(|| error(item, span, "raw memory operand is undefined"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let pointer = |ty: &Type| matches!(ty, Type::Pointer(_));
    let integer = |ty: &Type, width| *ty == Type::Integer(width);
    let valid = match op.opcode {
        0x148 => pointer(args[0]) && integer(args[1], 64),
        0x149 => pointer(args[0]) && args[0] == args[1],
        0x14a | 0x14b => pointer(args[0]) && pointer(args[1]) && integer(args[2], 64),
        0x14c => pointer(args[0]) && integer(args[1], 8) && integer(args[2], 64),
        0x14d => pointer(args[0]),
        _ => false,
    };
    if !valid {
        return Err(error(
            item,
            span,
            "raw memory operand types are incompatible",
        ));
    }
    Ok(op.result.as_ref().map(|name| {
        (
            name.clone(),
            if op.opcode == 0x148 {
                args[0].clone()
            } else {
                Type::Integer(64)
            },
        )
    }))
}

pub(crate) fn encode(
    op: &RawMemory,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(op.opcode)?;
    write(op.modifiers)?;
    write(u64::from(op.result.is_some()))?;
    for arg in &op.args {
        let position = values
            .iter()
            .position(|name| name == arg)
            .ok_or_else(|| error(item, span, "memory operand is undefined"))?;
        write((values.len() - position) as u64)?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    op: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let name = name(op.opcode).ok_or_else(|| malformed("unknown raw memory opcode"))?;
    let (opcode, arity, result) = opcode(name).expect("known raw memory opcode");
    if op.result_count != u64::from(result) {
        return Err(malformed("raw memory result count is invalid"));
    }
    let allowed = match opcode {
        0x148 => 1 << 5,
        0x14a => (1 << 18) | (1 << 31),
        0x14b | 0x14c => 1 << 18,
        0x14d => (1 << 48) | 7,
        _ => 0,
    };
    if op.modifiers & !allowed != 0 {
        return Err(malformed("unsupported raw memory modifier bits"));
    }
    let indices = match op.operands.as_slice() {
        [BinaryOperand::Value(index)] if arity == 1 => vec![*index],
        [BinaryOperand::Values(indices)] if indices.len() == arity => indices.clone(),
        _ => return Err(malformed("wrong raw memory operand shape")),
    };
    let mut args = Vec::new();
    for index in &indices {
        let (name, _) = usize::try_from(*index)
            .ok()
            .and_then(|index| values.get(index))
            .ok_or_else(|| malformed("raw memory SSA operand index invalid"))?;
        args.push(format!("%{name}"));
    }
    let mods = match opcode {
        0x148 if op.modifiers == (1 << 5) => "[inbounds] ".to_owned(),
        0x14a | 0x14b | 0x14c => {
            let mut flags = String::new();
            if op.modifiers & (1 << 18) != 0 {
                flags.push_str(" [volatile]");
            }
            if op.modifiers & (1 << 31) != 0 {
                flags.push_str(" [nooverlap]");
            }
            flags
        }
        0x14d => {
            let mut flags = String::new();
            if op.modifiers >> 48 == 1 {
                flags.push_str(" [write]");
            }
            let locality = op.modifiers & 3;
            if locality != 0 {
                write!(flags, " [locality {locality}]").map_err(formatting_error)?;
            }
            if op.modifiers & 4 != 0 {
                flags.push_str(" [instruction]");
            }
            flags
        }
        _ => String::new(),
    };
    let prefix = if opcode == 0x148 { mods.as_str() } else { "" };
    let suffix = if opcode == 0x148 { "" } else { mods.as_str() };
    match result {
        true => {
            let output = format!("v{}", values.len());
            writeln!(
                text,
                "    %{output} = {name} {prefix}{}{suffix}",
                args.join(", ")
            )
            .map_err(formatting_error)?;
            let ty = if opcode == 0x148 {
                values
                    .get(indices[0] as usize)
                    .map(|(_, ty)| *ty)
                    .ok_or_else(|| malformed("raw pointer operand is undefined"))?
            } else {
                types
                    .entries
                    .iter()
                    .position(|ty| matches!(ty, BinaryType::Integer(64)))
                    .ok_or_else(|| malformed("ptr.diff i64 result is not interned"))?
                    as u64
            };
            values.push((output, ty));
        }
        false => {
            writeln!(text, "    {name} {}{suffix}", args.join(", ")).map_err(formatting_error)?
        }
    }
    Ok(())
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
fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
fn formatting_error(issue: std::fmt::Error) -> FormatError {
    malformed(issue.to_string())
}
