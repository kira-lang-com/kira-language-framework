//! Non-generic function reference (0x00a) using SIGNATURES identity.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::text_direct_call::Callees;
use crate::types::{
    BinaryInstruction, BinaryOperand, BinarySignatureTable, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct FunctionReference {
    result: String,
    callee: String,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<FunctionReference, TextParseError> {
    if instruction.opcode != "func.ref" {
        return Err(error(item, instruction.span, "expected func.ref"));
    }
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "func.ref must define exactly one SSA value",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid func.ref text span"))?
        .trim();
    let prefix = format!("%{result} = func.ref @");
    let callee = line
        .strip_prefix(&prefix)
        .filter(|value| {
            !value.is_empty()
                && !value.starts_with('.')
                && !value.ends_with('.')
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "func.ref must name a declared @function",
            )
        })?;
    Ok(FunctionReference {
        result: result.clone(),
        callee: callee.to_owned(),
    })
}

pub(crate) fn validate(
    reference: &FunctionReference,
    values: &HashMap<String, Type>,
    callees: &Callees,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&reference.result) {
        return Err(error(item, span, "duplicate func.ref SSA definition"));
    }
    let callee = callees
        .get(&reference.callee)
        .ok_or_else(|| error(item, span, "func.ref callee is not declared in SIGNATURES"))?;
    if callee.generic_parameters != 0 {
        return Err(error(
            item,
            span,
            "generic func.ref requires explicit type arguments",
        ));
    }
    Ok((
        reference.result.clone(),
        Type::Function(callee.signature.clone()),
    ))
}

pub(crate) fn encode(
    reference: &FunctionReference,
    callees: &Callees,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let callee = callees
        .get(&reference.callee)
        .ok_or_else(|| error(item, span, "func.ref callee is undefined"))?;
    let symbol = callee.index.checked_mul(4).ok_or_else(|| {
        error(
            item,
            span,
            "func.ref SIGNATURES index exceeds symbol format",
        )
    })?;
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(0x00a)?;
    write(0)?;
    write(1)?;
    write(symbol)?;
    write(0)?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x00a || instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed("func.ref requires zero modifiers and one result"));
    }
    let [
        BinaryOperand::Symbol { kind: 0, index },
        BinaryOperand::Types(arguments),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "func.ref binary operands must be a function symbol and type arguments",
        ));
    };
    let signature = usize::try_from(*index)
        .ok()
        .and_then(|index| signatures.records.get(index))
        .ok_or_else(|| malformed("func.ref references an unknown function"))?;
    if !arguments.is_empty() || !signature.generic_parameters.is_empty() {
        return Err(malformed(
            "generic func.ref cannot be represented by this text CODE subset",
        ));
    }
    if !matches!(
        types.entries.get(signature.function_type as usize),
        Some(BinaryType::Function(_))
    ) {
        return Err(malformed(
            "func.ref SIGNATURES record does not have a function type",
        ));
    }
    let result = format!("v{}", values.len());
    writeln!(text, "    %{result} = func.ref @{}", signature.name)
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((result, signature.function_type));
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
