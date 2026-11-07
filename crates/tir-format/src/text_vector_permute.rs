use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const REDUCTIONS: [&str; 15] = [
    "add", "mul", "and", "or", "xor", "smin", "smax", "umin", "umax", "fadd", "fmul", "fmin",
    "fmax", "fminnum", "fmaxnum",
];
const MASK: u64 = 1 << 32;
const REASSOC: u64 = 1 << 12;
const SELECTOR: u64 = 0xf << 48;

pub(crate) enum VectorPermutation {
    Shuffle {
        result: String,
        left: String,
        right: String,
        indices: Vec<u64>,
    },
    Reduce {
        result: String,
        input: String,
        mask: Option<String>,
        selector: u64,
        reassoc: bool,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "vec.shuffle" | "vec.reduce")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<VectorPermutation, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "vector shuffle/reduce requires exactly one SSA result",
        ));
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "vector opcode text span is invalid"))?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let args = text.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "invalid vector shuffle/reduce syntax",
        )
    })?;
    if instruction.opcode == "vec.shuffle" {
        let (both, indices) = args.split_once(", [").ok_or_else(|| {
            error(
                item,
                instruction.span,
                "vec.shuffle requires a counted lane index list",
            )
        })?;
        let (left, right) = both.split_once(", ").ok_or_else(|| {
            error(
                item,
                instruction.span,
                "vec.shuffle requires two vector operands",
            )
        })?;
        let indices = indices.strip_suffix(']').ok_or_else(|| {
            error(
                item,
                instruction.span,
                "vec.shuffle lane indices have no closing bracket",
            )
        })?;
        let indices = indices
            .split(',')
            .map(|number| {
                let number = number.trim();
                if number.starts_with('-') || number.starts_with('+') {
                    return Err(error(
                        item,
                        instruction.span,
                        "shuffle index must be an unsigned literal",
                    ));
                }
                number.parse::<u64>().map_err(|_| {
                    error(item, instruction.span, "shuffle index is not a u64 literal")
                })
            })
            .collect::<Result<Vec<_>, TextParseError>>()?;
        if !valid_lanes(indices.len() as u32) {
            return Err(error(
                item,
                instruction.span,
                "shuffle result needs 2, 3, 4, 8, 16, 32, or 64 lanes",
            ));
        }
        Ok(VectorPermutation::Shuffle {
            result: result.clone(),
            left: name(left, item, instruction.span)?,
            right: name(right, item, instruction.span)?,
            indices,
        })
    } else if instruction.opcode == "vec.reduce" {
        let (args, reassoc) = if let Some(args) = args.strip_prefix("[reassoc] ") {
            (args, true)
        } else {
            (args, false)
        };
        let (operator, rest) = args.split_once(' ').ok_or_else(|| {
            error(
                item,
                instruction.span,
                "vec.reduce requires an operator and vector",
            )
        })?;
        let selector = REDUCTIONS
            .iter()
            .position(|name| *name == operator)
            .ok_or_else(|| error(item, instruction.span, "vec.reduce operator is unknown"))?
            as u64;
        if reassoc && !matches!(selector, 9 | 10) {
            return Err(error(
                item,
                instruction.span,
                "reassoc only applies to fadd/fmul reductions",
            ));
        }
        let (vector, mask) = if let Some((vector, mask)) = rest.split_once(" [mask ") {
            let mask = mask
                .strip_suffix(']')
                .ok_or_else(|| error(item, instruction.span, "vec.reduce mask is not closed"))?;
            (vector, Some(name(mask, item, instruction.span)?))
        } else {
            (rest, None)
        };
        Ok(VectorPermutation::Reduce {
            result: result.clone(),
            input: name(vector, item, instruction.span)?,
            mask,
            selector,
            reassoc,
        })
    } else {
        Err(error(
            item,
            instruction.span,
            "unsupported vector permutation opcode",
        ))
    }
}

