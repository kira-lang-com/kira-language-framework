//! Direct fixed-vector CODE forms with specified operand arities and selectors.
//! Complex masked memory, shuffles and scalable-vector operations are separate.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct VectorOperation {
    pub(crate) opcode: u64,
    pub(crate) modifiers: u64,
    pub(crate) args: Vec<String>,
    pub(crate) ty: Option<Type>,
    pub(crate) result: String,
}

pub(crate) fn opcode(name: &str) -> Option<(u64, usize)> {
    Some(match name {
        "vec.splat" => (0x0c0, 1),
        "vec.extract" => (0x0c1, 2),
        "vec.insert" => (0x0c2, 3),
        "vec.step" => (0x0c4, 0),
        "vec.vscale" => (0x0c5, 0),
        "vec.compress" => (0x0cb, 3),
        "vec.expand" => (0x0cc, 3),
        "vec.dot" => (0x0cd, 3),
        "vec.zip" => (0x0ce, 2),
        "vec.unzip" => (0x0cf, 2),
        "vec.reverse" => (0x0d0, 1),
        "vec.slide" => (0x0d1, 3),
        "vec.concat" => (0x0d2, 2),
        "vec.split" => (0x0d3, 1),
        "vec.mask.active" => (0x0d4, 2),
        "vec.mask.count" => (0x0d5, 1),
        "vec.mask.first" => (0x0d6, 1),
        "vec.table" => (0x0d7, 2),
        _ => return None,
    })
}

pub(crate) fn name(code: u64) -> Option<&'static str> {
    Some(match code {
        0x0c0 => "vec.splat",
        0x0c1 => "vec.extract",
        0x0c2 => "vec.insert",
        0x0c4 => "vec.step",
        0x0c5 => "vec.vscale",
        0x0cb => "vec.compress",
        0x0cc => "vec.expand",
        0x0cd => "vec.dot",
        0x0ce => "vec.zip",
        0x0cf => "vec.unzip",
        0x0d0 => "vec.reverse",
        0x0d1 => "vec.slide",
        0x0d2 => "vec.concat",
        0x0d3 => "vec.split",
        0x0d4 => "vec.mask.active",
        0x0d5 => "vec.mask.count",
        0x0d6 => "vec.mask.first",
        0x0d7 => "vec.table",
        _ => return None,
    })
}

