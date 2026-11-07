//! Trivial fixed-array aggregate construction and element access (0x107–0x109).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum ArrayOp {
    New {
        result: String,
        ty: Type,
        args: Vec<String>,
    },
    Extract {
        result: String,
        array: String,
        index: String,
    },
    Insert {
        result: String,
        array: String,
        index: String,
        element: String,
    },
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(name, "array.new" | "array.extract" | "array.insert")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<ArrayOp, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "array instruction must define one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "array instruction source span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let rest = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "array instruction spelling is invalid",
        )
    })?;
    match instruction.opcode.as_str() {
        "array.new" => {
            let (type_text, args) = rest.split_once('(').ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "array.new requires a fixed array type and arguments",
                )
            })?;
            let args = args.strip_suffix(')').ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "array.new argument list is not closed",
                )
            })?;
            let ty = crate::parse_type(type_text.trim()).map_err(|issue| {
                error(
                    item,
                    instruction.span,
                    format!("invalid array type: {issue}"),
                )
            })?;
            let args = if args.trim().is_empty() {
                Vec::new()
            } else {
                args.split(',')
                    .map(|arg| parse_name(arg.trim(), item, instruction.span))
                    .collect::<Result<Vec<_>, _>>()?
            };
            Ok(ArrayOp::New {
                result: result.clone(),
                ty,
                args,
            })
        }
        "array.extract" | "array.insert" => {
            let args = rest
                .split(',')
                .map(|arg| parse_name(arg.trim(), item, instruction.span))
                .collect::<Result<Vec<_>, _>>()?;
            match (instruction.opcode.as_str(), args.as_slice()) {
                ("array.extract", [array, index]) => Ok(ArrayOp::Extract {
                    result: result.clone(),
                    array: array.clone(),
                    index: index.clone(),
                }),
                ("array.insert", [array, index, element]) => Ok(ArrayOp::Insert {
                    result: result.clone(),
                    array: array.clone(),
                    index: index.clone(),
                    element: element.clone(),
                }),
                _ => Err(error(
                    item,
                    instruction.span,
                    "array extract/insert operand count is invalid",
                )),
            }
        }
        _ => Err(error(
            item,
            instruction.span,
            "unsupported array instruction",
        )),
    }
}

fn parse_name(raw: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    let name = raw
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .ok_or_else(|| error(item, span, "array operand must be a simple SSA name"))?;
    Ok(name.to_owned())
}

fn trivial(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_) | Type::Float(_) | Type::Pointer(_))
}

pub(crate) fn validate(
    operation: &ArrayOp,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    let get = |name: &String| {
        values
            .get(name)
            .ok_or_else(|| error(item, span, "array operation references an undefined value"))
    };
    match operation {
        ArrayOp::New { result, ty, args } => {
            let Type::Array { count, element } = ty else {
                return Err(error(item, span, "array.new requires a fixed array type"));
            };
            if *count > 1024 || *count as usize != args.len() || !trivial(element) {
                return Err(error(
                    item,
                    span,
                    "array.new requires exact element count and a trivial scalar element type",
                ));
            }
            for arg in args {
                if get(arg)? != element.as_ref() {
                    return Err(error(item, span, "array.new element has the wrong type"));
                }
            }
            Ok((result.clone(), ty.clone()))
        }
        ArrayOp::Extract {
            result,
            array,
            index,
        } => {
            let Type::Array { element, .. } = get(array)? else {
                return Err(error(
                    item,
                    span,
                    "array.extract requires a fixed array value",
                ));
            };
            if !trivial(element) || !matches!(get(index)?, Type::Integer(_)) {
                return Err(error(
                    item,
                    span,
                    "array.extract requires integer index and trivial element",
                ));
            }
            Ok((result.clone(), *element.clone()))
        }
        ArrayOp::Insert {
            result,
            array,
            index,
            element,
        } => {
            let ty @ Type::Array {
                element: ty_element,
                ..
            } = get(array)?
            else {
                return Err(error(item, span, "array.insert requires a fixed array"));
            };
            if !trivial(ty_element)
                || !matches!(get(index)?, Type::Integer(_))
                || get(element)? != ty_element.as_ref()
            {
                return Err(error(
                    item,
                    span,
                    "array.insert index/element types are incompatible",
                ));
            }
            Ok((result.clone(), ty.clone()))
        }
    }
}