pub(crate) fn infer(
    operation: &VectorPermutation,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    let source = |name: &str| {
        values
            .get(name)
            .ok_or_else(|| error(item, span, "vector SSA operand is undefined"))
    };
    match operation {
        VectorPermutation::Shuffle {
            result,
            left,
            right,
            indices,
        } => {
            let (a, b) = (source(left)?, source(right)?);
            let (Some((left_lanes, element)), Some((right_lanes, other))) = (fixed(a), fixed(b))
            else {
                return Err(error(
                    item,
                    span,
                    "vec.shuffle requires two fixed-vector operands",
                ));
            };
            let max = left_lanes
                .checked_add(right_lanes)
                .ok_or_else(|| error(item, span, "shuffle operand lane count overflow"))?;
            if element != other
                || !valid_lanes(indices.len() as u32)
                || indices.iter().any(|index| *index >= u64::from(max))
            {
                return Err(error(
                    item,
                    span,
                    "shuffle element types or lane indexes disagree",
                ));
            }
            Ok((
                result.clone(),
                Type::FixedVector {
                    lanes: indices.len() as u32,
                    element: Box::new(element.clone()),
                },
            ))
        }
        VectorPermutation::Reduce {
            result,
            input,
            mask,
            selector,
            ..
        } => {
            let Some((lanes, element)) = fixed(source(input)?) else {
                return Err(error(
                    item,
                    span,
                    "vec.reduce requires a fixed-vector operand",
                ));
            };
            let numeric = if *selector <= 8 {
                matches!(element, Type::Integer(_))
            } else {
                matches!(element, Type::Float(_))
            };
            if !numeric
                || mask.as_ref().is_some_and(|mask| {
                    source(mask).ok()
                        != Some(&Type::FixedVector {
                            lanes,
                            element: Box::new(Type::Integer(1)),
                        })
                })
            {
                return Err(error(
                    item,
                    span,
                    "reduction operator/element/mask types disagree",
                ));
            }
            Ok((result.clone(), element.clone()))
        }
    }
}

