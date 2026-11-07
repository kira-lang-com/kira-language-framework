use crate::TextInstruction;
use crate::text::TokenKind;
use crate::{TextItem, TextParseError};
use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryType, BinaryTypeTable};

pub(super) fn trivial_constant_type_index(types: &BinaryTypeTable, ty: &Type) -> Option<usize> {
    crate::text_memory_assembly::supported_type_index(types, ty).or_else(|| match ty {
        Type::Pointer(space) => types.entries.iter().position(
            |entry| matches!(entry, BinaryType::Pointer(encoded) if *encoded == *space as u8),
        ),
        _ => None,
    })
}

pub(super) fn write(
    value: u64,
    output: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    crate::leb::write_uleb(value, output)
        .map_err(|err| error(item, span, format!("CODE encoding failed: {err}")))
}

pub(crate) fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}

pub(super) fn unsupported(item: TextItem, span: Span) -> TextParseError {
    error(item, span, "unsupported straight-line instruction")
}

pub(super) fn encode_select(
    instruction: &TextInstruction,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
) -> Result<(), TextParseError> {
    let parsed = crate::text_select_assembly::parse_select(instruction, item)?;
    write(0x03d, bytes, item, instruction.span)?;
    write(0, bytes, item, instruction.span)?;
    write(1, bytes, item, instruction.span)?;
    for operand in [&parsed.condition, &parsed.when_true, &parsed.when_false] {
        let position = values
            .iter()
            .position(|name| name == operand)
            .ok_or_else(|| error(item, instruction.span, "undefined SSA operand"))?;
        write(
            (values.len() - position) as u64,
            bytes,
            item,
            instruction.span,
        )?;
    }
    Ok(())
}

pub(super) fn encode_ownership(
    instruction: &TextInstruction,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
) -> Result<(), TextParseError> {
    let parsed = crate::text_ownership_assembly::parse(instruction, item)?;
    let position = values
        .iter()
        .position(|name| name == &parsed.operand)
        .ok_or_else(|| error(item, instruction.span, "ownership operand is undefined"))?;
    write(parsed.opcode, bytes, item, instruction.span)?;
    write(0, bytes, item, instruction.span)?;
    write(
        u64::from(parsed.result.is_some()),
        bytes,
        item,
        instruction.span,
    )?;
    write(
        (values.len() - position) as u64,
        bytes,
        item,
        instruction.span,
    )?;
    Ok(())
}

pub(super) fn encode_access(
    instruction: &TextInstruction,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
) -> Result<(), TextParseError> {
    let parsed = crate::text_access_assembly::parse(instruction, item)?;
    let position = values
        .iter()
        .position(|name| name == &parsed.operand)
        .ok_or_else(|| error(item, instruction.span, "undefined access SSA operand"))?;
    write(parsed.opcode, bytes, item, instruction.span)?;
    write(parsed.modifiers, bytes, item, instruction.span)?;
    write(
        u64::from(parsed.result.is_some()),
        bytes,
        item,
        instruction.span,
    )?;
    write(
        (values.len() - position) as u64,
        bytes,
        item,
        instruction.span,
    )?;
    Ok(())
}

pub(super) fn encode_return(
    terminator: &TextInstruction,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
) -> Result<(), TextParseError> {
    write(0x1e5, bytes, item, terminator.span)?;
    write(0, bytes, item, terminator.span)?;
    write(0, bytes, item, terminator.span)?;
    if let [_, _, name] = terminator.tokens.as_slice() {
        let TokenKind::Atom(name) = &name.kind else {
            return Err(error(item, terminator.span, "invalid return operand"));
        };
        let position = values
            .iter()
            .position(|candidate| candidate == name)
            .ok_or_else(|| error(item, terminator.span, "undefined return value"))?;
        write(
            (values.len() - position) as u64,
            bytes,
            item,
            terminator.span,
        )?;
    }
    Ok(())
}
