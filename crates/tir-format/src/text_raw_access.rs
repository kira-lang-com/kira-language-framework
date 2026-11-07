use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum RawAccess {
    Load {
        result: String,
        ty: Type,
        pointer: String,
        align: u64,
        flags: u64,
    },
    Store {
        value: String,
        pointer: String,
        align: u64,
        flags: u64,
    },
}

pub(crate) fn handles(op: &str) -> bool {
    matches!(op, "raw.load" | "raw.store")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<RawAccess, TextParseError> {
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "raw access source span is invalid"))?
        .trim();
    let (line, clauses) = line
        .split_once(" [")
        .map_or((line, ""), |(prefix, clauses)| (prefix, clauses));
    let mut flags = 0u64;
    let mut align = 0u64;
    if !clauses.is_empty() {
        for clause in clauses.split('[') {
            let clause = clause.trim();
            let contents = clause
                .strip_suffix(']')
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "raw access option is missing a closing bracket",
                    )
                })?
                .trim();
            match contents {
                "volatile" if flags & (1 << 18) == 0 => flags |= 1 << 18,
                "nontemporal" if flags & (1 << 19) == 0 => flags |= 1 << 19,
                _ if contents.starts_with("align ") && align == 0 => {
                    align = contents
                        .strip_prefix("align ")
                        .and_then(|text| text.parse::<u64>().ok())
                        .filter(|value| *value > 0 && value.is_power_of_two())
                        .ok_or_else(|| {
                            error(
                                item,
                                instruction.span,
                                "alignment must be a positive power of two",
                            )
                        })?;
                }
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "unknown or duplicate raw access modifier",
                    ));
                }
            }
        }
    }
    match instruction.opcode.as_str() {
        "raw.load" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "raw.load must define one SSA result",
                ));
            };
            let prefix = format!("%{result} = raw.load ");
            let (ty, pointer) = line
                .strip_prefix(&prefix)
                .and_then(|text| text.split_once(", "))
                .ok_or_else(|| {
                    error(item, instruction.span, "raw.load requires type and pointer")
                })?;
            let ty = crate::parse_type(ty.trim()).map_err(|issue| {
                error(
                    item,
                    instruction.span,
                    format!("invalid raw.load type: {issue}"),
                )
            })?;
            let pointer = parse_name(pointer, item, instruction.span)?;
            Ok(RawAccess::Load {
                result: result.clone(),
                ty,
                pointer,
                align,
                flags,
            })
        }
        "raw.store" => {
            if !instruction.results.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "raw.store cannot define an SSA result",
                ));
            }
            let (value, pointer) = line
                .strip_prefix("raw.store ")
                .and_then(|text| text.split_once(", "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "raw.store requires value and pointer",
                    )
                })?;
            Ok(RawAccess::Store {
                value: parse_name(value, item, instruction.span)?,
                pointer: parse_name(pointer, item, instruction.span)?,
                align,
                flags,
            })
        }
        _ => Err(error(
            item,
            instruction.span,
            "unsupported raw access operation",
        )),
    }
}

fn parse_name(text: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    text.trim()
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(ToOwned::to_owned)
        .ok_or_else(|| error(item, span, "raw access value must be an SSA name"))
}

fn trivial(ty: &Type) -> bool {
    match ty {
        Type::Integer(width) => *width >= 8 && width % 8 == 0,
        Type::Float(float) => float.bits() >= 8 && float.bits() % 8 == 0,
        Type::Pointer(_) => true,
        _ => false,
    }
}

pub(crate) fn validate(
    op: &RawAccess,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    match op {
        RawAccess::Load {
            result,
            ty,
            pointer,
            ..
        } => {
            if !trivial(ty) || !matches!(values.get(pointer), Some(Type::Pointer(_))) {
                return Err(error(
                    item,
                    span,
                    "raw.load requires a trivial type and raw pointer",
                ));
            }
            Ok(Some((result.clone(), ty.clone())))
        }
        RawAccess::Store { value, pointer, .. } => {
            if !values.get(value).is_some_and(trivial)
                || !matches!(values.get(pointer), Some(Type::Pointer(_)))
            {
                return Err(error(
                    item,
                    span,
                    "raw.store requires a trivial value and raw pointer",
                ));
            }
            Ok(None)
        }
    }
}