pub(crate) fn encode(
    operation: &VectorPermutation,
    names: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let delta = |value: &str| {
        names
            .iter()
            .position(|name| *name == value)
            .map(|position| (names.len() - position) as u64)
            .ok_or_else(|| error(item, span, "undefined vector operand"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        VectorPermutation::Shuffle {
            left,
            right,
            indices,
            ..
        } => {
            write(0x0c3)?;
            write(0)?;
            write(1)?;
            write(delta(left)?)?;
            write(delta(right)?)?;
            write(indices.len() as u64)?;
            for index in indices {
                crate::leb::write_integer_immediate(false, u128::from(*index), bytes)
                    .map_err(|issue| error(item, span, issue.to_string()))?;
            }
        }
        VectorPermutation::Reduce {
            input,
            mask,
            selector,
            reassoc,
            ..
        } => {
            let flags = (*selector << 48)
                | if mask.is_some() { MASK } else { 0 }
                | if *reassoc { REASSOC } else { 0 };
            write(0x0c6)?;
            write(flags)?;
            write(1)?;
            write(delta(input)?)?;
            if let Some(mask) = mask {
                write(delta(mask)?)?;
            }
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
    if instruction.result_count != 1 {
        return Err(malformed(
            "vector shuffle/reduce must have exactly one result",
        ));
    }
    let index = values.len();
    let name = |id: u64| {
        values
            .get(id as usize)
            .map(|(name, ty)| (name.as_str(), *ty))
            .ok_or_else(|| malformed("vector shuffle/reduce operand is undefined"))
    };
    let (code, result_type) = match instruction.opcode {
        0x0c3 => {
            if instruction.modifiers != 0 {
                return Err(malformed("vec.shuffle has unexpected modifier bits"));
            }
            let [
                BinaryOperand::Values(args),
                BinaryOperand::Integers(indices),
            ] = instruction.operands.as_slice()
            else {
                return Err(malformed("vec.shuffle binary operands are invalid"));
            };
            if args.len() != 2 || !valid_lanes(indices.len() as u32) {
                return Err(malformed("vec.shuffle operand/index counts are invalid"));
            }
            let (left, a) = name(args[0])?;
            let (right, b) = name(args[1])?;
            let (
                Some(BinaryType::FixedVector {
                    lanes: x,
                    element: e,
                }),
                Some(BinaryType::FixedVector {
                    lanes: y,
                    element: f,
                }),
            ) = (types.entries.get(a as usize), types.entries.get(b as usize))
            else {
                return Err(malformed("vec.shuffle inputs must be fixed vectors"));
            };
            let Some(max) = x.checked_add(*y) else {
                return Err(malformed("shuffle lane count overflow"));
            };
            if e != f
                || indices
                    .iter()
                    .any(|index| index.negative || index.magnitude >= u128::from(max))
            {
                return Err(malformed(
                    "vec.shuffle lane index or element types mismatch",
                ));
            }
            let output = types.entries.iter().position(|ty| matches!(ty,
                BinaryType::FixedVector { lanes, element } if *lanes == indices.len() as u64 && element == e))
                .ok_or_else(|| malformed("shuffle result type is not in TYPES"))?;
            let indices = indices
                .iter()
                .map(|index| index.magnitude.to_string())
                .collect::<Vec<_>>();
            (
                format!("vec.shuffle %{left}, %{right}, [{}]", indices.join(", ")),
                output as u64,
            )
        }
        0x0c6 => {
            if instruction.modifiers & !(SELECTOR | MASK | REASSOC) != 0 {
                return Err(malformed("vec.reduce has invalid modifier bits"));
            }
            let selector = instruction.modifiers >> 48;
            let Some(op) = REDUCTIONS.get(selector as usize) else {
                return Err(malformed("vec.reduce selector is invalid"));
            };
            let reassoc = instruction.modifiers & REASSOC != 0;
            if reassoc && !matches!(selector, 9 | 10) {
                return Err(malformed("reassoc only applies to fadd/fmul reductions"));
            }
            let masked = instruction.modifiers & MASK != 0;
            let input = match instruction.operands.as_slice() {
                [BinaryOperand::Value(input)] if !masked => (*input, None),
                [BinaryOperand::Value(input), BinaryOperand::Value(mask)] if masked => {
                    (*input, Some(*mask))
                }
                _ => return Err(malformed("vec.reduce operand shape is invalid")),
            };
            let (vector, vector_type) = name(input.0)?;
            let Some(BinaryType::FixedVector { lanes, element }) =
                types.entries.get(vector_type as usize)
            else {
                return Err(malformed("vec.reduce requires fixed vector input"));
            };
            if !matches!(types.entries.get(*element as usize),
                Some(BinaryType::Integer(_)) if selector <= 8)
                && !matches!(types.entries.get(*element as usize),
                    Some(BinaryType::Float(_)) if selector >= 9)
            {
                return Err(malformed(
                    "reduction operator does not match vector element type",
                ));
            }
            let mut suffix = String::new();
            if let Some(mask) = input.1 {
                let (mask_name, mask_type) = name(mask)?;
                let Some(BinaryType::FixedVector {
                    lanes: mask_lanes,
                    element: mask_element,
                }) = types.entries.get(mask_type as usize)
                else {
                    return Err(malformed("reduce mask is not a fixed vector"));
                };
                if mask_lanes != lanes
                    || !matches!(
                        types.entries.get(*mask_element as usize),
                        Some(BinaryType::Integer(1))
                    )
                {
                    return Err(malformed("reduce mask needs matching vec<N x i1> type"));
                }
                suffix.push_str(&format!(" [mask %{mask_name}]"));
            }
            let prefix = if reassoc { "[reassoc] " } else { "" };
            (
                format!("vec.reduce {prefix}{op} %{vector}{suffix}"),
                *element,
            )
        }
        _ => return Err(malformed("unexpected vector permutation opcode")),
    };
    writeln!(text, "    %v{index} = {code}").map_err(|issue| malformed(issue.to_string()))?;
    values.push((format!("v{index}"), result_type));
    Ok(())
}

fn fixed(ty: &Type) -> Option<(u32, &Type)> {
    if let Type::FixedVector { lanes, element } = ty {
        Some((*lanes, element))
    } else {
        None
    }
}
fn valid_lanes(lanes: u32) -> bool {
    matches!(lanes, 2 | 3 | 4 | 8 | 16 | 32 | 64)
}
fn name<'a>(value: &'a str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    value
        .strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(str::to_owned)
        .ok_or_else(|| error(item, span, "vector operand must be a named SSA value"))
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
