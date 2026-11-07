//! Restricted trivial tuple construction and extraction in CODE (0x104/0x105).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum TupleOp {
    New {
        result: String,
        args: Vec<String>,
    },
    Extract {
        result: String,
        tuple: String,
        index: u64,
    },
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(name, "tuple.new" | "tuple.extract")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<TupleOp, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "tuple instruction requires one SSA result",
        ));
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "tuple source span is invalid"))?
        .trim();
    match instruction.opcode.as_str() {
        "tuple.new" => {
            let prefix = format!("%{result} = tuple.new (");
            let contents = text
                .strip_prefix(&prefix)
                .and_then(|source| source.strip_suffix(')'))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "tuple.new requires parenthesized SSA arguments",
                    )
                })?;
            let args = if contents.trim().is_empty() {
                Vec::new()
            } else {
                contents
                    .split(',')
                    .map(|argument| {
                        let name = argument
                            .trim()
                            .strip_prefix('%')
                            .filter(|name| valid_name(name))
                            .ok_or_else(|| {
                                error(
                                    item,
                                    instruction.span,
                                    "tuple.new arguments must be SSA values",
                                )
                            })?;
                        Ok(name.to_owned())
                    })
                    .collect::<Result<Vec<_>, TextParseError>>()?
            };
            Ok(TupleOp::New {
                result: result.clone(),
                args,
            })
        }
        "tuple.extract" => {
            let prefix = format!("%{result} = tuple.extract %");
            let (tuple, index) = text
                .strip_prefix(&prefix)
                .and_then(|line| line.split_once(", "))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "tuple.extract requires an SSA tuple and field index",
                    )
                })?;
            let index = index
                .parse::<u64>()
                .map_err(|_| error(item, instruction.span, "tuple field index must be unsigned"))?;
            if !valid_name(tuple) {
                return Err(error(item, instruction.span, "invalid tuple SSA value"));
            }
            Ok(TupleOp::Extract {
                result: result.clone(),
                tuple: tuple.to_owned(),
                index,
            })
        }
        _ => Err(error(item, instruction.span, "unknown tuple operation")),
    }
}

pub(crate) fn validate(
    operation: &TupleOp,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    match operation {
        TupleOp::New { result, args } => {
            let elements = args
                .iter()
                .map(|name| {
                    values.get(name).ok_or_else(|| {
                        error(item, span, "tuple.new references an undefined SSA value")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if elements.is_empty() || elements.iter().any(|ty| !trivial_scalar(ty)) {
                return Err(error(
                    item,
                    span,
                    "tuple.new currently requires one or more trivial scalar elements",
                ));
            }
            Ok((
                result.clone(),
                Type::Tuple(elements.into_iter().cloned().collect()),
            ))
        }
        TupleOp::Extract {
            result,
            tuple,
            index,
        } => {
            let Some(Type::Tuple(elements)) = values.get(tuple) else {
                return Err(error(
                    item,
                    span,
                    "tuple.extract requires a tuple SSA value",
                ));
            };
            let ty = usize::try_from(*index)
                .ok()
                .and_then(|index| elements.get(index))
                .filter(|ty| trivial_scalar(ty))
                .ok_or_else(|| error(item, span, "tuple field is missing or nontrivial"))?;
            Ok((result.clone(), ty.clone()))
        }
    }
}

fn trivial_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_) | Type::Float(_) | Type::Pointer(_))
}

pub(crate) fn encode(
    operation: &TupleOp,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let mut write = |index: u64| {
        crate::leb::write_uleb(index, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        TupleOp::New { args, .. } => {
            write(0x104)?;
            write(0)?;
            write(1)?;
            write(args.len() as u64)?;
            for arg in args {
                let pos = values
                    .iter()
                    .position(|name| *name == arg)
                    .ok_or_else(|| error(item, span, "undefined tuple construction operand"))?;
                write((values.len() - pos) as u64)?;
            }
        }
        TupleOp::Extract { tuple, index, .. } => {
            let pos = values
                .iter()
                .position(|name| *name == tuple)
                .ok_or_else(|| error(item, span, "undefined tuple value"))?;
            write(0x105)?;
            write(0)?;
            write(1)?;
            write((values.len() - pos) as u64)?;
            write(*index)?;
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
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "tuple operation has invalid modifiers or result count",
        ));
    }
    let name = format!("v{}", values.len());
    let result_type = match instruction.opcode {
        0x104 => {
            let [BinaryOperand::Values(indices)] = instruction.operands.as_slice() else {
                return Err(malformed("tuple.new requires a counted SSA value list"));
            };
            let fields = indices
                .iter()
                .map(|value| get_value(values, *value))
                .collect::<Result<Vec<_>, _>>()?;
            if fields.is_empty() || fields.iter().any(|(_, ty)| !trivial_binary(types, *ty)) {
                return Err(malformed("tuple.new contains unsupported field types"));
            }
            let type_index = types.entries.iter().position(|entry| {
                matches!(entry, BinaryType::Tuple(elements)
                    if elements.as_slice() == fields.iter().map(|(_, ty)| *ty).collect::<Vec<_>>().as_slice())
            }).ok_or_else(|| malformed("tuple result type is not interned"))?;
            writeln!(
                text,
                "    %{name} = tuple.new ({})",
                fields
                    .iter()
                    .map(|(name, _)| format!("%{name}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .map_err(|error| malformed(error.to_string()))?;
            type_index as u64
        }
        0x105 => {
            let [BinaryOperand::Value(value), BinaryOperand::Index(index)] =
                instruction.operands.as_slice()
            else {
                return Err(malformed(
                    "tuple.extract requires one SSA value and field index",
                ));
            };
            let (source, ty) = get_value(values, *value)?;
            let Some(BinaryType::Tuple(fields)) = types.entries.get(ty as usize) else {
                return Err(malformed("tuple.extract input has no tuple type"));
            };
            let field = usize::try_from(*index)
                .ok()
                .and_then(|index| fields.get(index))
                .copied()
                .filter(|ty| trivial_binary(types, *ty))
                .ok_or_else(|| malformed("tuple.extract field type is missing or nontrivial"))?;
            writeln!(text, "    %{name} = tuple.extract %{source}, {index}")
                .map_err(|error| malformed(error.to_string()))?;
            field
        }
        _ => return Err(malformed("unsupported tuple opcode")),
    };
    values.push((name, result_type));
    Ok(())
}

fn get_value(values: &[(String, u64)], index: u64) -> Result<(&str, u64), FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("tuple SSA index is undefined"))
}

fn trivial_binary(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Integer(_) | BinaryType::Float(_) | BinaryType::Pointer(_))
    )
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
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
