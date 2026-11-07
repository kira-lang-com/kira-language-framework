//! `block.addr bbN`: the only supported producer of block-local indirect-branch targets.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

#[derive(Clone, Debug)]
pub(crate) struct BlockAddress {
    pub(crate) result: String,
    pub(crate) label: String,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<BlockAddress, TextParseError> {
    if instruction.opcode != "block.addr" {
        return Err(error(item, instruction.span, "expected block.addr"));
    }
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "block.addr defines one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "block.addr source span invalid"))?
        .trim();
    let prefix = format!("%{result} = block.addr ");
    let label = line
        .strip_prefix(&prefix)
        .filter(|label| {
            label.starts_with("bb")
                && label[2..].chars().all(|ch| ch.is_ascii_digit())
                && label.len() > 2
        })
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "block.addr requires canonical bbN label",
            )
        })?;
    Ok(BlockAddress {
        result: result.clone(),
        label: label.to_owned(),
    })
}

pub(crate) fn validate(
    address: &BlockAddress,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&address.result) {
        return Err(error(
            item,
            span,
            "block.addr result SSA value is duplicated",
        ));
    }
    Ok((address.result.clone(), Type::Pointer(AddressSpace::Generic)))
}

pub(crate) fn encode(
    address: &BlockAddress,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index = address.label[2..]
        .parse::<u64>()
        .map_err(|_| error(item, span, "block.addr index exceeds ULEB range"))?;
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(0x00f)?;
    write(0)?;
    write(1)?;
    write(index)?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x00f || instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed("invalid block.addr result or modifiers"));
    }
    let [BinaryOperand::Index(target)] = instruction.operands.as_slice() else {
        return Err(malformed("block.addr must encode one block index"));
    };
    let ty = types
        .entries
        .iter()
        .position(|entry| matches!(entry, BinaryType::Pointer(0)))
        .ok_or_else(|| malformed("block.addr generic pointer type is not interned"))?;
    let name = format!("v{}", values.len());
    writeln!(text, "    %{name} = block.addr bb{target}")
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, ty as u64));
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
