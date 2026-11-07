use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::leb::IntegerImmediate;
use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, StringTable, TextInstruction, TextItem, TextParseError};

pub(crate) enum SimpleOperation {
    Assume(String),
    Nop,
    Counter(u64),
    PageSize(String),
    Feature {
        result: String,
        name: String,
    },
    Expect {
        result: String,
        source: String,
        expected: IntegerImmediate,
        probability: Option<u32>,
    },
    Intrinsic {
        id: u64,
        result: Option<String>,
    },
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "assume" | "nop" | "prof.count" | "target.pagesize" | "target.has" | "expect" | "intrinsic"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<SimpleOperation, TextParseError> {
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid simple CODE source span"))?
        .trim();
    match instruction.opcode.as_str() {
        "assume" => {
            if !instruction.results.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "assume cannot define an SSA result",
                ));
            }
            let name = line
                .strip_prefix("assume %")
                .filter(|name| valid_name(name))
                .ok_or_else(|| error(item, instruction.span, "assume requires one SSA operand"))?;
            Ok(SimpleOperation::Assume(name.to_owned()))
        }
        "nop" if line == "nop" && instruction.results.is_empty() => Ok(SimpleOperation::Nop),
        "prof.count" => {
            if !instruction.results.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "profile counter cannot define a result",
                ));
            }
            let id = line
                .strip_prefix("prof.count ")
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "prof.count requires one unsigned ID",
                    )
                })?;
            Ok(SimpleOperation::Counter(id))
        }
        "target.pagesize" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "target.pagesize requires one SSA result",
                ));
            };
            if line != format!("%{result} = target.pagesize") {
                return Err(error(
                    item,
                    instruction.span,
                    "target.pagesize has no operands",
                ));
            }
            Ok(SimpleOperation::PageSize(result.clone()))
        }
        "target.has" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "target.has requires an SSA result",
                ));
            };
            let prefix = format!("%{result} = target.has \"");
            let contents = line
                .strip_prefix(&prefix)
                .and_then(|value| value.strip_suffix('"'))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "target.has requires a quoted feature",
                    )
                })?;
            let name =
                crate::text::decode_kira_string_literal_contents(contents).map_err(|issue| {
                    error(
                        item,
                        instruction.span,
                        format!("invalid feature string: {issue:?}"),
                    )
                })?;
            if name.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "target feature name cannot be empty",
                ));
            }
            Ok(SimpleOperation::Feature {
                result: result.clone(),
                name,
            })
        }
        "expect" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "expect requires one SSA result",
                ));
            };
            let prefix = format!("%{result} = expect %");
            let (source, remainder) = line
                .strip_prefix(&prefix)
                .and_then(|rest| rest.split_once(", "))
                .ok_or_else(|| {
                    error(item, instruction.span, "expect requires `%value, integer`")
                })?;
            if !valid_name(source) {
                return Err(error(
                    item,
                    instruction.span,
                    "expect source must be an SSA name",
                ));
            }
            let (literal, probability) =
                if let Some((literal, probability)) = remainder.split_once(" [probability ") {
                    let probability = probability
                        .strip_suffix(']')
                        .and_then(|value| value.parse::<f32>().ok())
                        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                        .ok_or_else(|| {
                            error(
                                item,
                                instruction.span,
                                "expect probability must be finite and in [0,1]",
                            )
                        })?;
                    (literal, Some(probability.to_bits()))
                } else {
                    (remainder, None)
                };
            let negative = literal.starts_with('-');
            let unsigned = literal
                .strip_prefix('-')
                .or_else(|| literal.strip_prefix('+'))
                .unwrap_or(literal);
            let magnitude = crate::numeric_literals::parse_unsigned_integer(unsigned)
                .filter(|value| !negative || (*value != 0 && *value <= (1u128 << 127)))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "expect integer immediate is invalid",
                    )
                })?;
            Ok(SimpleOperation::Expect {
                result: result.clone(),
                source: source.to_owned(),
                expected: IntegerImmediate {
                    negative,
                    magnitude,
                },
                probability,
            })
        }
        "intrinsic" => {
            let (id, result) = if let [result] = instruction.results.as_slice() {
                let prefix = format!("%{result} = intrinsic ");
                let id = line
                    .strip_prefix(&prefix)
                    .and_then(|value| value.strip_suffix("() -> i64"))
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|id| matches!(id, 12 | 13))
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "only result-producing intrinsic 12/13 is supported",
                        )
                    })?;
                (id, Some(result.clone()))
            } else {
                let id = line
                    .strip_prefix("intrinsic ")
                    .and_then(|value| value.strip_suffix("() -> unit"))
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|id| matches!(id, 11 | 14))
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "only unit intrinsic 11/14 is supported",
                        )
                    })?;
                (id, None)
            };
            Ok(SimpleOperation::Intrinsic { id, result })
        }
        _ => Err(error(
            item,
            instruction.span,
            "malformed simple CODE instruction",
        )),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
}

