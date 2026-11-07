//! Trivial tuple/struct destructuring into consecutive SSA results.
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use tir_diag::Span;
use tir_types::Type;

use crate::text_direct_call::Callees;
use crate::types::{
    BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct Destructure {
    source: String,
    results: Vec<String>,
    struct_value: bool,
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "tuple.destructure" | "struct.destructure")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<Destructure, TextParseError> {
    if !handles(&instruction.opcode) {
        return Err(error(
            item,
            instruction.span,
            "expected tuple/struct.destructure",
        ));
    }
    if instruction.results.is_empty() {
        return Err(error(
            item,
            instruction.span,
            "destructure requires one result per field",
        ));
    }
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid destructure source span"))?
        .trim();
    let prefix = format!(
        "{} = {} %",
        instruction
            .results
            .iter()
            .map(|name| format!("%{name}"))
            .collect::<Vec<_>>()
            .join(", "),
        instruction.opcode,
    );
    let source = text
        .strip_prefix(&prefix)
        .filter(|source| valid_name(source))
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "destructure requires exactly one SSA aggregate",
            )
        })?;
    if instruction.results.iter().any(|name| !valid_name(name)) {
        return Err(error(
            item,
            instruction.span,
            "invalid destructure result SSA name",
        ));
    }
    Ok(Destructure {
        source: source.to_owned(),
        results: instruction.results.clone(),
        struct_value: instruction.opcode == "struct.destructure",
    })
}

pub(crate) fn validate(
    operation: &Destructure,
    values: &HashMap<String, Type>,
    symbols: Option<&Callees>,
    item: TextItem,
    span: Span,
) -> Result<Vec<(String, Type)>, TextParseError> {
    let source = values
        .get(&operation.source)
        .ok_or_else(|| error(item, span, "destructure source is undefined"))?;
    let fields = if operation.struct_value {
        let Type::Nominal { path, arguments } = source else {
            return Err(error(
                item,
                span,
                "struct.destructure requires a nominal struct",
            ));
        };
        if !arguments.is_empty() {
            return Err(error(
                item,
                span,
                "struct.destructure currently requires nongeneric struct",
            ));
        }
        symbols
            .and_then(|symbols| symbols.struct_fields.get(path))
            .ok_or_else(|| {
                error(
                    item,
                    span,
                    "struct.destructure requires all fields to be trivial",
                )
            })?
    } else {
        let Type::Tuple(fields) = source else {
            return Err(error(
                item,
                span,
                "tuple.destructure requires a tuple value",
            ));
        };
        fields
    };
    if fields.is_empty()
        || fields.len() != operation.results.len()
        || fields.iter().any(|ty| !trivial(ty))
    {
        return Err(error(
            item,
            span,
            "destructure result count/types must match nonempty trivial fields",
        ));
    }
    let mut seen = HashSet::new();
    if operation
        .results
        .iter()
        .any(|name| !seen.insert(name) || values.contains_key(name) || name == &operation.source)
    {
        return Err(error(
            item,
            span,
            "destructure result SSA names must be fresh and distinct",
        ));
    }
    Ok(operation
        .results
        .iter()
        .cloned()
        .zip(fields.iter().cloned())
        .collect())
}

pub(crate) fn encode(
    operation: &Destructure,
    names: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let source = names
        .iter()
        .position(|value| *value == operation.source)
        .map(|index| (names.len() - index) as u64)
        .ok_or_else(|| error(item, span, "destructure source SSA value is undefined"))?;
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(if operation.struct_value { 0x103 } else { 0x106 })?;
    write(0)?;
    write(operation.results.len() as u64)?;
    write(source)?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let struct_value = match instruction.opcode {
        0x103 => true,
        0x106 => false,
        _ => return Err(malformed("invalid aggregate destructure opcode")),
    };
    if instruction.modifiers != 0 || instruction.result_count == 0 {
        return Err(malformed(
            "destructure requires at least one SSA result and zero modifiers",
        ));
    }
    let [BinaryOperand::Value(source)] = instruction.operands.as_slice() else {
        return Err(malformed(
            "destructure requires exactly one SSA value operand",
        ));
    };
    let (name, source_ty) = usize::try_from(*source)
        .ok()
        .and_then(|index| values.get(index))
        .ok_or_else(|| malformed("destructure SSA source is out of range"))?;
    let fields = match (struct_value, types.entries.get(*source_ty as usize)) {
        (false, Some(BinaryType::Tuple(fields))) => fields.clone(),
        (
            true,
            Some(BinaryType::Nominal {
                declaration,
                arguments,
            }),
        ) if arguments.is_empty() => {
            if types
                .declarations
                .get(*declaration as usize)
                .is_none_or(|entry| entry.kind != BinaryNominalKind::Struct)
            {
                return Err(malformed("struct.destructure requires a nominal struct"));
            }
            types
                .definitions
                .iter()
                .find(|entry| entry.declaration == *declaration)
                .ok_or_else(|| malformed("struct.destructure lacks a TYPES definition"))?
                .fields
                .iter()
                .map(|field| field.ty)
                .collect()
        }
        _ => return Err(malformed("destructure source aggregate type mismatch")),
    };
    if fields.is_empty()
        || instruction.result_count != fields.len() as u64
        || fields.iter().any(|field| !trivial_binary(types, *field))
    {
        return Err(malformed(
            "destructure result count or field types are not supported",
        ));
    }
    let results = (0..fields.len())
        .map(|index| format!("%v{}", values.len() + index))
        .collect::<Vec<_>>();
    let opcode = if struct_value {
        "struct.destructure"
    } else {
        "tuple.destructure"
    };
    writeln!(text, "    {} = {opcode} %{name}", results.join(", "))
        .map_err(|issue| malformed(issue.to_string()))?;
    let offset = values.len();
    for (index, field) in fields.into_iter().enumerate() {
        values.push((format!("v{}", offset + index), field));
    }
    Ok(())
}

fn trivial(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_) | Type::Float(_) | Type::Pointer(_))
}
fn trivial_binary(types: &BinaryTypeTable, index: u64) -> bool {
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
