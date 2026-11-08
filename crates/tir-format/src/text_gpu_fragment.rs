//! Fragment GPU instructions in the 0x405–0x409 CODE range.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, FloatType, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum FragmentOp {
    Demote,
    RasterBegin,
    RasterEnd,
    LoadAttachment { result: String, attachment: u32 },
    ImageblockAddress { result: String },
}

pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "frag.demote"
            | "frag.raster_order.begin"
            | "frag.raster_order.end"
            | "frag.load_attachment"
            | "frag.imageblock.addr"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<FragmentOp, TextParseError> {
    if !handles(&instruction.opcode) {
        return Err(error(
            item,
            instruction.span,
            "expected fragment GPU opcode",
        ));
    }
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid fragment instruction span"))?
        .trim();
    match instruction.opcode.as_str() {
        "frag.demote" | "frag.raster_order.begin" | "frag.raster_order.end" => {
            if text != instruction.opcode || !instruction.results.is_empty() {
                return Err(error(
                    item,
                    instruction.span,
                    "zero-operand fragment instruction must not define results or operands",
                ));
            }
            Ok(match instruction.opcode.as_str() {
                "frag.demote" => FragmentOp::Demote,
                "frag.raster_order.begin" => FragmentOp::RasterBegin,
                _ => FragmentOp::RasterEnd,
            })
        }
        "frag.load_attachment" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "frag.load_attachment requires one SSA result",
                ));
            };
            let prefix = format!("%{result} = frag.load_attachment ");
            let attachment = text
                .strip_prefix(&prefix)
                .and_then(|raw| crate::numeric_literals::parse_unsigned_integer(raw))
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "frag.load_attachment requires unsigned 32-bit attachment index",
                    )
                })?;
            Ok(FragmentOp::LoadAttachment {
                result: result.clone(),
                attachment,
            })
        }
        "frag.imageblock.addr" => {
            let [result] = instruction.results.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "frag.imageblock.addr requires one SSA result",
                ));
            };
            if text != format!("%{result} = frag.imageblock.addr") {
                return Err(error(
                    item,
                    instruction.span,
                    "frag.imageblock.addr takes no operands",
                ));
            }
            Ok(FragmentOp::ImageblockAddress {
                result: result.clone(),
            })
        }
        _ => unreachable!(),
    }
}

fn result_type(op: &FragmentOp) -> Option<(&str, Type)> {
    match op {
        FragmentOp::LoadAttachment { result, .. } => Some((
            result.as_str(),
            Type::FixedVector {
                lanes: 4,
                element: Box::new(Type::Float(FloatType::F32)),
            },
        )),
        FragmentOp::ImageblockAddress { result } => {
            Some((result.as_str(), Type::Pointer(AddressSpace::Tile)))
        }
        _ => None,
    }
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    if let Some((name, ty)) = result_type(&op) {
        if values.contains_key(name) {
            return Err(error(
                item,
                instruction.span,
                "duplicate fragment result SSA name",
            ));
        }
        values.insert(name.to_owned(), ty);
    }
    Ok(())
}

pub(crate) fn required_result_type(opcode: &str) -> Option<Type> {
    match opcode {
        "frag.load_attachment" => result_type(&FragmentOp::LoadAttachment {
            result: String::new(),
            attachment: 0,
        })
        .map(|(_, ty)| ty),
        "frag.imageblock.addr" => result_type(&FragmentOp::ImageblockAddress {
            result: String::new(),
        })
        .map(|(_, ty)| ty),
        _ => None,
    }
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    let (opcode, results) = match op {
        FragmentOp::Demote => (0x405, 0),
        FragmentOp::LoadAttachment { .. } => (0x406, 1),
        FragmentOp::RasterBegin => (0x407, 0),
        FragmentOp::RasterEnd => (0x408, 0),
        FragmentOp::ImageblockAddress { .. } => (0x409, 1),
    };
    for value in [opcode, 0, results] {
        crate::leb::write_uleb(value, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    if let FragmentOp::LoadAttachment { attachment, .. } = op {
        crate::leb::write_integer_immediate(false, u128::from(attachment), bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    operation: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let (opcode, result_ty) = match operation.opcode {
        0x405 => ("frag.demote", None),
        0x406 => (
            "frag.load_attachment",
            Some(required_result_type("frag.load_attachment").unwrap()),
        ),
        0x407 => ("frag.raster_order.begin", None),
        0x408 => ("frag.raster_order.end", None),
        0x409 => (
            "frag.imageblock.addr",
            Some(Type::Pointer(AddressSpace::Tile)),
        ),
        _ => return Err(malformed("unsupported fragment opcode")),
    };
    if operation.modifiers != 0 || operation.result_count != u64::from(result_ty.is_some()) {
        return Err(malformed(
            "fragment instruction has invalid modifiers or result count",
        ));
    }
    if operation.opcode == 0x406 {
        let [BinaryOperand::Integer(attachment)] = operation.operands.as_slice() else {
            return Err(malformed(
                "frag.load_attachment requires an integer immediate",
            ));
        };
        if attachment.negative || attachment.magnitude > u128::from(u32::MAX) {
            return Err(malformed("fragment attachment index must fit unsigned u32"));
        }
        let index = types.entries.iter().position(|candidate|
            matches!(candidate, BinaryType::FixedVector { lanes: 4, element }
                if matches!(types.entries.get(*element as usize), Some(BinaryType::Float(FloatType::F32)))))
            .ok_or_else(|| malformed("frag.load_attachment result type was not interned"))?;
        let result = format!("v{}", values.len());
        writeln!(text, "    %{result} = {opcode} {}", attachment.magnitude)
            .map_err(|issue| malformed(issue.to_string()))?;
        values.push((result, index as u64));
    } else if operation.opcode == 0x409 {
        if !operation.operands.is_empty() {
            return Err(malformed("frag.imageblock.addr requires no operands"));
        }
        let index = types
            .entries
            .iter()
            .position(|candidate| matches!(candidate, BinaryType::Pointer(8)))
            .ok_or_else(|| malformed("frag.imageblock.addr requires ptr<tile> in TYPES"))?;
        let result = format!("v{}", values.len());
        writeln!(text, "    %{result} = {opcode}").map_err(|issue| malformed(issue.to_string()))?;
        values.push((result, index as u64));
    } else {
        if !operation.operands.is_empty() {
            return Err(malformed(
                "zero-result fragment opcode must have no operands",
            ));
        }
        writeln!(text, "    {opcode}").map_err(|issue| malformed(issue.to_string()))?;
    }
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