pub(crate) fn parse(
    op: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<VectorOperation, TextParseError> {
    let (code, arity) =
        opcode(&op.opcode).ok_or_else(|| error(item, op.span, "unsupported vector opcode"))?;
    let [result] = op.results.as_slice() else {
        return Err(error(
            item,
            op.span,
            "vector operation requires one SSA result",
        ));
    };
    let text = source
        .get(op.span.start..op.span.end)
        .ok_or_else(|| error(item, op.span, "invalid vector source span"))?
        .trim();
    let prefix = format!("%{result} = {}", op.opcode);
    let rest = text
        .strip_prefix(&prefix)
        .filter(|rest| rest.is_empty() || rest.starts_with(' '))
        .ok_or_else(|| error(item, op.span, "malformed vector opcode"))?
        .trim();
    let (rest, modifiers) = match code {
        0x0ce | 0x0d3 => strip_selector(rest, "lo", "hi"),
        0x0cf => strip_selector(rest, "even", "odd"),
        0x0d1 => strip_selector(rest, "up", "down"),
        0x0cd => {
            let (kind, operands) = rest
                .split_once(' ')
                .ok_or_else(|| error(item, op.span, "vec.dot requires an operator kind"))?;
            let selector = ["sdot", "udot", "usdot", "bfdot"]
                .iter()
                .position(|option| *option == kind)
                .ok_or_else(|| error(item, op.span, "vec.dot requires sdot/udot/usdot/bfdot"))?;
            (operands, (selector as u64) << 48)
        }
        _ => (rest, 0),
    };
    let ty =
        match code {
            0x0c4 => Some(
                crate::parse_type(rest)
                    .map_err(|e| error(item, op.span, format!("invalid vec.step type: {e}")))?,
            ),
            0x0c0 => {
                let (_, target) = rest
                    .split_once(" to ")
                    .ok_or_else(|| error(item, op.span, "vec.splat requires `to T`"))?;
                Some(crate::parse_type(target).map_err(|e| {
                    error(item, op.span, format!("invalid vector target type: {e}"))
                })?)
            }
            _ => None,
        };
    let args_source = match code {
        0x0c0 => rest.split_once(" to ").map(|(args, _)| args).unwrap_or(""),
        0x0c4 | 0x0c5 => "",
        _ => rest,
    };
    let args = if args_source.is_empty() {
        Vec::new()
    } else {
        args_source
            .split(',')
            .map(|argument| {
                let name = argument
                    .trim()
                    .strip_prefix('%')
                    .filter(|name| {
                        !name.is_empty()
                            && name
                                .chars()
                                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
                    })
                    .ok_or_else(|| error(item, op.span, "vector operand must be an SSA value"))?;
                Ok(name.to_owned())
            })
            .collect::<Result<Vec<_>, TextParseError>>()?
    };
    if args.len() != arity {
        return Err(error(
            item,
            op.span,
            "vector opcode operand count disagrees with specification",
        ));
    }
    Ok(VectorOperation {
        opcode: code,
        modifiers,
        args,
        ty,
        result: result.clone(),
    })
}

fn strip_selector<'a>(source: &'a str, first: &str, second: &str) -> (&'a str, u64) {
    let next = format!(" [{second}]");
    if let Some(body) = source.strip_suffix(&next) {
        return (body, 1 << 48);
    }
    let default = format!(" [{first}]");
    if let Some(body) = source.strip_suffix(&default) {
        return (body, 0);
    }
    (source, 0)
}

