use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum DeviceIo {
    Load {
        result: String,
        ty: Type,
        pointer: String,
        alignment: u64,
        order: u64,
    },
    Store {
        value: String,
        pointer: String,
        alignment: u64,
        order: u64,
    },
    Fence {
        selector: u64,
    },
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(name, "mmio.load" | "mmio.store" | "fence.io")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<DeviceIo, TextParseError> {
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "MMIO instruction source span is invalid",
            )
        })?
        .trim();
    if instruction.opcode == "fence.io" {
        if !instruction.results.is_empty() {
            return Err(error(
                item,
                instruction.span,
                "fence.io cannot define an SSA result",
            ));
        }
        let selector = match line {
            "fence.io" | "fence.io [read]" => 0,
            "fence.io [write]" => 1,
            "fence.io [full]" => 2,
            _ => return Err(error(item, instruction.span, "invalid fence.io selector")),
        };
        return Ok(DeviceIo::Fence { selector });
    }
    let (body, options) = line
        .split_once(" [")
        .map_or((line, ""), |(body, options)| (body, options));
    let mut order = 0u64;
    let mut alignment = 0u64;
    let mut saw_order = false;
    let mut saw_alignment = false;
    if !options.is_empty() {
        for clause in options.split('[') {
            let contents = clause
                .trim()
                .strip_suffix(']')
                .ok_or_else(|| error(item, instruction.span, "unclosed MMIO option"))?
                .trim();
            if let Some(mode) = contents.strip_prefix("order ") {
                if saw_order {
                    return Err(error(item, instruction.span, "duplicate MMIO ordering"));
                }
                order = match (instruction.opcode.as_str(), mode) {
                    (_, "relaxed") => 1,
                    ("mmio.load", "acquire") => 2,
                    ("mmio.store", "release") => 3,
                    _ => return Err(error(item, instruction.span, "invalid MMIO ordering")),
                };
                saw_order = true;
            } else if let Some(value) = contents.strip_prefix("align ") {
                if saw_alignment {
                    return Err(error(item, instruction.span, "duplicate MMIO alignment"));
                }
                alignment = value
                    .parse::<u64>()
                    .ok()
                    .filter(|alignment| *alignment > 0 && alignment.is_power_of_two())
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "MMIO alignment must be a positive power of two",
                        )
                    })?;
                saw_alignment = true;
            } else {
                return Err(error(item, instruction.span, "unknown MMIO option"));
            }
        }
    }
    match instruction.opcode.as_str() {
        "mmio.load" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "mmio.load requires one SSA result",
                ));
            };
            let prefix = format!("%{result} = mmio.load ");
            let (ty, pointer) = body
                .strip_prefix(&prefix)
                .and_then(|body| body.split_once(", "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "mmio.load requires a type and pointer",
                    )
                })?;
            let ty = crate::parse_type(ty.trim()).map_err(|issue| {
                error(
                    item,
                    instruction.span,
                    format!("invalid MMIO type: {issue}"),
                )
            })?;
            Ok(DeviceIo::Load {
                result: result.clone(),
                ty,
                pointer: parse_name(pointer, item, instruction.span)?,
                alignment,
                order,
            })
        }
        "mmio.store" => {
            if !instruction.results.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "mmio.store cannot define an SSA result",
                ));
            }
            let (value, pointer) = body
                .strip_prefix("mmio.store ")
                .and_then(|body| body.split_once(", "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "mmio.store requires a value and pointer",
                    )
                })?;
            Ok(DeviceIo::Store {
                value: parse_name(value, item, instruction.span)?,
                pointer: parse_name(pointer, item, instruction.span)?,
                alignment,
                order,
            })
        }
        _ => Err(error(
            item,
            instruction.span,
            "unsupported device I/O opcode",
        )),
    }
}

fn parse_name(value: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    value
        .trim()
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(ToOwned::to_owned)
        .ok_or_else(|| error(item, span, "device I/O operand must be an SSA name"))
}

pub(crate) fn supported_type(ty: &Type) -> bool {
    match ty {
        Type::Integer(width) => matches!(width, 8 | 16 | 32 | 64),
        Type::Float(float) => matches!(float.bits(), 16 | 32 | 64),
        _ => false,
    }
}

pub(crate) fn validate(
    operation: &DeviceIo,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    match operation {
        DeviceIo::Load {
            result,
            ty,
            pointer,
            ..
        } => {
            if !supported_type(ty) || !matches!(values.get(pointer), Some(Type::Pointer(_))) {
                return Err(error(
                    item,
                    span,
                    "mmio.load requires a 1, 2, 4 or 8-byte scalar and raw pointer",
                ));
            }
            Ok(Some((result.clone(), ty.clone())))
        }
        DeviceIo::Store { value, pointer, .. } => {
            if !values.get(value).is_some_and(supported_type)
                || !matches!(values.get(pointer), Some(Type::Pointer(_)))
            {
                return Err(error(
                    item,
                    span,
                    "mmio.store requires a 1, 2, 4 or 8-byte scalar and raw pointer",
                ));
            }
            Ok(None)
        }
        DeviceIo::Fence { .. } => Ok(None),
    }
}

