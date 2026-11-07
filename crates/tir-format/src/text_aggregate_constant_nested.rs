//! Recursive literal validation for typed tuple and array CONSTANTS values.
use tir_diag::Span;
use tir_types::Type;

use crate::text_aggregate_constant::{error, malformed, scalar, scalar_text};
use crate::text_direct_call::Callees;
use crate::types::{BinaryConstantValue, BinaryNominalKind, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextItem, TextParseError};

const MAX_LITERAL_NESTING: usize = 64;

/// Find the aggregate type before the outer argument list, even when that
/// type is itself a tuple or the value contains nested parentheses.
pub(crate) fn split_typed_arguments<'a>(
    source: &'a str,
    item: TextItem,
    span: Span,
) -> Result<(Type, &'a str), TextParseError> {
    if !source.ends_with(')') {
        return Err(error(
            item,
            span,
            "const.aggregate arguments are not closed",
        ));
    }
    for (index, ch) in source.char_indices() {
        if index > 0 && ch == '(' {
            if let Ok(ty) = crate::parse_type(source[..index].trim()) {
                return Ok((ty, &source[index + 1..source.len() - 1]));
            }
        }
    }
    Err(error(
        item,
        span,
        "const.aggregate requires a valid type followed by values",
    ))
}

pub(crate) fn parse_aggregate(
    ty: &Type,
    contents: &str,
    item: TextItem,
    span: Span,
    depth: usize,
    symbols: Option<&Callees>,
) -> Result<BinaryConstantValue, TextParseError> {
    if depth >= MAX_LITERAL_NESTING {
        return Err(error(
            item,
            span,
            "const.aggregate nesting exceeds 64 levels",
        ));
    }
    let arguments = split_values(contents, item, span)?;
    match ty {
        Type::Tuple(fields) if !fields.is_empty() && fields.len() == arguments.len() => {
            let values = fields
                .iter()
                .zip(arguments)
                .map(|(ty, text)| parse_field(ty, text, item, span, depth + 1, symbols))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(BinaryConstantValue::Tuple(values))
        }
        Type::Array { count, element }
            if *count > 0 && usize::try_from(*count).ok() == Some(arguments.len()) =>
        {
            let values = arguments
                .into_iter()
                .map(|text| parse_field(element, text, item, span, depth + 1, symbols))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(BinaryConstantValue::Array(values))
        }
        Type::Nominal {
            path,
            arguments: generic_arguments,
        } if generic_arguments.is_empty() => {
            if let Some(fields) = symbols.and_then(|symbols| symbols.union_fields.get(path)) {
                let [member, literal] = arguments.as_slice() else {
                    return Err(error(
                        item,
                        span,
                        "explicit union constant requires member index and value",
                    ));
                };
                let index = member
                    .parse::<usize>()
                    .ok()
                    .filter(|index| *index < fields.len())
                    .ok_or_else(|| error(item, span, "explicit union member index is invalid"))?;
                let value = parse_field(&fields[index], literal, item, span, depth + 1, symbols)?;
                return Ok(BinaryConstantValue::Union {
                    member: index as u64,
                    value: Box::new(value),
                });
            }
            let fields = symbols
                .and_then(|symbols| symbols.struct_fields.get(path))
                .ok_or_else(|| {
                    error(
                        item,
                        span,
                        "const.aggregate requires declared nongeneric trivial fields",
                    )
                })?;
            if fields.len() != arguments.len() {
                return Err(error(
                    item,
                    span,
                    "const.aggregate nominal struct field count is incorrect",
                ));
            }
            let values = fields
                .iter()
                .zip(arguments)
                .map(|(ty, text)| parse_field(ty, text, item, span, depth + 1, symbols))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(BinaryConstantValue::Struct(values))
        }
        _ => Err(error(
            item,
            span,
            "const.aggregate requires matching nonempty tuple/array fields",
        )),
    }
}

fn parse_field(
    ty: &Type,
    literal: &str,
    item: TextItem,
    span: Span,
    depth: usize,
    symbols: Option<&Callees>,
) -> Result<BinaryConstantValue, TextParseError> {
    if matches!(
        ty,
        Type::Tuple(_) | Type::Array { .. } | Type::Nominal { .. }
    ) {
        let contents = literal
            .trim()
            .strip_prefix('(')
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| error(item, span, "nested aggregate field must be parenthesized"))?;
        parse_aggregate(ty, contents, item, span, depth, symbols)
    } else {
        scalar(ty, literal.trim(), item, span)
    }
}

