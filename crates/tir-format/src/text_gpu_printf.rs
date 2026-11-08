use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::text::{TokenKind, decode_kira_string_literal_contents};
use crate::types::{BinaryInstruction, BinaryOperand};
use crate::{FormatError, StringTable, TextInstruction, TextItem, TextParseError};

pub(crate) struct GpuPrintf {
    pub(crate) format: String,
    pub(crate) arguments: Vec<String>,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    item: TextItem,
) -> Result<GpuPrintf, TextParseError> {
    if instruction.opcode != "gpu.printf" || !instruction.results.is_empty() {
        return Err(error(
            item,
            instruction.span,
            "gpu.printf cannot define SSA results",
        ));
    }
    let tokens = &instruction.tokens;
    let [gpu, dot, printf, format, tail @ ..] = tokens.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "gpu.printf requires a quoted format string",
        ));
    };
    if !matches!(&gpu.kind, TokenKind::Atom(value) if value == "gpu")
        || !matches!(&dot.kind, TokenKind::Punctuation('.'))
        || !matches!(&printf.kind, TokenKind::Atom(value) if value == "printf")
    {
        return Err(error(
            item,
            instruction.span,
            "invalid gpu.printf opcode tokens",
        ));
    }
    let TokenKind::String(raw) = &format.kind else {
        return Err(error(
            item,
            instruction.span,
            "gpu.printf format must be a quoted string",
        ));
    };
    let format = decode_kira_string_literal_contents(raw).map_err(|issue| {
        error(
            item,
            instruction.span,
            format!("gpu.printf string: {issue}"),
        )
    })?;
    if tail.len() % 3 != 0 {
        return Err(error(
            item,
            instruction.span,
            "gpu.printf arguments must be written as `, %value`",
        ));
    }
    let mut arguments = Vec::with_capacity(tail.len() / 3);
    for chunk in tail.chunks_exact(3) {
        let [comma, percent, value] = chunk else {
            unreachable!()
        };
        if !matches!(comma.kind, TokenKind::Punctuation(','))
            || !matches!(percent.kind, TokenKind::Punctuation('%'))
        {
            return Err(error(
                item,
                instruction.span,
                "gpu.printf requires comma-separated SSA operands",
            ));
        }
        let TokenKind::Atom(name) = &value.kind else {
            return Err(error(
                item,
                instruction.span,
                "gpu.printf requires SSA names",
            ));
        };
        arguments.push(name.clone());
    }
    Ok(GpuPrintf { format, arguments })
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    item: TextItem,
    values: &HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let printf = parse(instruction, item)?;
    for name in &printf.arguments {
        if !values.contains_key(name) {
            return Err(error(
                item,
                instruction.span,
                "gpu.printf argument is undefined",
            ));
        }
    }
    Ok(())
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    item: TextItem,
    strings: &HashMap<usize, u64>,
    values: &[&str],
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let printf = parse(instruction, item)?;
    let format_index = strings
        .get(&instruction.span.start)
        .copied()
        .ok_or_else(|| error(item, instruction.span, "gpu.printf string was not interned"))?;
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))
    };
    for value in [0x40a, 0, 0, format_index, printf.arguments.len() as u64] {
        write(value)?;
    }
    for name in &printf.arguments {
        let position = values
            .iter()
            .position(|candidate| candidate == name)
            .ok_or_else(|| error(item, instruction.span, "gpu.printf argument is undefined"))?;
        write((values.len() - position) as u64)?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    strings: &StringTable,
    values: &[(String, u64)],
) -> Result<(), FormatError> {
    if instruction.opcode != 0x40a || instruction.modifiers != 0 || instruction.result_count != 0 {
        return Err(malformed("gpu.printf must have zero modifiers and results"));
    }
    let [
        BinaryOperand::Index(format_index),
        BinaryOperand::Values(arguments),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "gpu.printf requires a STRINGS index and SSA value list",
        ));
    };
    let format = usize::try_from(*format_index)
        .ok()
        .and_then(|index| strings.get(index))
        .ok_or_else(|| malformed("gpu.printf format STRINGS index is invalid"))?;
    let quoted = crate::text_global_disassembly::quote(format)?;
    write!(text, "    gpu.printf {quoted}").map_err(|issue| malformed(issue.to_string()))?;
    for argument in arguments {
        let name = usize::try_from(*argument)
            .ok()
            .and_then(|index| values.get(index))
            .map(|(name, _)| name)
            .ok_or_else(|| malformed("gpu.printf refers to an undefined SSA value"))?;
        write!(text, ", %{name}").map_err(|issue| malformed(issue.to_string()))?;
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