pub(crate) fn encode(
    operation: &DeviceIo,
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
            .ok_or_else(|| error(item, span, "undefined device I/O SSA operand"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        DeviceIo::Load {
            ty,
            pointer,
            alignment,
            order,
            ..
        } => {
            let ty = crate::text_conversion_assembly::target_index(types, ty)
                .ok_or_else(|| error(item, span, "MMIO result type was not interned"))?;
            write(0x270)?;
            write(order << 20)?;
            write(1)?;
            write(ty as u64)?;
            write(index(pointer)?)?;
            write(*alignment)?;
        }
        DeviceIo::Store {
            value,
            pointer,
            alignment,
            order,
        } => {
            write(0x271)?;
            write(order << 20)?;
            write(0)?;
            write(index(value)?)?;
            write(index(pointer)?)?;
            write(*alignment)?;
        }
        DeviceIo::Fence { selector } => {
            write(0x272)?;
            write(selector << 48)?;
            write(0)?;
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    operation: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let selector = (operation.modifiers >> 48) & 15;
    let order = (operation.modifiers >> 20) & 7;
    match (operation.opcode, operation.operands.as_slice()) {
        (
            0x270,
            [
                BinaryOperand::Type(ty),
                BinaryOperand::Value(pointer),
                BinaryOperand::Index(alignment),
            ],
        ) => {
            if operation.result_count != 1
                || selector != 0
                || order > 2
                || operation.modifiers & !(0x7 << 20) != 0
                || !valid_alignment(*alignment)
            {
                return Err(malformed("invalid mmio.load flags, order, or alignment"));
            }
            let (_, pointer_type) = get_value(values, *pointer)?;
            if !valid_pointer(types, pointer_type) || !valid_scalar(types, *ty) {
                return Err(malformed("invalid mmio.load pointer or scalar type"));
            }
            let (pointer, _) = get_value(values, *pointer)?;
            let result = format!("v{}", values.len());
            let ty_name = crate::text_straight_line_disassembly::constant_type_name(types, *ty)?;
            write!(text, "    %{result} = mmio.load {ty_name}, %{pointer}")
                .map_err(formatting_error)?;
            format_order_alignment(text, order, *alignment, true)?;
            text.push('\n');
            values.push((result, *ty));
        }
        (
            0x271,
            [
                BinaryOperand::Values(arguments),
                BinaryOperand::Index(alignment),
            ],
        ) if arguments.len() == 2 => {
            if operation.result_count != 0
                || selector != 0
                || !matches!(order, 0 | 1 | 3)
                || operation.modifiers & !(0x7 << 20) != 0
                || !valid_alignment(*alignment)
            {
                return Err(malformed("invalid mmio.store flags, order, or alignment"));
            }
            let (value, ty) = get_value(values, arguments[0])?;
            let (pointer, pointer_ty) = get_value(values, arguments[1])?;
            if !valid_scalar(types, ty) || !valid_pointer(types, pointer_ty) {
                return Err(malformed("invalid mmio.store scalar or pointer type"));
            }
            write!(text, "    mmio.store %{value}, %{pointer}").map_err(formatting_error)?;
            format_order_alignment(text, order, *alignment, false)?;
            text.push('\n');
        }
        (0x272, []) => {
            if operation.result_count != 0
                || operation.modifiers & !(0xf << 48) != 0
                || selector > 2
            {
                return Err(malformed("invalid fence.io flags"));
            }
            match selector {
                0 => text.push_str("    fence.io\n"),
                1 => text.push_str("    fence.io [write]\n"),
                _ => text.push_str("    fence.io [full]\n"),
            }
        }
        _ => return Err(malformed("MMIO CODE operands do not match the opcode")),
    }
    Ok(())
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("MMIO operand SSA index is invalid"))
}

fn valid_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Pointer(_))
    )
}

pub(crate) fn valid_scalar(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Integer(8 | 16 | 32 | 64))
    ) || matches!(types.entries.get(index as usize),
            Some(BinaryType::Float(float)) if matches!(float.bits(), 16 | 32 | 64))
}

pub(crate) fn valid_alignment(alignment: u64) -> bool {
    alignment == 0 || alignment.is_power_of_two()
}

fn format_order_alignment(
    text: &mut String,
    order: u64,
    alignment: u64,
    load: bool,
) -> Result<(), FormatError> {
    match (load, order) {
        (_, 0) => {}
        (_, 1) => text.push_str(" [order relaxed]"),
        (true, 2) => text.push_str(" [order acquire]"),
        (false, 3) => text.push_str(" [order release]"),
        _ => return Err(malformed("MMIO ordering is invalid")),
    }
    if alignment != 0 {
        write!(text, " [align {alignment}]").map_err(formatting_error)?;
    }
    Ok(())
}

fn formatting_error(error: std::fmt::Error) -> FormatError {
    malformed(error.to_string())
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
