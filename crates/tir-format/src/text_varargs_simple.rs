//! A restricted scalar va_list and setjmp CODE path, opcodes 0x210–0x214.
//! Non-local-jump terminators, variadic calls and foreign ABI forwarding remain separate.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum Varargs {
    Start(String),
    Arg {
        result: String,
        ty: Type,
        list: String,
    },
    Copy {
        destination: String,
        source: String,
    },
    End(String),
    Setjmp {
        result: String,
        buffer: String,
    },
}

pub(crate) fn handles(op: &str) -> bool {
    matches!(op, "va.start" | "va.arg" | "va.copy" | "va.end" | "setjmp")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<Varargs, TextParseError> {
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid variadic source span"))?
        .trim();
    match instruction.opcode.as_str() {
        "va.start" | "va.end" => {
            if !instruction.results.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "va.start/va.end have no SSA result",
                ));
            }
            let value = line
                .strip_prefix(&format!("{} ", instruction.opcode))
                .ok_or_else(|| {
                    error(item, instruction.span, "va operation requires list operand")
                })?;
            let value = parse_name(value, item, instruction.span)?;
            if instruction.opcode == "va.start" {
                Ok(Varargs::Start(value))
            } else {
                Ok(Varargs::End(value))
            }
        }
        "va.arg" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "va.arg must define one SSA result",
                ));
            };
            let prefix = format!("%{result} = va.arg ");
            let (ty, list) = line
                .strip_prefix(&prefix)
                .and_then(|body| body.split_once(", "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "va.arg requires an explicit type and list",
                    )
                })?;
            let ty = crate::parse_type(ty.trim()).map_err(|issue| {
                error(
                    item,
                    instruction.span,
                    format!("invalid va.arg type: {issue}"),
                )
            })?;
            Ok(Varargs::Arg {
                result: result.clone(),
                ty,
                list: parse_name(list, item, instruction.span)?,
            })
        }
        "va.copy" => {
            if !instruction.results.is_empty() {
                return Err(error(item, instruction.span, "va.copy has no SSA result"));
            }
            let (destination, source) = line
                .strip_prefix("va.copy ")
                .and_then(|line| line.split_once(", "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "va.copy requires two list addresses",
                    )
                })?;
            Ok(Varargs::Copy {
                destination: parse_name(destination, item, instruction.span)?,
                source: parse_name(source, item, instruction.span)?,
            })
        }
        "setjmp" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "setjmp must define an i32 SSA result",
                ));
            };
            let prefix = format!("%{result} = setjmp ");
            let buffer = line
                .strip_prefix(&prefix)
                .ok_or_else(|| error(item, instruction.span, "setjmp buffer is missing"))?;
            Ok(Varargs::Setjmp {
                result: result.clone(),
                buffer: parse_name(buffer, item, instruction.span)?,
            })
        }
        _ => Err(error(item, instruction.span, "unsupported variadic opcode")),
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
        .ok_or_else(|| error(item, span, "variadic operand must name an SSA value"))
}

fn accepted_scalar(ty: &Type) -> bool {
    match ty {
        Type::Integer(bits) => matches!(bits, 8 | 16 | 32 | 64 | 128),
        Type::Float(float) => matches!(float.bits(), 32 | 64),
        Type::Pointer(_) => true,
        _ => false,
    }
}

pub(crate) fn validate(
    operation: &Varargs,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let ptr = |name: &str| values.get(name) == Some(&Type::Pointer(AddressSpace::Generic));
    match operation {
        Varargs::Start(name) | Varargs::End(name) if ptr(name) => Ok(None),
        Varargs::Copy {
            destination,
            source,
        } if ptr(destination) && ptr(source) => Ok(None),
        Varargs::Arg { result, ty, list } if ptr(list) && accepted_scalar(ty) => {
            Ok(Some((result.clone(), ty.clone())))
        }
        Varargs::Setjmp { result, buffer } if ptr(buffer) => {
            Ok(Some((result.clone(), Type::Integer(32))))
        }
        _ => Err(error(
            item,
            span,
            "variadic list must be generic ptr and result type must be supported",
        )),
    }
}