pub(crate) fn encode(
    op: &RawAccess,
    values: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index = |name: &str| -> Result<u64, TextParseError> {
        values
            .iter()
            .position(|value| *value == name)
            .map(|position| (values.len() - position) as u64)
            .ok_or_else(|| error(item, span, "raw access operand is undefined"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match op {
        RawAccess::Load {
            ty,
            pointer,
            align,
            flags,
            ..
        } => {
            let ty = crate::text_conversion_assembly::target_index(types, ty)
                .ok_or_else(|| error(item, span, "raw.load type is not interned"))?;
            write(0x146)?;
            write(*flags)?;
            write(1)?;
            write(ty as u64)?;
            write(index(pointer)?)?;
            write(*align)?;
        }
        RawAccess::Store {
            value,
            pointer,
            align,
            flags,
        } => {
            write(0x147)?;
            write(*flags)?;
            write(0)?;
            write(index(value)?)?;
            write(index(pointer)?)?;
            write(*align)?;
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    op: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if op.modifiers & !((1 << 18) | (1 << 19)) != 0 {
        return Err(malformed("unsupported raw access modifier flags"));
    }
    let mut suffix = String::new();
    let (align, body) = match (op.opcode, op.operands.as_slice()) {
        (
            0x146,
            [
                BinaryOperand::Type(ty),
                BinaryOperand::Value(pointer_id),
                BinaryOperand::Index(align),
            ],
        ) if op.result_count == 1 => {
            let ty_text = crate::text_straight_line_disassembly::constant_type_name(types, *ty)?;
            let (_, pointer_type) = get_value(values, *pointer_id)?;
            if !trivial_binary(types, *ty)
                || !matches!(
                    types.entries.get(pointer_type as usize),
                    Some(BinaryType::Pointer(_))
                )
            {
                return Err(malformed("raw.load type or pointer is invalid"));
            }
            let (pointer, _) = get_value(values, *pointer_id)?;
            let name = format!("v{}", values.len());
            (
                *align,
                (
                    format!("%{name} = raw.load {ty_text}, %{pointer}"),
                    Some((name, *ty)),
                ),
            )
        }
        (0x147, [BinaryOperand::Values(ids), BinaryOperand::Index(align)])
            if op.result_count == 0 && ids.len() == 2 =>
        {
            let (value, value_type) = get_value(values, ids[0])?;
            let (pointer, pointer_type) = get_value(values, ids[1])?;
            if !trivial_binary(types, value_type)
                || !matches!(
                    types.entries.get(pointer_type as usize),
                    Some(BinaryType::Pointer(_))
                )
            {
                return Err(malformed("raw.store value/pointer type is invalid"));
            }
            (*align, (format!("raw.store %{value}, %{pointer}"), None))
        }
        _ => {
            return Err(malformed(
                "invalid raw access operand shape or result count",
            ));
        }
    };
    if align != 0 {
        if !align.is_power_of_two() {
            return Err(malformed("raw alignment must be a power of two"));
        }
        write!(suffix, " [align {align}]").map_err(|issue| malformed(issue.to_string()))?;
    }
    if op.modifiers & (1 << 18) != 0 {
        suffix.push_str(" [volatile]");
    }
    if op.modifiers & (1 << 19) != 0 {
        suffix.push_str(" [nontemporal]");
    }
    writeln!(text, "    {}{suffix}", body.0).map_err(|issue| malformed(issue.to_string()))?;
    if let Some(result) = body.1 {
        values.push(result);
    }
    Ok(())
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("raw access SSA operand index out of range"))
}

fn trivial_binary(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(types.entries.get(index as usize),
        Some(BinaryType::Integer(bits)) if *bits >= 8 && bits % 8 == 0)
        || matches!(types.entries.get(index as usize), Some(BinaryType::Float(float)) if float.bits() >= 8 && float.bits() % 8 == 0)
        || matches!(
            types.entries.get(index as usize),
            Some(BinaryType::Pointer(_))
        )
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
