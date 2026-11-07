use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{AddressSpace, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

#[derive(Clone, Debug)]
pub(crate) struct Longjmp {
    pub(crate) buffer: String,
    pub(crate) value: String,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<Longjmp, TextParseError> {
    if instruction.opcode != "longjmp" || !instruction.results.is_empty() {
        return Err(error(
            item,
            instruction.span,
            "longjmp is a zero-result terminator",
        ));
    }
    let source = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid longjmp source span"))?
        .trim();
    let (buffer, value) = source
        .strip_prefix("longjmp ")
        .and_then(|arguments| arguments.split_once(", "))
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "longjmp requires buffer and value operands",
            )
        })?;
    let name = |text: &str| -> Result<String, TextParseError> {
        text.strip_prefix('%')
            .filter(|name| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
            })
            .map(ToOwned::to_owned)
            .ok_or_else(|| error(item, instruction.span, "longjmp operands must be SSA names"))
    };
    Ok(Longjmp {
        buffer: name(buffer)?,
        value: name(value)?,
    })
}

pub(crate) fn validate(
    operation: &Longjmp,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    if values.get(&operation.buffer) != Some(&Type::Pointer(AddressSpace::Generic))
        || values.get(&operation.value) != Some(&Type::Integer(32))
    {
        return Err(error(
            item,
            span,
            "longjmp expects a generic ptr buffer and i32 value",
        ));
    }
    Ok(())
}

pub(crate) fn encode(
    operation: &Longjmp,
    values: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let value_delta = |name: &str| {
        values
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| (values.len() - index) as u64)
            .ok_or_else(|| {
                error(
                    item,
                    span,
                    "longjmp operand has no preceding SSA definition",
                )
            })
    };
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(0x215)?;
    write(0)?;
    write(0)?;
    write(value_delta(&operation.buffer)?)?;
    write(value_delta(&operation.value)?)?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &[(String, u64)],
) -> Result<(), FormatError> {
    if instruction.opcode != 0x215
        || !instruction.terminator
        || instruction.modifiers != 0
        || instruction.result_count != 0
    {
        return Err(malformed(
            "invalid longjmp terminator modifiers or result count",
        ));
    }
    let [BinaryOperand::Values(arguments)] = instruction.operands.as_slice() else {
        return Err(malformed("longjmp must encode two SSA operands"));
    };
    if arguments.len() != 2 {
        return Err(malformed("longjmp expects two SSA operands"));
    }
    let input = |index: u64| {
        usize::try_from(index)
            .ok()
            .and_then(|index| values.get(index))
            .ok_or_else(|| malformed("longjmp references an undefined SSA operand"))
    };
    let (buffer, ptr_ty) = input(arguments[0])?;
    let (value, int_ty) = input(arguments[1])?;
    if !matches!(
        types.entries.get(*ptr_ty as usize),
        Some(BinaryType::Pointer(0))
    ) || !matches!(
        types.entries.get(*int_ty as usize),
        Some(BinaryType::Integer(32))
    ) {
        return Err(malformed(
            "longjmp requires a generic ptr buffer and i32 value",
        ));
    }
    writeln!(text, "longjmp %{buffer}, %{value}").map_err(|issue| malformed(issue.to_string()))
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
