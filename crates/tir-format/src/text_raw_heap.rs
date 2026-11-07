//! Unmanaged heap CODE subset: allocate, reallocate, and free generic raw pointers.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum RawHeap {
    Alloc {
        result: String,
        size: String,
        align: String,
        uninit: bool,
    },
    Realloc {
        result: String,
        pointer: String,
        size: String,
        align: String,
    },
    Free {
        pointer: String,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(
        opcode,
        "heap.alloc.raw" | "heap.realloc.raw" | "heap.free.raw"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<RawHeap, TextParseError> {
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "raw heap source span is invalid"))?
        .trim();
    match instruction.opcode.as_str() {
        "heap.alloc.raw" | "heap.realloc.raw" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "raw heap allocation must define one SSA result",
                ));
            };
            let prefix = format!("%{result} = {} ", instruction.opcode);
            let args = line.strip_prefix(&prefix).ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "raw heap instruction spelling is invalid",
                )
            })?;
            let (args, uninit) = match args.strip_suffix(" [uninit]") {
                Some(args) => (args, true),
                None => (args, false),
            };
            let args = args
                .split(',')
                .map(|arg| parse_name(arg.trim(), item, instruction.span))
                .collect::<Result<Vec<_>, _>>()?;
            match (instruction.opcode.as_str(), args.as_slice()) {
                ("heap.alloc.raw", [size, align]) => Ok(RawHeap::Alloc {
                    result: result.clone(),
                    size: size.clone(),
                    align: align.clone(),
                    uninit,
                }),
                ("heap.realloc.raw", [pointer, size, align]) if !uninit => Ok(RawHeap::Realloc {
                    result: result.clone(),
                    pointer: pointer.clone(),
                    size: size.clone(),
                    align: align.clone(),
                }),
                _ => Err(error(
                    item,
                    instruction.span,
                    "raw heap instruction has wrong argument count or mode",
                )),
            }
        }
        "heap.free.raw" => {
            if !instruction.results.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "heap.free.raw does not define an SSA result",
                ));
            }
            let pointer = line
                .strip_prefix("heap.free.raw ")
                .ok_or_else(|| error(item, instruction.span, "heap.free.raw requires a pointer"))?;
            Ok(RawHeap::Free {
                pointer: parse_name(pointer, item, instruction.span)?,
            })
        }
        _ => Err(error(item, instruction.span, "unsupported raw heap opcode")),
    }
}

fn parse_name(value: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    value
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(ToOwned::to_owned)
        .ok_or_else(|| error(item, span, "raw heap operand must be an SSA value"))
}

pub(crate) fn validate(
    operation: &RawHeap,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let int64 = |name: &str| values.get(name) == Some(&Type::Integer(64));
    let pointer = |name: &str| values.get(name) == Some(&Type::Pointer(AddressSpace::Generic));
    match operation {
        RawHeap::Alloc {
            result,
            size,
            align,
            ..
        } => {
            if !int64(size) || !int64(align) {
                return Err(error(
                    item,
                    span,
                    "heap.alloc.raw requires i64 size and alignment",
                ));
            }
            Ok(Some((result.clone(), Type::Pointer(AddressSpace::Generic))))
        }
        RawHeap::Realloc {
            result,
            pointer: source,
            size,
            align,
        } => {
            if !pointer(source) || !int64(size) || !int64(align) {
                return Err(error(
                    item,
                    span,
                    "heap.realloc.raw requires generic ptr, i64 size, and i64 alignment",
                ));
            }
            Ok(Some((result.clone(), Type::Pointer(AddressSpace::Generic))))
        }
        RawHeap::Free { pointer: source } => {
            if !pointer(source) {
                return Err(error(
                    item,
                    span,
                    "heap.free.raw requires a generic pointer",
                ));
            }
            Ok(None)
        }
    }
}

pub(crate) fn encode(
    operation: &RawHeap,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index = |name: &str| {
        values
            .iter()
            .position(|value| *value == name)
            .map(|position| (values.len() - position) as u64)
            .ok_or_else(|| error(item, span, "raw heap SSA operand is undefined"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        RawHeap::Alloc {
            size,
            align,
            uninit,
            ..
        } => {
            write(0x164)?;
            write(if *uninit { 1 << 30 } else { 0 })?;
            write(1)?;
            write(index(size)?)?;
            write(index(align)?)?;
        }
        RawHeap::Realloc {
            pointer,
            size,
            align,
            ..
        } => {
            write(0x165)?;
            write(0)?;
            write(1)?;
            for name in [pointer, size, align] {
                write(index(name)?)?;
            }
        }
        RawHeap::Free { pointer } => {
            write(0x166)?;
            write(0)?;
            write(0)?;
            write(index(pointer)?)?;
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let (opcode, args, result) = match (instruction.opcode, instruction.operands.as_slice()) {
        (0x164, [BinaryOperand::Values(args)])
            if args.len() == 2 && instruction.result_count == 1 =>
        {
            ("heap.alloc.raw", args.as_slice(), true)
        }
        (0x165, [BinaryOperand::Values(args)])
            if args.len() == 3 && instruction.result_count == 1 =>
        {
            ("heap.realloc.raw", args.as_slice(), true)
        }
        (0x166, [BinaryOperand::Value(value)]) if instruction.result_count == 0 => {
            let (name, ty) = get_value(values, *value)?;
            if !is_generic_pointer(types, ty) {
                return Err(malformed("heap.free.raw requires generic pointer"));
            }
            writeln!(text, "    heap.free.raw %{name}").map_err(formatting_error)?;
            if instruction.modifiers != 0 {
                return Err(malformed("heap.free.raw has unsupported modifiers"));
            }
            return Ok(());
        }
        _ => return Err(malformed("raw heap binary instruction shape is invalid")),
    };
    if instruction.modifiers & !(1 << 30) != 0
        || (instruction.opcode != 0x164 && instruction.modifiers != 0)
    {
        return Err(malformed("raw heap instruction has unsupported modifiers"));
    }
    let names = args
        .iter()
        .map(|index| get_value(values, *index))
        .collect::<Result<Vec<_>, _>>()?;
    let valid = match instruction.opcode {
        0x164 => names.iter().all(|(_, ty)| is_i64(types, *ty)),
        0x165 => {
            is_generic_pointer(types, names[0].1)
                && names[1..].iter().all(|(_, ty)| is_i64(types, *ty))
        }
        _ => false,
    };
    if !valid {
        return Err(malformed("raw heap SSA operand types are incompatible"));
    }
    let new_name = format!("v{}", values.len());
    let arguments = names
        .iter()
        .map(|(name, _)| format!("%{name}"))
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if instruction.modifiers & (1 << 30) != 0 {
        " [uninit]"
    } else {
        ""
    };
    writeln!(text, "    %{new_name} = {opcode} {arguments}{suffix}").map_err(formatting_error)?;
    if result {
        let ty = types
            .entries
            .iter()
            .position(|ty| matches!(ty, BinaryType::Pointer(0)))
            .ok_or_else(|| malformed("generic ptr result type is not interned"))?;
        values.push((new_name, ty as u64));
    }
    Ok(())
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("raw heap SSA index is invalid"))
}
fn is_i64(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Integer(64))
    )
}
fn is_generic_pointer(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Pointer(0))
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
