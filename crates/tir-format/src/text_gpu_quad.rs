//! GPU derivative and quad collective operations (CODE 0x420–0x426).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{FloatType, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const FINE: u64 = 1 << 36;
const UNCHECKED_UNIFORMITY: u64 = 1 << 34;
const SELECTOR_SHIFT: u64 = 48;

pub(crate) struct QuadOp {
    result: String,
    opcode: u64,
    modifiers: u64,
    operands: Vec<String>,
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "gpu.ddx"
            | "gpu.ddy"
            | "gpu.fwidth"
            | "quad.broadcast"
            | "quad.swap"
            | "quad.all"
            | "quad.any"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<QuadOp, TextParseError> {
    let opcode = match instruction.opcode.as_str() {
        "gpu.ddx" => 0x420,
        "gpu.ddy" => 0x421,
        "gpu.fwidth" => 0x422,
        "quad.broadcast" => 0x423,
        "quad.swap" => 0x424,
        "quad.all" => 0x425,
        "quad.any" => 0x426,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "expected GPU derivative or quad opcode",
            ));
        }
    };
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "GPU derivative/quad requires one SSA result",
        ));
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "GPU quad source span is invalid"))?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let input = text
        .strip_prefix(&prefix)
        .ok_or_else(|| error(item, instruction.span, "GPU quad instruction is malformed"))?;
    let operand_end = input.find(" [").unwrap_or(input.len());
    let argument_text = &input[..operand_end];
    let arguments = argument_text.split(',').map(str::trim).collect::<Vec<_>>();
    let count = if opcode == 0x423 { 2 } else { 1 };
    if arguments.len() != count
        || arguments
            .iter()
            .any(|argument| !argument.starts_with('%') || !valid_name(&argument[1..]))
    {
        return Err(error(
            item,
            instruction.span,
            "GPU derivative/quad arguments must be comma-separated SSA names",
        ));
    }
    let mut rest = input[operand_end..].trim();
    let mut seen_selector = false;
    let mut seen_uniformity = false;
    let mut modifiers = 0u64;
    while !rest.is_empty() {
        let (clause, remainder) = rest
            .strip_prefix('[')
            .and_then(|text| text.split_once(']'))
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "GPU quad modifier clause must be bracketed",
                )
            })?;
        let clause = clause.trim();
        if clause == "unchecked_uniformity" {
            if seen_uniformity {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate unchecked_uniformity modifier",
                ));
            }
            seen_uniformity = true;
            modifiers |= UNCHECKED_UNIFORMITY;
        } else {
            if seen_selector {
                return Err(error(
                    item,
                    instruction.span,
                    "duplicate GPU derivative/quad selector",
                ));
            }
            seen_selector = true;
            match (opcode, clause) {
                (0x420 | 0x421, "coarse") | (0x424, "x") => {}
                (0x420 | 0x421, "fine") => modifiers |= FINE,
                (0x424, "y") => modifiers |= 1 << SELECTOR_SHIFT,
                (0x424, "diagonal") => modifiers |= 2 << SELECTOR_SHIFT,
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "unsupported GPU quad mode or selector",
                    ));
                }
            }
        }
        rest = remainder.trim();
    }
    Ok(QuadOp {
        result: result.clone(),
        opcode,
        modifiers,
        operands: arguments
            .into_iter()
            .map(|value| value[1..].to_owned())
            .collect(),
    })
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    if values.contains_key(&op.result) {
        return Err(error(
            item,
            instruction.span,
            "GPU derivative/quad SSA result is duplicated",
        ));
    }
    let first = values.get(&op.operands[0]).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "GPU derivative/quad input SSA is undefined",
        )
    })?;
    let accepted = match op.opcode {
        0x420..=0x422 => float_type(first),
        0x425 | 0x426 => first == &Type::Integer(1),
        _ => gpu_scalar_vector(first),
    };
    if !accepted {
        return Err(error(
            item,
            instruction.span,
            "GPU derivative/quad input has unsupported type",
        ));
    }
    if op.opcode == 0x423 && values.get(&op.operands[1]) != Some(&Type::Integer(32)) {
        return Err(error(
            item,
            instruction.span,
            "quad.broadcast lane must be i32",
        ));
    }
    let result = if matches!(op.opcode, 0x425 | 0x426) {
        Type::Integer(1)
    } else {
        first.clone()
    };
    values.insert(op.result, result);
    Ok(())
}

fn float_type(ty: &Type) -> bool {
    match ty {
        Type::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64) => true,
        Type::FixedVector { element, .. } => float_type(element),
        _ => false,
    }
}

