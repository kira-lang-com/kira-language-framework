//! Typed CPU atomic load, store and fence encoding (TIR 15.6, 22.8).
//! GPU scopes, RMW, compare/exchange, wait and notify have separate gates.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum AtomicOp {
    Load {
        result: String,
        ty: Type,
        pointer: String,
        modifiers: u64,
    },
    Store {
        value: String,
        pointer: String,
        modifiers: u64,
    },
    Fence {
        modifiers: u64,
    },
}

pub(crate) fn handles(op: &str) -> bool {
    matches!(op, "atomic.load" | "atomic.store" | "fence")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<AtomicOp, TextParseError> {
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "atomic source span is invalid"))?
        .trim();
    let prefix = match (instruction.opcode.as_str(), instruction.results.as_slice()) {
        ("atomic.load", [result]) => format!("%{result} = atomic.load"),
        ("atomic.store", []) => "atomic.store".to_owned(),
        ("fence", []) => "fence".to_owned(),
        _ => {
            return Err(error(
                item,
                instruction.span,
                "atomic result count is invalid",
            ));
        }
    };
    let mut rest = line
        .strip_prefix(&prefix)
        .filter(|rest| rest.is_empty() || rest.starts_with(' '))
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "atomic instruction syntax is invalid",
            )
        })?
        .trim();
    let mut order = 0u64;
    let mut scope = 0u64;
    let mut has_order = false;
    let mut has_scope = false;
    while let Some(after_open) = rest.strip_prefix('[') {
        let (flag, trailing) = after_open
            .split_once(']')
            .ok_or_else(|| error(item, instruction.span, "unterminated atomic modifier"))?;
        if let Some(value) = flag.strip_prefix("order=") {
            if has_order {
                return Err(error(item, instruction.span, "duplicate atomic order"));
            }
            order = match value {
                "relaxed" => 1,
                "acquire" => 2,
                "release" => 3,
                "acq_rel" => 4,
                "seq_cst" => 5,
                _ => return Err(error(item, instruction.span, "invalid atomic memory order")),
            };
            has_order = true;
        } else if let Some(value) = flag.strip_prefix("scope=") {
            if has_scope {
                return Err(error(item, instruction.span, "duplicate atomic scope"));
            }
            scope = match value {
                "thread" => 1,
                "system" => 5,
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "supported CPU atomics require thread/system scope",
                    ));
                }
            };
            has_scope = true;
        } else {
            return Err(error(item, instruction.span, "unknown atomic modifier"));
        }
        if !trailing.is_empty() && !trailing.starts_with(' ') {
            return Err(error(
                item,
                instruction.span,
                "atomic modifiers must be separated by spaces",
            ));
        }
        rest = trailing.trim();
    }
    let modifiers = (order << 20) | (scope << 26);
    match instruction.opcode.as_str() {
        "atomic.load" => {
            if !matches!(order, 0 | 1 | 2 | 5) {
                return Err(error(
                    item,
                    instruction.span,
                    "atomic.load cannot have release/acq_rel order",
                ));
            }
            let (ty, pointer) = rest.split_once(", ").ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "atomic.load requires a type and pointer",
                )
            })?;
            let ty = crate::parse_type(ty.trim()).map_err(|issue| {
                error(
                    item,
                    instruction.span,
                    format!("invalid atomic type: {issue}"),
                )
            })?;
            Ok(AtomicOp::Load {
                result: instruction.results[0].clone(),
                ty,
                pointer: parse_name(pointer, item, instruction.span)?,
                modifiers,
            })
        }
        "atomic.store" => {
            if !matches!(order, 0 | 1 | 3 | 5) {
                return Err(error(
                    item,
                    instruction.span,
                    "atomic.store cannot have acquire/acq_rel order",
                ));
            }
            let (value, pointer) = rest.split_once(", ").ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "atomic.store requires value and pointer",
                )
            })?;
            Ok(AtomicOp::Store {
                value: parse_name(value, item, instruction.span)?,
                pointer: parse_name(pointer, item, instruction.span)?,
                modifiers,
            })
        }
        "fence" => {
            if !rest.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "fence accepts no SSA operands",
                ));
            }
            Ok(AtomicOp::Fence { modifiers })
        }
        _ => Err(error(item, instruction.span, "unsupported atomic opcode")),
    }
}

fn parse_name(name: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    name.trim()
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(ToOwned::to_owned)
        .ok_or_else(|| error(item, span, "atomic operand must be an SSA value"))
}

fn scalar(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => matches!(bits, 8 | 16 | 32 | 64 | 128),
        Type::Float(float) => matches!(float.bits(), 16 | 32 | 64),
        _ => false,
    }
}