pub(crate) fn validate(
    operation: &SimpleOperation,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    match operation {
        SimpleOperation::Assume(name) => {
            if values.get(name) != Some(&Type::Integer(1)) {
                return Err(error(
                    item,
                    span,
                    "assume requires a previously defined i1 value",
                ));
            }
            Ok(None)
        }
        SimpleOperation::Nop | SimpleOperation::Counter(_) => Ok(None),
        SimpleOperation::PageSize(name) => Ok(Some((name.clone(), Type::Integer(64)))),
        SimpleOperation::Feature { result, .. } => Ok(Some((result.clone(), Type::Integer(1)))),
        SimpleOperation::Expect {
            result,
            source,
            expected,
            ..
        } => {
            let Some(ty @ Type::Integer(bits)) = values.get(source) else {
                return Err(error(item, span, "expect requires an integer SSA operand"));
            };
            let literal = if expected.negative {
                format!("-{}", expected.magnitude)
            } else {
                expected.magnitude.to_string()
            };
            if !crate::numeric_literals::integer_literal_fits(&literal, *bits) {
                return Err(error(
                    item,
                    span,
                    "expect integer does not fit operand type",
                ));
            }
            Ok(Some((result.clone(), ty.clone())))
        }
        SimpleOperation::Intrinsic { result, .. } => Ok(result
            .as_ref()
            .map(|name| (name.clone(), Type::Integer(64)))),
    }
}

