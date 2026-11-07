//! Typed field and fixed-array element addresses (CODE 0x122 / 0x123).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::text_direct_call::Callees;
use crate::types::{
    BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const UNCHECKED: u64 = 1 << 33;

pub(crate) enum Projection {
    Field {
        result: String,
        base: String,
        field: u64,
    },
    Element {
        result: String,
        base: String,
        index: String,
        unchecked: bool,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "field.addr" | "elem.addr")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<Projection, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "address projection requires one SSA result",
        ));
    };
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "address projection span is invalid"))?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let operands = text
        .strip_prefix(&prefix)
        .ok_or_else(|| error(item, instruction.span, "invalid address projection syntax"))?;
    match instruction.opcode.as_str() {
        "field.addr" => {
            let (base, field) = operands.split_once(", ").ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "field.addr requires base and field index",
                )
            })?;
            let field = field
                .parse::<u64>()
                .map_err(|_| error(item, instruction.span, "field.addr index must be unsigned"))?;
            Ok(Projection::Field {
                result: result.clone(),
                base: ssa(base, item, instruction.span)?,
                field,
            })
        }
        "elem.addr" => {
            let (operands, unchecked) =
                if let Some(operands) = operands.strip_suffix(" [unchecked]") {
                    (operands, true)
                } else {
                    (operands, false)
                };
            let (base, index) = operands.split_once(", ").ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "elem.addr requires base and SSA index",
                )
            })?;
            Ok(Projection::Element {
                result: result.clone(),
                base: ssa(base, item, instruction.span)?,
                index: ssa(index, item, instruction.span)?,
                unchecked,
            })
        }
        _ => Err(error(
            item,
            instruction.span,
            "unexpected address projection opcode",
        )),
    }
}

pub(crate) fn validate(
    operation: &Projection,
    values: &HashMap<String, Type>,
    symbols: Option<&Callees>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    let (result, base, field, index) = match operation {
        Projection::Field {
            result,
            base,
            field,
        } => (result, base, Some(*field), None),
        Projection::Element {
            result,
            base,
            index,
            ..
        } => (result, base, None, Some(index)),
    };
    if values.contains_key(result) {
        return Err(error(
            item,
            span,
            "address projection SSA result was already defined",
        ));
    }
    let Some(Type::Address { pointee, space }) = values.get(base) else {
        return Err(error(
            item,
            span,
            "address projection base must be a typed address",
        ));
    };
    let projected = if let Some(field) = field {
        let fields = match pointee.as_ref() {
            Type::Tuple(fields) => fields,
            Type::Nominal { path, arguments } if arguments.is_empty() => symbols
                .and_then(|symbols| symbols.struct_fields.get(path))
                .ok_or_else(|| {
                    error(
                        item,
                        span,
                        "field.addr requires a declared nongeneric trivial struct",
                    )
                })?,
            _ => {
                return Err(error(
                    item,
                    span,
                    "field.addr base must address a tuple or struct",
                ));
            }
        };
        let ty = usize::try_from(field)
            .ok()
            .and_then(|index| fields.get(index))
            .ok_or_else(|| error(item, span, "field.addr index is outside aggregate fields"))?;
        if !trivial(ty) {
            return Err(error(
                item,
                span,
                "field.addr text CODE requires a trivial field type",
            ));
        }
        ty.clone()
    } else {
        let Type::Array { element, .. } = pointee.as_ref() else {
            return Err(error(
                item,
                span,
                "elem.addr base must address a fixed array",
            ));
        };
        let Some(index_ty) = index.and_then(|name| values.get(name)) else {
            return Err(error(item, span, "elem.addr index SSA value is undefined"));
        };
        if !matches!(index_ty, Type::Integer(_)) || !trivial(element) {
            return Err(error(
                item,
                span,
                "elem.addr requires integer index and trivial array element",
            ));
        }
        element.as_ref().clone()
    };
    Ok((
        result.clone(),
        Type::Address {
            pointee: Box::new(projected),
            space: *space,
        },
    ))
}