pub(crate) fn validate(
    operation: &AtomicOp,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    match operation {
        AtomicOp::Load {
            result,
            ty,
            pointer,
            ..
        } => {
            if !scalar(ty) || !matches!(values.get(pointer), Some(Type::Pointer(_))) {
                return Err(error(
                    item,
                    span,
                    "atomic.load requires a fixed scalar type and raw pointer",
                ));
            }
            Ok(Some((result.clone(), ty.clone())))
        }
        AtomicOp::Store { value, pointer, .. } => {
            if !values.get(value).is_some_and(scalar)
                || !matches!(values.get(pointer), Some(Type::Pointer(_)))
            {
                return Err(error(
                    item,
                    span,
                    "atomic.store requires fixed scalar value and raw pointer",
                ));
            }
            Ok(None)
        }
        AtomicOp::Fence { .. } => Ok(None),
    }
}

pub(crate) fn encode(
    operation: &AtomicOp,
    values: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let value = |name: &str| {
        values
            .iter()
            .position(|value| *value == name)
            .map(|pos| (values.len() - pos) as u64)
            .ok_or_else(|| error(item, span, "atomic operand is undefined"))
    };
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        AtomicOp::Load {
            ty,
            pointer,
            modifiers,
            ..
        } => {
            let index = crate::text_conversion_assembly::target_index(types, ty)
                .ok_or_else(|| error(item, span, "atomic.load type is not interned"))?;
            write(0x200)?;
            write(*modifiers)?;
            write(1)?;
            write(index as u64)?;
            write(value(pointer)?)?;
        }
        AtomicOp::Store {
            value: stored,
            pointer,
            modifiers,
        } => {
            write(0x201)?;
            write(*modifiers)?;
            write(0)?;
            write(value(stored)?)?;
            write(value(pointer)?)?;
        }
        AtomicOp::Fence { modifiers } => {
            write(0x204)?;
            write(*modifiers)?;
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
    let modifiers = operation.modifiers;
    if modifiers & !((0x7 << 20) | (0x7 << 26)) != 0 {
        return Err(malformed("unsupported atomic modifier bits"));
    }
    let order = (modifiers >> 20) & 0x7;
    let scope = (modifiers >> 26) & 0x7;
    if !matches!(scope, 0 | 1 | 5) {
        return Err(malformed(
            "atomic scope cannot be represented in CPU text subset",
        ));
    }
    let order = match order {
        0 => "",
        1 => " [order=relaxed]",
        2 => " [order=acquire]",
        3 => " [order=release]",
        4 => " [order=acq_rel]",
        5 => " [order=seq_cst]",
        _ => return Err(malformed("atomic order is invalid")),
    };
    let scope = match scope {
        0 => "",
        1 => " [scope=thread]",
        5 => " [scope=system]",
        _ => unreachable!(),
    };
    match (operation.opcode, operation.operands.as_slice()) {
        (0x200, [BinaryOperand::Type(ty), BinaryOperand::Value(pointer)]) => {
            if operation.result_count != 1
                || !matches!((modifiers >> 20) & 7, 0 | 1 | 2 | 5)
                || !supported_binary(types, *ty)
            {
                return Err(malformed("atomic.load has invalid result, order or type"));
            }
            let (pointer, pointer_type) = get_value(values, *pointer)?;
            if !is_pointer(types, pointer_type) {
                return Err(malformed("atomic.load operand is not a pointer"));
            }
            let ty_name = crate::text_constant_type_name::constant_type_name(types, *ty)?;
            let result = format!("v{}", values.len());
            writeln!(
                text,
                "    %{result} = atomic.load{order}{scope} {ty_name}, %{pointer}"
            )
            .map_err(formatting_error)?;
            values.push((result, *ty));
        }
        (0x201, [BinaryOperand::Values(args)]) if args.len() == 2 => {
            if operation.result_count != 0 || !matches!((modifiers >> 20) & 7, 0 | 1 | 3 | 5) {
                return Err(malformed("atomic.store has invalid result or order"));
            }
            let (stored, ty) = get_value(values, args[0])?;
            let (pointer, pointer_type) = get_value(values, args[1])?;
            if !supported_binary(types, ty) || !is_pointer(types, pointer_type) {
                return Err(malformed("atomic.store type or pointer is invalid"));
            }
            writeln!(text, "    atomic.store{order}{scope} %{stored}, %{pointer}")
                .map_err(formatting_error)?;
        }
        (0x204, []) => {
            if operation.result_count != 0 {
                return Err(malformed("fence must not define SSA values"));
            }
            writeln!(text, "    fence{order}{scope}").map_err(formatting_error)?;
        }
        _ => return Err(malformed("atomic operands are not supported by text CODE")),
    }
    Ok(())
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("atomic SSA operand index is invalid"))
}
fn supported_binary(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Integer(8 | 16 | 32 | 64 | 128))
    ) || matches!(types.entries.get(index as usize),
            Some(BinaryType::Float(float)) if matches!(float.bits(), 16 | 32 | 64))
}
fn is_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Pointer(_))
    )
}
fn formatting_error(issue: std::fmt::Error) -> FormatError {
    malformed(issue.to_string())
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