pub(crate) fn encode(
    operation: &SimpleOperation,
    values: &[&str],
    bytes: &mut Vec<u8>,
    feature_index: Option<u64>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let mut write =
        |value| crate::leb::write_uleb(value, bytes).map_err(|e| error(item, span, e.to_string()));
    match operation {
        SimpleOperation::Assume(name) => {
            let position = values
                .iter()
                .position(|value| *value == name)
                .ok_or_else(|| error(item, span, "assume operand is undefined"))?;
            write(0x243)?;
            write(0)?;
            write(0)?;
            write((values.len() - position) as u64)?;
        }
        SimpleOperation::Nop => {
            write(0x245)?;
            write(0)?;
            write(0)?;
        }
        SimpleOperation::Counter(id) => {
            write(0x246)?;
            write(0)?;
            write(0)?;
            write(*id)?;
        }
        SimpleOperation::PageSize(_) => {
            write(0x224)?;
            write(0)?;
            write(1)?;
        }
        SimpleOperation::Feature { .. } => {
            let index = feature_index
                .ok_or_else(|| error(item, span, "target feature missing from STRINGS"))?;
            write(0x223)?;
            write(0)?;
            write(1)?;
            write(index)?;
        }
        SimpleOperation::Expect {
            source,
            expected,
            probability,
            ..
        } => {
            let position = values
                .iter()
                .position(|name| *name == source)
                .ok_or_else(|| error(item, span, "expect operand is undefined"))?;
            write(0x244)?;
            write(0)?;
            write(1)?;
            write((values.len() - position) as u64)?;
            drop(write);
            crate::leb::write_integer_immediate(expected.negative, expected.magnitude, bytes)
                .map_err(|issue| error(item, span, issue.to_string()))?;
            crate::leb::write_uleb(u64::from(probability.is_some()), bytes)
                .map_err(|issue| error(item, span, issue.to_string()))?;
            if let Some(bits) = probability {
                bytes.extend_from_slice(&bits.to_le_bytes());
            }
        }
        SimpleOperation::Intrinsic { id, result } => {
            write(0x230)?;
            write(0)?;
            write(u64::from(result.is_some()))?;
            write(*id)?;
            write(0)?;
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    strings: &StringTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.modifiers != 0
        || instruction.result_count
            != u64::from(
                matches!(instruction.opcode, 0x223 | 0x224 | 0x244)
                    || instruction.opcode == 0x230
                        && matches!(
                            instruction.operands.as_slice(),
                            [BinaryOperand::Index(12 | 13)]
                        ),
            )
    {
        return Err(malformed("invalid simple CODE modifiers or result count"));
    }
    let value_name = |index: u64| -> Result<String, FormatError> {
        usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index))
            .map(|(name, _)| name.clone())
            .ok_or_else(|| malformed("invalid simple CODE SSA reference"))
    };
    match instruction.opcode {
        0x243 => {
            let [BinaryOperand::Value(index)] = instruction.operands.as_slice() else {
                return Err(malformed("assume requires one SSA value"));
            };
            let name = value_name(*index)?;
            writeln!(text, "    assume %{name}").map_err(formatting_error)?;
        }
        0x245 if instruction.operands.is_empty() => text.push_str("    nop\n"),
        0x246 => {
            let [BinaryOperand::Index(id)] = instruction.operands.as_slice() else {
                return Err(malformed("prof.count requires an ID"));
            };
            writeln!(text, "    prof.count {id}").map_err(formatting_error)?;
        }
        0x224 if instruction.operands.is_empty() => {
            let ty = types
                .entries
                .iter()
                .position(|ty| matches!(ty, BinaryType::Integer(64)))
                .ok_or_else(|| malformed("target.pagesize i64 result type is not interned"))?;
            let name = format!("v{}", values.len());
            writeln!(text, "    %{name} = target.pagesize").map_err(formatting_error)?;
            values.push((name, ty as u64));
        }
        0x223 => {
            let [BinaryOperand::Index(index)] = instruction.operands.as_slice() else {
                return Err(malformed("target.has requires a STRINGS index"));
            };
            let feature = usize::try_from(*index)
                .ok()
                .and_then(|index| strings.get(index))
                .filter(|feature| !feature.is_empty())
                .ok_or_else(|| malformed("target.has feature name is invalid"))?;
            let ty = types
                .entries
                .iter()
                .position(|ty| matches!(ty, BinaryType::Integer(1)))
                .ok_or_else(|| malformed("target.has requires an interned i1 type"))?;
            let name = format!("v{}", values.len());
            let literal = crate::text_global_disassembly::quote(feature)?;
            writeln!(text, "    %{name} = target.has {literal}").map_err(formatting_error)?;
            values.push((name, ty as u64));
        }
        0x244 => {
            let [
                BinaryOperand::Value(source),
                BinaryOperand::Integer(integer),
                BinaryOperand::OptionalFloat32(probability),
            ] = instruction.operands.as_slice()
            else {
                return Err(malformed(
                    "expect requires an SSA value, integer and optional probability",
                ));
            };
            let name = value_name(*source)?;
            let ty = values
                .get(*source as usize)
                .map(|(_, ty)| *ty)
                .ok_or_else(|| malformed("expect input is undefined"))?;
            if !matches!(types.entries.get(ty as usize), Some(BinaryType::Integer(_))) {
                return Err(malformed("expect operand is not integer typed"));
            }
            let immediate = if integer.negative {
                format!("-{}", integer.magnitude)
            } else {
                integer.magnitude.to_string()
            };
            let suffix = match probability {
                Some(bits) => {
                    let value = f32::from_bits(*bits);
                    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                        return Err(malformed("expect probability is invalid"));
                    }
                    format!(" [probability {value}]")
                }
                None => String::new(),
            };
            let result = format!("v{}", values.len());
            writeln!(text, "    %{result} = expect %{name}, {immediate}{suffix}")
                .map_err(formatting_error)?;
            values.push((result, ty));
        }
        0x230 => {
            let [BinaryOperand::Index(id)] = instruction.operands.as_slice() else {
                return Err(malformed("zero-argument intrinsic must contain its ID"));
            };
            match id {
                11 | 14 => {
                    writeln!(text, "    intrinsic {id}() -> unit").map_err(formatting_error)?;
                }
                12 | 13 => {
                    let ty = types
                        .entries
                        .iter()
                        .position(|ty| matches!(ty, BinaryType::Integer(64)))
                        .ok_or_else(|| malformed("counter intrinsic requires i64 result type"))?;
                    let name = format!("v{}", values.len());
                    writeln!(text, "    %{name} = intrinsic {id}() -> i64")
                        .map_err(formatting_error)?;
                    values.push((name, ty as u64));
                }
                _ => {
                    return Err(malformed(
                        "intrinsic is outside the supported zero-argument subset",
                    ));
                }
            }
        }
        _ => return Err(malformed("unsupported simple CODE operands")),
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
