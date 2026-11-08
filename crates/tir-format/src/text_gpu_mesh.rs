//! Mesh/object GPU CODE (0x4c0–0x4c4), typed SSA operands and bit-34 uniformity.
use std::collections::HashMap;
use std::fmt::Write as _;
use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const UNCHECKED_UNIFORMITY: u64 = 1 << 34;
const NAMES: [&str; 5] = [
    "object.dispatch_mesh",
    "mesh.set_counts",
    "mesh.write.vertex",
    "mesh.write.primitive",
    "mesh.write.indices",
];

struct MeshOp {
    opcode: u64,
    modifiers: u64,
    operands: Vec<String>,
}

pub(crate) fn handles(name: &str) -> bool {
    NAMES.contains(&name)
}

fn expected_count(opcode: u64) -> usize {
    if opcode == 0x4c0 { 3 } else { 2 }
}

fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<MeshOp, TextParseError> {
    let opcode = NAMES
        .iter()
        .position(|name| *name == instruction.opcode)
        .map(|index| 0x4c0 + index as u64)
        .ok_or_else(|| error(item, instruction.span, "expected mesh/object opcode"))?;
    if !instruction.results.is_empty() {
        return Err(error(
            item,
            instruction.span,
            "mesh/object operations cannot define SSA results",
        ));
    }
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .and_then(|line| line.trim().strip_prefix(&instruction.opcode))
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "invalid mesh/object instruction source",
            )
        })?;
    let line = line.trim();
    let (arguments, modifiers) = match line.find(" [") {
        Some(index) => {
            let suffix = line[index..].trim();
            if !matches!(opcode, 0x4c0 | 0x4c1) || suffix != "[unchecked_uniformity]" {
                return Err(error(
                    item,
                    instruction.span,
                    "invalid mesh modifier clause",
                ));
            }
            (&line[..index], UNCHECKED_UNIFORMITY)
        }
        None => (line, 0),
    };
    let operands = arguments
        .split(',')
        .map(str::trim)
        .map(|value| {
            value
                .strip_prefix('%')
                .filter(|name| valid_name(name))
                .map(str::to_owned)
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "mesh arguments must be SSA references",
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if operands.len() != expected_count(opcode) {
        return Err(error(
            item,
            instruction.span,
            "mesh/object CODE operand count is incorrect",
        ));
    }
    Ok(MeshOp {
        opcode,
        modifiers,
        operands,
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let operation = parse(instruction, source, item)?;
    let types = operation
        .operands
        .iter()
        .map(|name| {
            values
                .get(name)
                .ok_or_else(|| error(item, instruction.span, "mesh SSA operand is undefined"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let integer = |ty: &Type| matches!(ty, Type::Integer(1..=64));
    let indices = |ty: &Type| match ty {
        Type::Integer(32) => true,
        Type::FixedVector {
            lanes: 2 | 3,
            element,
        } => matches!(element.as_ref(), Type::Integer(32)),
        _ => false,
    };
    let valid = match operation.opcode {
        0x4c0 | 0x4c1 => types.iter().all(|ty| integer(ty)),
        0x4c2 | 0x4c3 => integer(types[0]),
        0x4c4 => integer(types[0]) && indices(types[1]),
        _ => false,
    };
    if !valid {
        return Err(error(
            item,
            instruction.span,
            "mesh operands have incompatible GPU types",
        ));
    }
    Ok(())
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &[&str],
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    for word in [op.opcode, op.modifiers, 0] {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    for name in &op.operands {
        let position = values
            .iter()
            .position(|value| *value == name)
            .ok_or_else(|| error(item, instruction.span, "undefined mesh SSA operand"))?;
        crate::leb::write_uleb((values.len() - position) as u64, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &[(String, u64)],
) -> Result<(), FormatError> {
    let name = instruction
        .opcode
        .checked_sub(0x4c0)
        .and_then(|index| NAMES.get(index as usize))
        .ok_or_else(|| malformed("invalid mesh opcode"))?;
    let allowed = if matches!(instruction.opcode, 0x4c0 | 0x4c1) {
        UNCHECKED_UNIFORMITY
    } else {
        0
    };
    if instruction.modifiers & !allowed != 0 || instruction.result_count != 0 {
        return Err(malformed("mesh CODE modifiers or result count are invalid"));
    }
    let [BinaryOperand::Values(indices)] = instruction.operands.as_slice() else {
        return Err(malformed("mesh CODE must contain an SSA value list"));
    };
    if indices.len() != expected_count(instruction.opcode) {
        return Err(malformed("mesh CODE SSA operand count is invalid"));
    }
    let operands = indices
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.as_str(), *ty))
                .ok_or_else(|| malformed("mesh CODE references undefined SSA operand"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ty = |index: usize| {
        operands
            .get(index)
            .and_then(|(_, ty)| types.entries.get(*ty as usize))
    };
    let integer = |ty: &BinaryType| matches!(ty, BinaryType::Integer(1..=64));
    let indices_valid = |ty: &BinaryType| {
        matches!(ty, BinaryType::Integer(32))
            || matches!(ty, BinaryType::FixedVector { lanes: 2 | 3, element }
            if matches!(types.entries.get(*element as usize), Some(BinaryType::Integer(32))))
    };
    let valid = match instruction.opcode {
        0x4c0 | 0x4c1 => (0..operands.len()).all(|index| ty(index).is_some_and(integer)),
        0x4c2 | 0x4c3 => ty(0).is_some_and(integer),
        0x4c4 => ty(0).is_some_and(integer) && ty(1).is_some_and(indices_valid),
        _ => false,
    };
    if !valid {
        return Err(malformed("mesh CODE operand TYPES are invalid"));
    }
    write!(text, "    {name}").map_err(|issue| malformed(issue.to_string()))?;
    for (position, (name, _)) in operands.iter().enumerate() {
        write!(text, "{}%{name}", if position == 0 { " " } else { ", " })
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if instruction.modifiers & UNCHECKED_UNIFORMITY != 0 {
        text.push_str(" [unchecked_uniformity]");
    }
    text.push('\n');
    Ok(())
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