pub(crate) fn encode(
    operation: &Projection,
    names: &[&str],
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let delta = |name: &str| {
        names
            .iter()
            .position(|value| *value == name)
            .map(|index| (names.len() - index) as u64)
            .ok_or_else(|| error(item, span, "address projection SSA operand is undefined"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        Projection::Field { base, field, .. } => {
            write(0x122)?;
            write(0)?;
            write(1)?;
            write(delta(base)?)?;
            write(*field)?;
        }
        Projection::Element {
            base,
            index,
            unchecked,
            ..
        } => {
            write(0x123)?;
            write(if *unchecked { UNCHECKED } else { 0 })?;
            write(1)?;
            write(delta(base)?)?;
            write(delta(index)?)?;
        }
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let get = |id: u64| {
        usize::try_from(id)
            .ok()
            .and_then(|id| values.get(id))
            .map(|(name, ty)| (name.as_str(), *ty))
            .ok_or_else(|| malformed("address projection refers to invalid SSA operand"))
    };
    if instruction.result_count != 1 {
        return Err(malformed("address projection requires one SSA result"));
    }
    let (line, expected) = match instruction.opcode {
        0x122 => {
            if instruction.modifiers != 0 {
                return Err(malformed("field.addr forbids modifiers"));
            }
            let [BinaryOperand::Value(base), BinaryOperand::Index(field)] =
                instruction.operands.as_slice()
            else {
                return Err(malformed("field.addr requires value and field index"));
            };
            let (name, ty) = get(*base)?;
            let (pointee, space) = address(types, ty)?;
            let fields = match types.entries.get(pointee as usize) {
                Some(BinaryType::Tuple(fields)) => fields.clone(),
                Some(BinaryType::Nominal {
                    declaration,
                    arguments,
                }) if arguments.is_empty() => {
                    if types
                        .declarations
                        .get(*declaration as usize)
                        .is_none_or(|decl| decl.kind != BinaryNominalKind::Struct)
                    {
                        return Err(malformed("field.addr requires struct or tuple address"));
                    }
                    types
                        .definitions
                        .iter()
                        .find(|definition| definition.declaration == *declaration)
                        .ok_or_else(|| malformed("struct has no definition"))?
                        .fields
                        .iter()
                        .map(|field| field.ty)
                        .collect()
                }
                _ => return Err(malformed("field.addr base does not address aggregate")),
            };
            let field_type = usize::try_from(*field)
                .ok()
                .and_then(|i| fields.get(i))
                .ok_or_else(|| malformed("field.addr index is out of bounds"))?;
            if !trivial_binary(types, *field_type) {
                return Err(malformed("field.addr currently needs trivial field type"));
            }
            (
                format!("field.addr %{name}, {field}"),
                result_address(types, *field_type, space)?,
            )
        }
        0x123 => {
            if instruction.modifiers & !UNCHECKED != 0 {
                return Err(malformed("elem.addr has unsupported modifiers"));
            }
            let [BinaryOperand::Values(operands)] = instruction.operands.as_slice() else {
                return Err(malformed("elem.addr requires two SSA operands"));
            };
            let [base, index] = operands.as_slice() else {
                return Err(malformed("elem.addr must have base and index"));
            };
            let (base_name, base_ty) = get(*base)?;
            let (index_name, index_ty) = get(*index)?;
            if !matches!(
                types.entries.get(index_ty as usize),
                Some(BinaryType::Integer(_))
            ) {
                return Err(malformed("elem.addr index must be integer"));
            }
            let (pointee, space) = address(types, base_ty)?;
            let Some(BinaryType::Array { element, .. }) = types.entries.get(pointee as usize)
            else {
                return Err(malformed("elem.addr base must address array"));
            };
            if !trivial_binary(types, *element) {
                return Err(malformed("elem.addr currently needs trivial element"));
            }
            let suffix = if instruction.modifiers & UNCHECKED != 0 {
                " [unchecked]"
            } else {
                ""
            };
            (
                format!("elem.addr %{base_name}, %{index_name}{suffix}"),
                result_address(types, *element, space)?,
            )
        }
        _ => return Err(malformed("unsupported address projection opcode")),
    };
    writeln!(text, "    %v{} = {line}", values.len())
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((format!("v{}", values.len()), expected));
    Ok(())
}

fn address(types: &BinaryTypeTable, ty: u64) -> Result<(u64, u8), FormatError> {
    match types.entries.get(ty as usize) {
        Some(BinaryType::Address { pointee, space }) => Ok((*pointee, *space)),
        _ => Err(malformed("address projection input must be typed address")),
    }
}
fn result_address(types: &BinaryTypeTable, pointee: u64, space: u8) -> Result<u64, FormatError> {
    types
        .entries
        .iter()
        .position(|ty| {
            matches!(ty, BinaryType::Address { pointee: p, space: s }
        if *p == pointee && *s == space)
        })
        .map(|index| index as u64)
        .ok_or_else(|| malformed("projected address result type is not interned"))
}
fn trivial(ty: &Type) -> bool {
    matches!(ty, Type::Integer(_) | Type::Float(_) | Type::Pointer(_))
}
fn trivial_binary(types: &BinaryTypeTable, ty: u64) -> bool {
    matches!(
        types.entries.get(ty as usize),
        Some(BinaryType::Integer(_) | BinaryType::Float(_) | BinaryType::Pointer(_))
    )
}
fn ssa(text: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    text.strip_prefix('%')
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
        })
        .map(str::to_owned)
        .ok_or_else(|| error(item, span, "address projection requires SSA name"))
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