pub(crate) fn encode(
    operation: &ArrayOp,
    values: &[&str],
    bytes: &mut Vec<u8>,
    types: &BinaryTypeTable,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    let mut ids = Vec::new();
    let (code, ty) = match operation {
        ArrayOp::New { ty, args, .. } => {
            ids.extend(args);
            let Type::Array { count, element } = ty else {
                unreachable!()
            };
            let elem = crate::text_conversion_assembly::target_index(types, element)
                .ok_or_else(|| error(item, span, "array scalar element type is not interned"))?
                as u64;
            let index = types.entries.iter().position(|entry|
                matches!(entry, BinaryType::Array { count: n, element: e } if n == count && *e == elem))
                .ok_or_else(|| error(item, span, "array result type is not interned"))?;
            (0x107, Some(index as u64))
        }
        ArrayOp::Extract { array, index, .. } => {
            ids.extend([array, index]);
            (0x108, None)
        }
        ArrayOp::Insert {
            array,
            index,
            element,
            ..
        } => {
            ids.extend([array, index, element]);
            (0x109, None)
        }
    };
    write(code)?;
    write(0)?;
    write(1)?;
    if let Some(ty) = ty {
        write(ty)?;
        write(ids.len() as u64)?;
    }
    for name in ids {
        let pos = values
            .iter()
            .position(|entry| *entry == name)
            .ok_or_else(|| error(item, span, "array operand is undefined"))?;
        write((values.len() - pos) as u64)?;
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
            "array operation has unsupported modifiers or result count",
        ));
    }
    let name = format!("v{}", values.len());
    let get = |index: u64| {
        usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index))
            .map(|(name, ty)| (name.as_str(), *ty))
            .ok_or_else(|| malformed("array operand SSA index is invalid"))
    };
    let result_type = match instruction.opcode {
        0x107 => {
            let [BinaryOperand::Type(array), BinaryOperand::Values(ids)] =
                instruction.operands.as_slice()
            else {
                return Err(malformed("array.new requires type and counted arguments"));
            };
            let Some(BinaryType::Array { count, element }) = types.entries.get(*array as usize)
            else {
                return Err(malformed("array.new result type is not an array"));
            };
            let ty = scalar_name(types, *element)?;
            if ids.len() as u64 != *count
                || *count > 1024
                || ids
                    .iter()
                    .any(|id| get(*id).map_or(true, |(_, actual)| actual != *element))
            {
                return Err(malformed("array.new elements do not match declared type"));
            }
            let args = ids
                .iter()
                .map(|id| get(*id).map(|(name, _)| format!("%{name}")))
                .collect::<Result<Vec<_>, _>>()?;
            writeln!(
                text,
                "    %{name} = array.new [{count} x {ty}]({})",
                args.join(", ")
            )
            .map_err(|issue| malformed(issue.to_string()))?;
            *array
        }
        0x108 | 0x109 => {
            let expected = if instruction.opcode == 0x108 { 2 } else { 3 };
            let ids = match instruction.operands.as_slice() {
                [BinaryOperand::Values(ids)] if ids.len() == expected => ids.clone(),
                _ => return Err(malformed("array extract/insert operand shape is invalid")),
            };
            let (source, array) = get(ids[0])?;
            let Some(BinaryType::Array { element, .. }) = types.entries.get(array as usize) else {
                return Err(malformed("array operation source must be an array"));
            };
            let (index, index_type) = get(ids[1])?;
            if !matches!(
                types.entries.get(index_type as usize),
                Some(BinaryType::Integer(_))
            ) {
                return Err(malformed("array index must have integer type"));
            }
            let result = if ids.len() == 3 {
                let (value, ty) = get(ids[2])?;
                if ty != *element {
                    return Err(malformed("array.insert element type does not match"));
                }
                writeln!(
                    text,
                    "    %{name} = array.insert %{source}, %{index}, %{value}"
                )
                .map_err(|issue| malformed(issue.to_string()))?;
                array
            } else {
                scalar_name(types, *element)?;
                writeln!(text, "    %{name} = array.extract %{source}, %{index}")
                    .map_err(|issue| malformed(issue.to_string()))?;
                *element
            };
            result
        }
        _ => return Err(malformed("unknown array opcode")),
    };
    values.push((name, result_type));
    Ok(())
}

fn scalar_name(types: &BinaryTypeTable, index: u64) -> Result<String, FormatError> {
    crate::text_straight_line_disassembly::constant_type_name(types, index)
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