fn gpu_scalar_vector(ty: &Type) -> bool {
    matches!(ty, Type::Integer(1..=64))
        || float_type(ty)
        || matches!(ty, Type::FixedVector { element, .. }
            if matches!(element.as_ref(), Type::Integer(1..=64)))
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.'))
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &[&str],
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    for value in [op.opcode, op.modifiers, 1] {
        crate::leb::write_uleb(value, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    for operand in &op.operands {
        let index = values
            .iter()
            .position(|value| value == operand)
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "GPU derivative/quad input is undefined",
                )
            })?;
        crate::leb::write_uleb((values.len() - index) as u64, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let name = match instruction.opcode {
        0x420 => "gpu.ddx",
        0x421 => "gpu.ddy",
        0x422 => "gpu.fwidth",
        0x423 => "quad.broadcast",
        0x424 => "quad.swap",
        0x425 => "quad.all",
        0x426 => "quad.any",
        _ => return Err(malformed("unsupported GPU quad opcode")),
    };
    let allowed = UNCHECKED_UNIFORMITY
        | match instruction.opcode {
            0x420 | 0x421 => FINE,
            0x424 => 3 << SELECTOR_SHIFT,
            _ => 0,
        };
    if instruction.modifiers & !allowed != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "GPU derivative/quad CODE modifiers or results are invalid",
        ));
    }
    let count = if instruction.opcode == 0x423 { 2 } else { 1 };
    let indices = match instruction.operands.as_slice() {
        [BinaryOperand::Value(value)] if count == 1 => vec![*value],
        [BinaryOperand::Values(values)] if count == 2 && values.len() == 2 => values.clone(),
        _ => return Err(malformed("GPU quad CODE SSA operand count is invalid")),
    };
    let operands = indices
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.as_str(), *ty))
                .ok_or_else(|| malformed("GPU quad CODE refers to undefined SSA value"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let input_type = types
        .entries
        .get(operands[0].1 as usize)
        .ok_or_else(|| malformed("GPU quad operand TYPES index is invalid"))?;
    let supported = match instruction.opcode {
        0x420..=0x422 => binary_float(types, input_type),
        0x425 | 0x426 => matches!(input_type, BinaryType::Integer(1)),
        _ => binary_numeric(types, input_type),
    };
    if !supported
        || (instruction.opcode == 0x423
            && !matches!(
                types.entries.get(operands[1].1 as usize),
                Some(BinaryType::Integer(32))
            ))
    {
        return Err(malformed(
            "GPU quad CODE has mismatched input or lane types",
        ));
    }
    let name_of_result = format!("v{}", values.len());
    write!(text, "    %{name_of_result} = {name} %{}", operands[0].0)
        .map_err(|issue| malformed(issue.to_string()))?;
    if count == 2 {
        write!(text, ", %{}", operands[1].0).map_err(|issue| malformed(issue.to_string()))?;
    }
    let selector = (instruction.modifiers >> SELECTOR_SHIFT) & 3;
    if instruction.opcode == 0x424 {
        let mode = match selector {
            0 => None,
            1 => Some("y"),
            2 => Some("diagonal"),
            _ => return Err(malformed("quad.swap selector exceeds diagonal")),
        };
        if let Some(mode) = mode {
            write!(text, " [{mode}]").map_err(|issue| malformed(issue.to_string()))?;
        }
    } else if selector != 0 {
        return Err(malformed("unexpected quad selector"));
    }
    if instruction.modifiers & FINE != 0 {
        write!(text, " [fine]").map_err(|issue| malformed(issue.to_string()))?;
    }
    if instruction.modifiers & UNCHECKED_UNIFORMITY != 0 {
        text.push_str(" [unchecked_uniformity]");
    }
    text.push('\n');
    let ty = if matches!(instruction.opcode, 0x425 | 0x426) {
        types
            .entries
            .iter()
            .position(|ty| matches!(ty, BinaryType::Integer(1)))
            .ok_or_else(|| malformed("quad predicate i1 result type not interned"))? as u64
    } else {
        operands[0].1
    };
    values.push((name_of_result, ty));
    Ok(())
}

fn binary_float(types: &BinaryTypeTable, ty: &BinaryType) -> bool {
    match ty {
        BinaryType::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64) => {
            true
        }
        BinaryType::FixedVector { element, .. } => types
            .entries
            .get(*element as usize)
            .is_some_and(|ty| binary_float(types, ty)),
        _ => false,
    }
}
fn binary_numeric(types: &BinaryTypeTable, ty: &BinaryType) -> bool {
    binary_float(types, ty)
        || matches!(ty, BinaryType::Integer(1..=64))
        || matches!(ty, BinaryType::FixedVector { element, .. }
            if matches!(types.entries.get(*element as usize), Some(BinaryType::Integer(1..=64))))
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