pub(crate) fn encode(
    operation: &Varargs,
    values: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index = |name: &str| {
        values
            .iter()
            .position(|value| *value == name)
            .map(|position| (values.len() - position) as u64)
            .ok_or_else(|| error(item, span, "undefined variadic SSA operand"))
    };
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        Varargs::Start(name) => {
            write(0x210)?;
            write(0)?;
            write(0)?;
            write(index(name)?)?;
        }
        Varargs::Arg { ty, list, .. } => {
            let ty = crate::text_conversion_assembly::target_index(types, ty)
                .ok_or_else(|| error(item, span, "va.arg result type is not interned"))?;
            write(0x211)?;
            write(0)?;
            write(1)?;
            write(ty as u64)?;
            write(index(list)?)?;
        }
        Varargs::Copy {
            destination,
            source,
        } => {
            write(0x212)?;
            write(0)?;
            write(0)?;
            write(index(destination)?)?;
            write(index(source)?)?;
        }
        Varargs::End(name) => {
            write(0x213)?;
            write(0)?;
            write(0)?;
            write(index(name)?)?;
        }
        Varargs::Setjmp { buffer, .. } => {
            write(0x214)?;
            write(0)?;
            write(1)?;
            write(index(buffer)?)?;
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
    if operation.modifiers != 0
        || operation.result_count != u64::from(matches!(operation.opcode, 0x211 | 0x214))
    {
        return Err(malformed(
            "variadic opcode has wrong modifiers or result count",
        ));
    }
    let scalar_type = match (operation.opcode, operation.operands.as_slice()) {
        (0x211, [BinaryOperand::Type(ty), BinaryOperand::Value(_)])
            if accepted_binary(types, *ty) =>
        {
            Some(*ty)
        }
        (0x214, [BinaryOperand::Value(_)]) => types
            .entries
            .iter()
            .position(|ty| matches!(ty, BinaryType::Integer(32)))
            .map(|index| index as u64),
        _ => None,
    };
    let ptr = |index| {
        usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index))
            .filter(|(_, ty)| {
                matches!(
                    types.entries.get(*ty as usize),
                    Some(BinaryType::Pointer(0))
                )
            })
            .map(|(name, _)| name.as_str())
            .ok_or_else(|| malformed("variadic operand must be generic ptr"))
    };
    match (operation.opcode, operation.operands.as_slice()) {
        (0x210, [BinaryOperand::Value(list)]) => {
            let list = ptr(*list)?;
            writeln!(text, "    va.start %{list}").map_err(formatting_error)?;
        }
        (0x211, [BinaryOperand::Type(ty), BinaryOperand::Value(list)]) => {
            let result_type = scalar_type.ok_or_else(|| malformed("invalid va.arg type"))?;
            let ty_name = crate::text_constant_type_name::constant_type_name(types, *ty)?;
            let list = ptr(*list)?;
            let name = format!("v{}", values.len());
            writeln!(text, "    %{name} = va.arg {ty_name}, %{list}").map_err(formatting_error)?;
            values.push((name, result_type));
        }
        (0x212, [BinaryOperand::Values(args)]) if args.len() == 2 => {
            let dest = ptr(args[0])?;
            let source = ptr(args[1])?;
            writeln!(text, "    va.copy %{dest}, %{source}").map_err(formatting_error)?;
        }
        (0x213, [BinaryOperand::Value(list)]) => {
            let list = ptr(*list)?;
            writeln!(text, "    va.end %{list}").map_err(formatting_error)?;
        }
        (0x214, [BinaryOperand::Value(buffer)]) => {
            let result_type =
                scalar_type.ok_or_else(|| malformed("setjmp i32 type is not interned"))?;
            let buffer = ptr(*buffer)?;
            let name = format!("v{}", values.len());
            writeln!(text, "    %{name} = setjmp %{buffer}").map_err(formatting_error)?;
            values.push((name, result_type));
        }
        _ => return Err(malformed("variadic CODE operand shape is invalid")),
    }
    Ok(())
}

fn accepted_binary(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Integer(8 | 16 | 32 | 64 | 128))
    ) || matches!(types.entries.get(index as usize), Some(BinaryType::Float(float)) if matches!(float.bits(), 32 | 64))
        || matches!(
            types.entries.get(index as usize),
            Some(BinaryType::Pointer(_))
        )
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