pub(crate) fn infer(
    op: &VectorOperation,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<Type, TextParseError> {
    let args = op
        .args
        .iter()
        .map(|arg| {
            values.get(arg).ok_or_else(|| {
                error(
                    item,
                    span,
                    "vector operand does not name a defined SSA value",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let integer = |ty: &Type| matches!(ty, Type::Integer(_));
    let numeric = |ty: &Type| matches!(ty, Type::Integer(_) | Type::Float(_));
    let expected = match op.opcode {
        0x0c0 => {
            let ty = op
                .ty
                .as_ref()
                .ok_or_else(|| error(item, span, "vec.splat target type missing"))?;
            let Some((lanes, element)) = fixed(ty) else {
                return Err(error(item, span, "vec.splat requires a fixed vector"));
            };
            if lanes == 0 || !numeric(element) || args[0] != element {
                return Err(error(
                    item,
                    span,
                    "vec.splat source must match vector element",
                ));
            }
            ty.clone()
        }
        0x0c4 => {
            let ty = op
                .ty
                .as_ref()
                .ok_or_else(|| error(item, span, "vec.step type missing"))?;
            if !matches!(fixed(ty), Some((lanes, element)) if lanes > 0 && integer(element)) {
                return Err(error(
                    item,
                    span,
                    "vec.step requires an integer fixed vector",
                ));
            }
            ty.clone()
        }
        0x0c5 => Type::Integer(64),
        0x0c1 | 0x0c2 => {
            let Some((_, element)) = fixed(args[0]) else {
                return Err(error(
                    item,
                    span,
                    "vector element operation requires fixed-vector input",
                ));
            };
            if !integer(args[1]) || (op.opcode == 0x0c2 && args[2] != element) {
                return Err(error(item, span, "vector index/element types disagree"));
            }
            if op.opcode == 0x0c1 {
                element.clone()
            } else {
                args[0].clone()
            }
        }
        0x0ce | 0x0cf | 0x0d0 => {
            let Some((_, element)) = fixed(args[0]) else {
                return Err(error(
                    item,
                    span,
                    "vector permutation requires fixed vectors",
                ));
            };
            if !numeric(element) || (args.len() == 2 && args[0] != args[1]) {
                return Err(error(item, span, "vector permutation operands disagree"));
            }
            args[0].clone()
        }
        0x0cb | 0x0cc => {
            let Some((lanes, element)) = fixed(args[0]) else {
                return Err(error(
                    item,
                    span,
                    "vec.compress/expand requires fixed vector input",
                ));
            };
            if !numeric(element)
                || args[2] != args[0]
                || args[1]
                    != &(Type::FixedVector {
                        lanes,
                        element: Box::new(Type::Integer(1)),
                    })
            {
                return Err(error(
                    item,
                    span,
                    "vec.compress/expand mask or passthrough type mismatches",
                ));
            }
            args[0].clone()
        }
        0x0cd => {
            let (Some((count, output)), Some((left, element)), Some((right, other))) =
                (fixed(args[0]), fixed(args[1]), fixed(args[2]))
            else {
                return Err(error(
                    item,
                    span,
                    "vec.dot needs three fixed vector operands",
                ));
            };
            let (input, accumulator, factor) = if op.modifiers >> 48 == 3 {
                (
                    Type::Float(tir_types::FloatType::Bf16),
                    Type::Float(tir_types::FloatType::F32),
                    2,
                )
            } else {
                (Type::Integer(8), Type::Integer(32), 4)
            };
            if output != &accumulator
                || element != &input
                || other != &input
                || count.checked_mul(factor) != Some(left)
                || right != left
            {
                return Err(error(
                    item,
                    span,
                    "vec.dot accumulator/input lanes or element types disagree",
                ));
            }
            args[0].clone()
        }
        0x0d1 => {
            if fixed(args[0]).is_none() || args[0] != args[1] || !integer(args[2]) {
                return Err(error(
                    item,
                    span,
                    "vec.slide needs matching fixed vectors and scalar integer offset",
                ));
            }
            args[0].clone()
        }
        0x0d7 => {
            let (Some((_, Type::Integer(8))), Some((lanes, index_ty))) =
                (fixed(args[0]), fixed(args[1]))
            else {
                return Err(error(
                    item,
                    span,
                    "vec.table requires byte table and integer index vectors",
                ));
            };
            if !integer(index_ty) {
                return Err(error(item, span, "vec.table indices must be integers"));
            }
            Type::FixedVector {
                lanes,
                element: Box::new(Type::Integer(8)),
            }
        }
        0x0d2 => {
            let (Some((a, element)), Some((b, other))) = (fixed(args[0]), fixed(args[1])) else {
                return Err(error(item, span, "vec.concat requires fixed vectors"));
            };
            if !numeric(element) || element != other {
                return Err(error(item, span, "vec.concat element types disagree"));
            }
            let lanes = a
                .checked_add(b)
                .ok_or_else(|| error(item, span, "vec.concat lane count overflow"))?;
            Type::FixedVector {
                lanes,
                element: Box::new(element.clone()),
            }
        }
        0x0d3 => {
            let Some((lanes, element)) = fixed(args[0]) else {
                return Err(error(item, span, "vec.split requires a fixed vector"));
            };
            if lanes < 2 || lanes % 2 != 0 || !numeric(element) {
                return Err(error(
                    item,
                    span,
                    "vec.split requires an even vector lane count",
                ));
            }
            Type::FixedVector {
                lanes: lanes / 2,
                element: Box::new(element.clone()),
            }
        }
        0x0d4 => {
            if args.len() != 2 || !integer(args[0]) || !integer(args[1]) {
                return Err(error(
                    item,
                    span,
                    "vec.mask.active requires two scalar integers",
                ));
            }
            Type::ScalableVector(Box::new(Type::Integer(1)))
        }
        0x0d5 | 0x0d6 => {
            if !matches!(args[0], Type::ScalableVector(element) if element.as_ref() == &Type::Integer(1))
                && !matches!(fixed(args[0]), Some((_, Type::Integer(1))))
            {
                return Err(error(
                    item,
                    span,
                    "mask operation requires fixed or scalable vector of i1",
                ));
            }
            Type::Integer(64)
        }
        _ => return Err(error(item, span, "unsupported vector opcode")),
    };
    Ok(expected)
}

fn fixed(ty: &Type) -> Option<(u32, &Type)> {
    match ty {
        Type::FixedVector { lanes, element } => Some((*lanes, element.as_ref())),
        _ => None,
    }
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let op = name(instruction.opcode).ok_or_else(|| malformed("unsupported vector opcode"))?;
    let (_, arity) = opcode(op).expect("known vector opcode");
    if instruction.result_count != 1 || instruction.modifiers & !(0xf << 48) != 0 {
        return Err(malformed("invalid vector result or modifier bits"));
    }
    let selector = instruction.modifiers >> 48;
    if (matches!(instruction.opcode, 0x0ce | 0x0cf | 0x0d1 | 0x0d3) && selector > 1)
        || (instruction.opcode == 0x0cd && selector > 3)
        || (!matches!(instruction.opcode, 0x0cd | 0x0ce | 0x0cf | 0x0d1 | 0x0d3) && selector != 0)
    {
        return Err(malformed("vector selector is invalid"));
    }
    let (args, ty) = match instruction.opcode {
        0x0c0 => {
            let [BinaryOperand::Value(value), BinaryOperand::Type(target)] =
                instruction.operands.as_slice()
            else {
                return Err(malformed("vec.splat expects one value and a vector type"));
            };
            (vec![*value], Some(*target))
        }
        0x0c4 => {
            let [BinaryOperand::Type(target)] = instruction.operands.as_slice() else {
                return Err(malformed("vec.step expects a vector type"));
            };
            (Vec::new(), Some(*target))
        }
        0x0c5 => {
            if !instruction.operands.is_empty() {
                return Err(malformed("vec.vscale has no operands"));
            }
            (Vec::new(), None)
        }
        _ => {
            let args = match instruction.operands.as_slice() {
                [BinaryOperand::Value(value)] if arity == 1 => vec![*value],
                [BinaryOperand::Values(args)] if args.len() == arity => args.clone(),
                _ => {
                    return Err(malformed(
                        "vector instruction operand count disagrees with opcode",
                    ));
                }
            };
            (args, None)
        }
    };
    let name = format!("v{}", values.len());
    let arg_names = args
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, _)| format!("%{name}"))
                .ok_or_else(|| malformed("vector operand index is invalid"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let suffix = match (instruction.opcode, selector) {
        (0x0ce | 0x0d3, 1) => " [hi]",
        (0x0cf, 1) => " [odd]",
        (0x0d1, 1) => " [down]",
        _ => "",
    };
    write!(text, "    %{name} = {op}").map_err(formatting_error)?;
    match instruction.opcode {
        0x0cd => {
            let kind = ["sdot", "udot", "usdot", "bfdot"]
                .get(selector as usize)
                .ok_or_else(|| malformed("invalid vec.dot kind"))?;
            write!(text, " {kind} {}", arg_names.join(", ")).map_err(formatting_error)?;
        }
        0x0c0 => {
            let ty = ty.expect("splat has target");
            let name_ty = crate::text_memory_disassembly::format_storage_type(types, ty)?;
            write!(text, " {} to {name_ty}", arg_names[0]).map_err(formatting_error)?;
        }
        0x0c4 => {
            let name_ty = crate::text_memory_disassembly::format_storage_type(
                types,
                ty.expect("step has target"),
            )?;
            write!(text, " {name_ty}").map_err(formatting_error)?;
        }
        _ => {
            if !arg_names.is_empty() {
                write!(text, " {}", arg_names.join(", ")).map_err(formatting_error)?;
            }
        }
    }
    writeln!(text, "{suffix}").map_err(formatting_error)?;
    let result_type = match instruction.opcode {
        0x0c0 | 0x0c4 => ty.expect("explicit vector result type"),
        0x0c5 | 0x0d5 | 0x0d6 => types
            .entries
            .iter()
            .position(|entry| matches!(entry, BinaryType::Integer(64)))
            .ok_or_else(|| malformed("i64 type is not interned for vector query"))?
            as u64,
        0x0d4 => {
            if args.iter().any(|index| {
                values.get(*index as usize).is_none_or(|(_, ty)| {
                    !matches!(
                        types.entries.get(*ty as usize),
                        Some(BinaryType::Integer(_))
                    )
                })
            }) {
                return Err(malformed(
                    "vec.mask.active operands must be scalar integers",
                ));
            }
            let i1 = types
                .entries
                .iter()
                .position(|ty| matches!(ty, BinaryType::Integer(1)))
                .ok_or_else(|| malformed("vec.mask.active result requires interned i1"))?
                as u64;
            types
                .entries
                .iter()
                .position(
                    |ty| matches!(ty, BinaryType::ScalableVector { element } if *element == i1),
                )
                .ok_or_else(|| malformed("vec.mask.active result requires interned svec<i1>"))?
                as u64
        }
        0x0c1 => {
            let input = values
                .get(args[0] as usize)
                .ok_or_else(|| malformed("invalid vector operand"))?
                .1;
            let Some(BinaryType::FixedVector { element, .. }) = types.entries.get(input as usize)
            else {
                return Err(malformed("vec.extract input is not a fixed vector"));
            };
            *element
        }
        0x0c2 | 0x0cb | 0x0cc | 0x0cd | 0x0ce | 0x0cf | 0x0d0 | 0x0d1 => {
            values
                .get(args[0] as usize)
                .ok_or_else(|| malformed("vector input is undefined"))?
                .1
        }
        0x0d2 | 0x0d3 => {
            let input = values
                .get(args[0] as usize)
                .ok_or_else(|| malformed("invalid vector operand"))?
                .1;
            let Some(BinaryType::FixedVector {
                lanes: first,
                element,
            }) = types.entries.get(input as usize)
            else {
                return Err(malformed("concat/split input must be fixed vector"));
            };
            let lanes = if instruction.opcode == 0x0d2 {
                let second = values
                    .get(args[1] as usize)
                    .ok_or_else(|| malformed("missing second vector"))?
                    .1;
                let Some(BinaryType::FixedVector {
                    lanes: other,
                    element: other_element,
                }) = types.entries.get(second as usize)
                else {
                    return Err(malformed("concat second input must be fixed vector"));
                };
                if other_element != element {
                    return Err(malformed("concat vector element types disagree"));
                }
                first
                    .checked_add(*other)
                    .ok_or_else(|| malformed("vector lane count overflows"))?
            } else {
                if *first < 2 || first % 2 != 0 {
                    return Err(malformed("vector split requires an even number of lanes"));
                }
                first / 2
            };
            types
                .entries
                .iter()
                .position(|entry| {
                    matches!(entry, BinaryType::FixedVector { lanes: n, element: e }
                    if *n == lanes && e == element)
                })
                .ok_or_else(|| malformed("vector concat/split result type is not interned"))?
                as u64
        }
        0x0d7 => {
            let input = values
                .get(args[1] as usize)
                .ok_or_else(|| malformed("vec.table index vector is undefined"))?
                .1;
            let Some(BinaryType::FixedVector { lanes, element }) =
                types.entries.get(input as usize)
            else {
                return Err(malformed("vec.table indices must be fixed vector"));
            };
            if !matches!(
                types.entries.get(*element as usize),
                Some(BinaryType::Integer(_))
            ) {
                return Err(malformed("vec.table indices must be integers"));
            }
            let i8_type = types
                .entries
                .iter()
                .position(|entry| matches!(entry, BinaryType::Integer(8)))
                .ok_or_else(|| malformed("i8 is not interned for vector table"))?
                as u64;
            types
                .entries
                .iter()
                .position(|entry| {
                    matches!(entry,
                BinaryType::FixedVector { lanes: actual, element: ty }
                    if actual == lanes && *ty == i8_type)
                })
                .ok_or_else(|| malformed("vec.table result vector is not interned"))?
                as u64
        }
        _ => return Err(malformed("unsupported vector result shape")),
    };
    let index = result_type;
    values.push((name, index as u64));
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
