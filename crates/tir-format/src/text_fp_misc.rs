//! Additional IEEE floating-point CODE operations (0x075..=0x079).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum FpMisc {
    Class {
        result: String,
        source: String,
    },
    Frexp {
        result: String,
        source: String,
    },
    Ldexp {
        result: String,
        source: String,
        exponent: String,
    },
    FlagsRead {
        result: String,
    },
    FlagsClear,
}

pub(crate) fn handles(op: &str) -> bool {
    matches!(
        op,
        "fclass" | "frexp" | "ldexp" | "fp.flags.read" | "fp.flags.clear"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<FpMisc, TextParseError> {
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "FP instruction source span is invalid",
            )
        })?
        .trim();
    match instruction.opcode.as_str() {
        "fclass" | "frexp" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "floating-point operation requires one SSA result",
                ));
            };
            let prefix = format!("%{result} = {} ", instruction.opcode);
            let name = line.strip_prefix(&prefix).ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "floating-point operation is missing its source",
                )
            })?;
            let source = parse_name(name, item, instruction.span)?;
            if instruction.opcode == "fclass" {
                Ok(FpMisc::Class {
                    result: result.clone(),
                    source,
                })
            } else {
                Ok(FpMisc::Frexp {
                    result: result.clone(),
                    source,
                })
            }
        }
        "ldexp" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "ldexp requires one SSA result",
                ));
            };
            let prefix = format!("%{result} = ldexp ");
            let (source, exponent) = line
                .strip_prefix(&prefix)
                .and_then(|body| body.split_once(", "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "ldexp requires float and exponent operands",
                    )
                })?;
            Ok(FpMisc::Ldexp {
                result: result.clone(),
                source: parse_name(source, item, instruction.span)?,
                exponent: parse_name(exponent, item, instruction.span)?,
            })
        }
        "fp.flags.read" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "fp.flags.read requires one SSA result",
                ));
            };
            if line != format!("%{result} = fp.flags.read") {
                return Err(error(
                    item,
                    instruction.span,
                    "fp.flags.read accepts no operands",
                ));
            }
            Ok(FpMisc::FlagsRead {
                result: result.clone(),
            })
        }
        "fp.flags.clear" if line == "fp.flags.clear" && instruction.results.is_empty() => {
            Ok(FpMisc::FlagsClear)
        }
        _ => Err(error(
            item,
            instruction.span,
            "malformed floating-point status operation",
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
        .ok_or_else(|| error(item, span, "expected one floating-point SSA operand"))
}

pub(crate) fn validate(
    operation: &FpMisc,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let get_float = |name: &str| {
        values
            .get(name)
            .filter(|ty| matches!(ty, Type::Float(_)))
            .cloned()
            .ok_or_else(|| {
                error(
                    item,
                    span,
                    "FP operand must be a defined floating-point scalar",
                )
            })
    };
    Ok(match operation {
        FpMisc::Class { result, source } => {
            get_float(source)?;
            Some((result.clone(), Type::Integer(32)))
        }
        FpMisc::Frexp { result, source } => {
            let float = get_float(source)?;
            Some((result.clone(), Type::Tuple(vec![float, Type::Integer(32)])))
        }
        FpMisc::Ldexp {
            result,
            source,
            exponent,
        } => {
            let float = get_float(source)?;
            if values.get(exponent) != Some(&Type::Integer(32)) {
                return Err(error(item, span, "ldexp exponent must be an i32 SSA value"));
            }
            Some((result.clone(), float))
        }
        FpMisc::FlagsRead { result } => Some((result.clone(), Type::Integer(32))),
        FpMisc::FlagsClear => None,
    })
}

pub(crate) fn encode(
    operation: &FpMisc,
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
            .ok_or_else(|| error(item, span, "FP operand is undefined"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        FpMisc::Class { source, .. } => {
            write(0x075)?;
            write(0)?;
            write(1)?;
            write(index(source)?)?;
        }
        FpMisc::Frexp { source, .. } => {
            write(0x076)?;
            write(0)?;
            write(1)?;
            write(index(source)?)?;
        }
        FpMisc::Ldexp {
            source, exponent, ..
        } => {
            write(0x077)?;
            write(0)?;
            write(1)?;
            write(index(source)?)?;
            write(index(exponent)?)?;
        }
        FpMisc::FlagsRead { .. } => {
            write(0x078)?;
            write(0)?;
            write(1)?;
        }
        FpMisc::FlagsClear => {
            write(0x079)?;
            write(0)?;
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
    if operation.modifiers != 0 || operation.result_count != u64::from(operation.opcode != 0x079) {
        return Err(malformed(
            "FP special opcode has unsupported modifiers or result count",
        ));
    }
    let int32 = || {
        types
            .entries
            .iter()
            .position(|ty| matches!(ty, BinaryType::Integer(32)))
            .map(|index| index as u64)
            .ok_or_else(|| malformed("i32 type is not interned"))
    };
    match (operation.opcode, operation.operands.as_slice()) {
        (0x075 | 0x076, [BinaryOperand::Value(source)]) => {
            let (name, ty) = get_value(values, *source)?;
            if !matches!(types.entries.get(ty as usize), Some(BinaryType::Float(_))) {
                return Err(malformed(
                    "fclass/frexp requires floating-point scalar input",
                ));
            }
            let opcode = if operation.opcode == 0x075 {
                "fclass"
            } else {
                "frexp"
            };
            let result = format!("v{}", values.len());
            writeln!(text, "    %{result} = {opcode} %{name}").map_err(formatting_error)?;
            let result_ty = if operation.opcode == 0x075 {
                int32()?
            } else {
                let exp = int32()?;
                types
                    .entries
                    .iter()
                    .position(|entry| {
                        matches!(entry, BinaryType::Tuple(elements)
                    if elements.as_slice() == [ty, exp])
                    })
                    .map(|index| index as u64)
                    .ok_or_else(|| malformed("frexp tuple result type is not interned"))?
            };
            values.push((result, result_ty));
        }
        (0x077, [BinaryOperand::Values(args)]) if args.len() == 2 => {
            let (name, ty) = get_value(values, args[0])?;
            let (exp, exponent_ty) = get_value(values, args[1])?;
            if !matches!(types.entries.get(ty as usize), Some(BinaryType::Float(_)))
                || exponent_ty != int32()?
            {
                return Err(malformed(
                    "ldexp requires floating-point scalar and i32 exponent",
                ));
            }
            let result = format!("v{}", values.len());
            writeln!(text, "    %{result} = ldexp %{name}, %{exp}").map_err(formatting_error)?;
            values.push((result, ty));
        }
        (0x078, []) => {
            let result = format!("v{}", values.len());
            writeln!(text, "    %{result} = fp.flags.read").map_err(formatting_error)?;
            values.push((result, int32()?));
        }
        (0x079, []) => text.push_str("    fp.flags.clear\n"),
        _ => return Err(malformed("FP special opcode operands are malformed")),
    }
    Ok(())
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("FP operand SSA index is invalid"))
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
