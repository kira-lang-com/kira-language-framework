//! Nongeneric enum.take/enum.project for trivial payload variants.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::text_direct_call::Callees;
use crate::types::{
    BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct EnumPayload {
    result: String,
    source: String,
    variant: String,
    project: bool,
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "enum.take" | "enum.project")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<EnumPayload, TextParseError> {
    if !handles(&instruction.opcode) {
        return Err(error(item, instruction.span, "expected enum.take/project"));
    }
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "enum.take/project must define one SSA result",
        ));
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "enum payload source span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{result} = {} %", instruction.opcode);
    let (source, variant) = text
        .strip_prefix(&prefix)
        .and_then(|value| value.split_once(", "))
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "enum payload requires %enum, Variant",
            )
        })?;
    if !valid_name(source) || !valid_name(variant) {
        return Err(error(
            item,
            instruction.span,
            "enum payload needs SSA source and variant identifier",
        ));
    }
    Ok(EnumPayload {
        result: result.clone(),
        source: source.to_owned(),
        variant: variant.to_owned(),
        project: instruction.opcode == "enum.project",
    })
}

pub(crate) fn validate(
    operation: &EnumPayload,
    values: &HashMap<String, Type>,
    symbols: Option<&Callees>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&operation.result) {
        return Err(error(item, span, "duplicate enum payload SSA result"));
    }
    let Type::Nominal { path, arguments } = values
        .get(&operation.source)
        .ok_or_else(|| error(item, span, "enum payload source is undefined"))?
    else {
        return Err(error(
            item,
            span,
            "enum payload requires a nongeneric enum source",
        ));
    };
    if !arguments.is_empty() {
        return Err(error(
            item,
            span,
            "enum payload requires a nongeneric enum type",
        ));
    }
    let expected = symbols
        .and_then(|symbols| symbols.enum_payloads.get(path))
        .and_then(|variants| variants.iter().find(|(name, _)| name == &operation.variant))
        .map(|(_, ty)| ty.clone())
        .ok_or_else(|| {
            error(
                item,
                span,
                "enum payload variant is missing or has nontrivial fields",
            )
        })?;
    Ok((operation.result.clone(), expected))
}

pub(crate) fn encode(
    operation: &EnumPayload,
    names: &[&str],
    inferred: &HashMap<String, Type>,
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let source = names
        .iter()
        .position(|name| *name == operation.source)
        .map(|position| (names.len() - position) as u64)
        .ok_or_else(|| error(item, span, "enum payload SSA operand is undefined"))?;
    let ty = inferred
        .get(&operation.source)
        .ok_or_else(|| error(item, span, "enum payload source has no inferred type"))?;
    let nominal_index = crate::text_descriptor_reference::type_index(types, ty)
        .ok_or_else(|| error(item, span, "enum payload nominal source lacks TYPES entry"))?;
    let Some(BinaryType::Nominal {
        declaration,
        arguments,
    }) = types.entries.get(nominal_index)
    else {
        return Err(error(
            item,
            span,
            "enum payload source is not a nominal enum",
        ));
    };
    if !arguments.is_empty()
        || types
            .declarations
            .get(*declaration as usize)
            .is_none_or(|entry| entry.kind != BinaryNominalKind::Enum)
    {
        return Err(error(item, span, "enum payload requires a nongeneric enum"));
    }
    let definition = types
        .definitions
        .iter()
        .find(|entry| entry.declaration == *declaration)
        .ok_or_else(|| error(item, span, "enum payload lacks a TYPES definition"))?;
    let variant = definition
        .variants
        .iter()
        .position(|entry| entry.name == operation.variant)
        .ok_or_else(|| error(item, span, "enum payload variant is unknown"))?;
    let payload = &definition.variants[variant].payload;
    if payload.iter().any(|ty| !trivial(types, *ty)) {
        return Err(error(
            item,
            span,
            "enum payload cannot extract nontrivial fields",
        ));
    }
    let expected = payload_type(types, payload)
        .ok_or_else(|| error(item, span, "enum payload result type was not interned"))?;
    let result = inferred.get(&operation.result).and_then(|ty| {
        crate::text_descriptor_reference::type_index(types, ty).map(|index| index as u64)
    });
    if result != Some(expected) {
        return Err(error(
            item,
            span,
            "enum payload inferred result disagrees with TYPES",
        ));
    }
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(if operation.project { 0x10d } else { 0x10c })?;
    write(0)?;
    write(1)?;
    write(source)?;
    write(variant as u64)?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let project = match instruction.opcode {
        0x10c => false,
        0x10d => true,
        _ => return Err(malformed("invalid enum payload CODE opcode")),
    };
    if instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "enum payload requires one result and zero modifiers",
        ));
    }
    let [BinaryOperand::Value(source), BinaryOperand::Index(variant)] =
        instruction.operands.as_slice()
    else {
        return Err(malformed(
            "enum payload requires SSA source and variant index",
        ));
    };
    let (name, ty) = usize::try_from(*source)
        .ok()
        .and_then(|index| values.get(index))
        .ok_or_else(|| malformed("enum payload SSA operand is out of range"))?;
    let Some(BinaryType::Nominal {
        declaration,
        arguments,
    }) = types.entries.get(*ty as usize)
    else {
        return Err(malformed("enum payload operand must have nominal type"));
    };
    if !arguments.is_empty()
        || types
            .declarations
            .get(*declaration as usize)
            .is_none_or(|entry| entry.kind != BinaryNominalKind::Enum)
    {
        return Err(malformed("enum payload requires nongeneric nominal enum"));
    }
    let definition = types
        .definitions
        .iter()
        .find(|definition| definition.declaration == *declaration)
        .ok_or_else(|| malformed("enum payload definition is missing"))?;
    let selected = definition
        .variants
        .get(*variant as usize)
        .ok_or_else(|| malformed("enum payload variant index is invalid"))?;
    if selected.payload.iter().any(|ty| !trivial(types, *ty)) {
        return Err(malformed("enum payload contains nontrivial fields"));
    }
    let result_ty = payload_type(types, &selected.payload)
        .ok_or_else(|| malformed("enum payload result type was not interned"))?;
    let opcode = if project { "enum.project" } else { "enum.take" };
    let result_name = format!("v{}", values.len());
    writeln!(
        text,
        "    %{result_name} = {opcode} %{name}, {}",
        selected.name
    )
    .map_err(|issue| malformed(issue.to_string()))?;
    values.push((result_name, result_ty));
    Ok(())
}

pub(crate) fn payload_type(types: &BinaryTypeTable, payload: &[u64]) -> Option<u64> {
    types
        .entries
        .iter()
        .position(|ty| match payload {
            [] => matches!(ty, BinaryType::Unit),
            [one] => types
                .entries
                .get(*one as usize)
                .is_some_and(|expected| expected == ty),
            many => matches!(ty, BinaryType::Tuple(fields) if fields.as_slice() == many),
        })
        .map(|index| index as u64)
}
fn trivial(types: &BinaryTypeTable, index: u64) -> bool {
    matches!(
        types.entries.get(index as usize),
        Some(BinaryType::Integer(_) | BinaryType::Float(_) | BinaryType::Pointer(_))
    )
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
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