fn split_values<'a>(
    contents: &'a str,
    item: TextItem,
    span: Span,
) -> Result<Vec<&'a str>, TextParseError> {
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut nesting = Vec::new();
    let mut previous = 0;
    let mut elements = Vec::new();
    for (i, ch) in contents.char_indices() {
        match ch {
            '(' | '[' => {
                nesting.push(ch);
                if nesting.len() > MAX_LITERAL_NESTING {
                    return Err(error(
                        item,
                        span,
                        "const.aggregate literal nesting is too deep",
                    ));
                }
            }
            ')' | ']' => {
                let expected = if ch == ')' { '(' } else { '[' };
                if nesting.pop() != Some(expected) {
                    return Err(error(
                        item,
                        span,
                        "const.aggregate has mismatched literal delimiters",
                    ));
                }
            }
            ',' if nesting.is_empty() => {
                let literal = contents[previous..i].trim();
                if literal.is_empty() {
                    return Err(error(item, span, "const.aggregate has empty element"));
                }
                elements.push(literal);
                previous = i + 1;
            }
            _ => {}
        }
    }
    if !nesting.is_empty() {
        return Err(error(
            item,
            span,
            "const.aggregate has unclosed literal delimiters",
        ));
    }
    let last = contents[previous..].trim();
    if last.is_empty() {
        return Err(error(
            item,
            span,
            "const.aggregate has trailing empty element",
        ));
    }
    elements.push(last);
    Ok(elements)
}

pub(crate) fn format_aggregate(
    types: &BinaryTypeTable,
    ty: u64,
    value: &BinaryConstantValue,
    depth: usize,
) -> Result<String, FormatError> {
    if depth >= MAX_LITERAL_NESTING {
        return Err(malformed(
            "const.aggregate formatting nesting exceeds 64 levels",
        ));
    }
    if let (
        Some(BinaryType::Nominal {
            declaration,
            arguments,
        }),
        BinaryConstantValue::Union { member, value },
    ) = (types.entries.get(ty as usize), value)
    {
        if !arguments.is_empty()
            || types
                .declarations
                .get(*declaration as usize)
                .is_none_or(|declaration| declaration.kind != BinaryNominalKind::Explicit)
        {
            return Err(malformed(
                "const.aggregate union requires an explicit nongeneric type",
            ));
        }
        let member_type = types
            .definitions
            .iter()
            .find(|definition| definition.declaration == *declaration)
            .and_then(|definition| {
                usize::try_from(*member)
                    .ok()
                    .and_then(|index| definition.fields.get(index))
            })
            .map(|field| field.ty)
            .ok_or_else(|| malformed("const.aggregate explicit union member is invalid"))?;
        let rendered = if matches!(
            types.entries.get(member_type as usize),
            Some(BinaryType::Tuple(_) | BinaryType::Array { .. } | BinaryType::Nominal { .. })
        ) {
            format_aggregate(types, member_type, value, depth + 1)?
        } else {
            scalar_text(types, member_type, value)?
        };
        return Ok(format!("({member}, {rendered})"));
    }
    let pairs = match (types.entries.get(ty as usize), value) {
        (Some(BinaryType::Tuple(fields)), BinaryConstantValue::Tuple(values))
            if !fields.is_empty() && fields.len() == values.len() =>
        {
            fields.iter().copied().zip(values).collect::<Vec<_>>()
        }
        (Some(BinaryType::Array { count, element }), BinaryConstantValue::Array(values))
            if *count > 0 && usize::try_from(*count).ok() == Some(values.len()) =>
        {
            values
                .iter()
                .map(|value| (*element, value))
                .collect::<Vec<_>>()
        }
        (
            Some(BinaryType::Nominal {
                declaration,
                arguments,
            }),
            BinaryConstantValue::Struct(values),
        ) if arguments.is_empty()
            && types
                .declarations
                .get(*declaration as usize)
                .is_some_and(|decl| decl.kind == BinaryNominalKind::Struct) =>
        {
            let definition = types
                .definitions
                .iter()
                .find(|definition| definition.declaration == *declaration)
                .ok_or_else(|| malformed("const.aggregate struct has no TYPES definition"))?;
            if definition.fields.len() != values.len() {
                return Err(malformed(
                    "const.aggregate struct field count differs from TYPES",
                ));
            }
            definition
                .fields
                .iter()
                .map(|field| field.ty)
                .zip(values)
                .collect::<Vec<_>>()
        }
        _ => {
            return Err(malformed(
                "const.aggregate has incompatible tuple/array CONSTANTS value",
            ));
        }
    };
    let literals = pairs
        .into_iter()
        .map(|(ty, value)| {
            if matches!(
                types.entries.get(ty as usize),
                Some(BinaryType::Tuple(_) | BinaryType::Array { .. } | BinaryType::Nominal { .. })
            ) {
                format_aggregate(types, ty, value, depth + 1)
            } else {
                scalar_text(types, ty, value)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("({})", literals.join(", ")))
}
