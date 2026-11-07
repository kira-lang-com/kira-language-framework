//! Non-generic trivial nominal struct/enum creation and struct insertion.
use std::collections::HashMap;
use tir_diag::Span;
use tir_types::Type;

use crate::types::{BinaryNominalKind, BinaryType, BinaryTypeTable};
use crate::{FormatError, FunctionBody, TextInstruction, TextItem, TextParseError};

pub(crate) enum NominalOp {
    StructNew {
        result: String,
        ty: Type,
        args: Vec<String>,
    },
    StructInsert {
        result: String,
        source: String,
        field: u64,
        value: String,
    },
    StructExtract {
        result: String,
        source: String,
        field: u64,
    },
    EnumNew {
        result: String,
        ty: Type,
        variant: String,
        args: Vec<String>,
    },
    EnumTag {
        result: String,
        source: String,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(
        opcode,
        "struct.new" | "struct.extract" | "struct.insert" | "enum.new" | "enum.tag"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<NominalOp, TextParseError> {
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "nominal creation/insertion requires one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "nominal operation source span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{result} = {} ", instruction.opcode);
    let rest = line
        .strip_prefix(&prefix)
        .ok_or_else(|| error(item, instruction.span, "invalid nominal operation syntax"))?;
    match instruction.opcode.as_str() {
        "struct.new" => {
            let (ty, args) = rest
                .split_once('(')
                .ok_or_else(|| error(item, instruction.span, "struct.new requires %T(fields)"))?;
            let ty = parse_nominal(ty.trim(), item, instruction.span)?;
            let args = parse_args(args, item, instruction.span)?;
            Ok(NominalOp::StructNew {
                result: result.clone(),
                ty,
                args,
            })
        }
        "struct.insert" => {
            let args = rest.split(", ").collect::<Vec<_>>();
            let [source, field, value] = args.as_slice() else {
                return Err(error(
                    item,
                    instruction.span,
                    "struct.insert requires object, field, value",
                ));
            };
            let field = field.parse::<u64>().map_err(|_| {
                error(
                    item,
                    instruction.span,
                    "struct.insert field index must be unsigned",
                )
            })?;
            Ok(NominalOp::StructInsert {
                result: result.clone(),
                source: name(source, item, instruction.span)?,
                field,
                value: name(value, item, instruction.span)?,
            })
        }
        "struct.extract" => {
            let (source, field) = rest.split_once(", ").ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "struct.extract requires %value, field-index",
                )
            })?;
            let field = field.parse::<u64>().map_err(|_| {
                error(
                    item,
                    instruction.span,
                    "struct.extract index must be unsigned",
                )
            })?;
            Ok(NominalOp::StructExtract {
                result: result.clone(),
                source: name(source, item, instruction.span)?,
                field,
            })
        }
        "enum.new" => {
            let (ty, variant_args) = rest.split_once(", ").ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "enum.new requires %Enum, Variant(payload)",
                )
            })?;
            let ty = parse_nominal(ty, item, instruction.span)?;
            let (variant, args) = variant_args.split_once('(').ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "enum.new requires variant payload parentheses",
                )
            })?;
            if !valid_name(variant) {
                return Err(error(
                    item,
                    instruction.span,
                    "enum.new variant name is invalid",
                ));
            }
            Ok(NominalOp::EnumNew {
                result: result.clone(),
                ty,
                variant: variant.to_owned(),
                args: parse_args(args, item, instruction.span)?,
            })
        }
        "enum.tag" => Ok(NominalOp::EnumTag {
            result: result.clone(),
            source: name(rest, item, instruction.span)?,
        }),
        _ => Err(error(
            item,
            instruction.span,
            "unsupported nominal operation",
        )),
    }
}

fn parse_nominal(source: &str, item: TextItem, span: Span) -> Result<Type, TextParseError> {
    let ty = crate::parse_type(source).map_err(|issue| error(item, span, issue.to_string()))?;
    if !matches!(ty, Type::Nominal { ref arguments, .. } if arguments.is_empty()) {
        return Err(error(
            item,
            span,
            "nominal constructor requires a nongeneric %T",
        ));
    }
    Ok(ty)
}

fn parse_args(text: &str, item: TextItem, span: Span) -> Result<Vec<String>, TextParseError> {
    let args = text
        .strip_suffix(')')
        .ok_or_else(|| error(item, span, "nominal constructor payload is not closed"))?;
    if args.is_empty() {
        Ok(Vec::new())
    } else {
        args.split(',')
            .map(|arg| name(arg.trim(), item, span))
            .collect()
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
}
fn name(text: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    text.strip_prefix('%')
        .filter(|name| valid_name(name))
        .map(str::to_owned)
        .ok_or_else(|| error(item, span, "nominal operation arguments must be SSA values"))
}

pub(crate) fn nominal_type(operation: &NominalOp) -> Option<&Type> {
    match operation {
        NominalOp::StructNew { ty, .. } | NominalOp::EnumNew { ty, .. } => Some(ty),
        _ => None,
    }
}

pub(crate) fn validate(
    operation: &NominalOp,
    values: &HashMap<String, Type>,
    symbols: Option<&crate::text_direct_call::Callees>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    let source = |name: &str| {
        values
            .get(name)
            .ok_or_else(|| error(item, span, "nominal operation has undefined SSA operand"))
    };
    let (name, result) = match operation {
        NominalOp::StructNew { result, ty, args }
        | NominalOp::EnumNew {
            result, ty, args, ..
        } => {
            for arg in args {
                source(arg)?;
            }
            (result.clone(), ty.clone())
        }
        NominalOp::StructInsert {
            result,
            source: original,
            value,
            ..
        } => {
            let ty = source(original)?;
            if !matches!(ty, Type::Nominal { arguments, .. } if arguments.is_empty()) {
                return Err(error(
                    item,
                    span,
                    "struct.insert source must be nongeneric struct",
                ));
            }
            source(value)?;
            (result.clone(), ty.clone())
        }
        NominalOp::StructExtract {
            result,
            source: value,
            field,
        } => {
            let Type::Nominal { path, arguments } = source(value)? else {
                return Err(error(
                    item,
                    span,
                    "struct.extract requires a nominal struct",
                ));
            };
            if !arguments.is_empty() {
                return Err(error(
                    item,
                    span,
                    "struct.extract requires a nongeneric struct",
                ));
            }
            let ty = symbols
                .and_then(|symbols| symbols.struct_fields.get(path))
                .and_then(|fields| {
                    usize::try_from(*field)
                        .ok()
                        .and_then(|index| fields.get(index))
                })
                .ok_or_else(|| {
                    error(
                        item,
                        span,
                        "struct.extract requires a declared trivial field",
                    )
                })?;
            (result.clone(), ty.clone())
        }
        NominalOp::EnumTag {
            result,
            source: value,
        } => {
            let Type::Nominal { path, arguments } = source(value)? else {
                return Err(error(item, span, "enum.tag requires a nominal enum"));
            };
            if !arguments.is_empty() {
                return Err(error(item, span, "enum.tag requires a nongeneric enum"));
            }
            let width = symbols
                .and_then(|symbols| symbols.enum_tag_widths.get(path))
                .ok_or_else(|| error(item, span, "enum.tag requires a declared enum"))?;
            (result.clone(), Type::Integer(*width))
        }
    };
    if values.contains_key(&name) {
        return Err(error(
            item,
            span,
            "nominal operation result is already defined",
        ));
    }
    Ok((name, result))
}

pub(super) fn definition<'a>(
    types: &'a BinaryTypeTable,
    ty: u64,
    kind: BinaryNominalKind,
) -> Result<&'a crate::types::BinaryNominalDefinition, FormatError> {
    let Some(BinaryType::Nominal {
        declaration,
        arguments,
    }) = types.entries.get(ty as usize)
    else {
        return Err(malformed(
            "nominal result type is not a nominal TYPES entry",
        ));
    };
    if !arguments.is_empty()
        || types
            .declarations
            .get(*declaration as usize)
            .is_none_or(|declaration| declaration.kind != kind)
    {
        return Err(malformed(
            "nominal constructor requires declared nongeneric struct/enum",
        ));
    }
    types
        .definitions
        .iter()
        .find(|definition| definition.declaration == *declaration)
        .ok_or_else(|| malformed("nominal constructor's type lacks a definition"))
}

pub(super) fn trivial(types: &BinaryTypeTable, ty: u64) -> bool {
    matches!(
        types.entries.get(ty as usize),
        Some(BinaryType::Integer(_) | BinaryType::Float(_) | BinaryType::Pointer(_))
    )
}

fn type_index(types: &BinaryTypeTable, ty: &Type) -> Result<u64, FormatError> {
    crate::text_descriptor_reference::type_index(types, ty)
        .and_then(|index| u64::try_from(index).ok())
        .ok_or_else(|| malformed("nominal type has no TYPES index"))
}

/// Check nominal field/payload types after the complete TYPES table has been
/// interned. The earlier text pass can infer an SSA result from `%Nominal`, but
/// cannot resolve its field declarations until TYPES exists.
pub(crate) fn validate_typed(
    body: &FunctionBody,
    source: &str,
    item: TextItem,
    types: &BinaryTypeTable,
    callees: &crate::text_direct_call::Callees,
) -> Result<(), TextParseError> {
    let mut values = body
        .blocks
        .iter()
        .flat_map(|block| &block.parameters)
        .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
        .collect::<HashMap<_, _>>();
    for block in &body.blocks {
        let Some((_, instructions)) = block.instructions.split_last() else {
            continue;
        };
        for instruction in instructions {
            if handles(&instruction.opcode) {
                let operation = parse(instruction, source, item)?;
                check_types(&operation, &values, types)
                    .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
            }
            crate::text_straight_line_validation::validate_operations(
                std::slice::from_ref(instruction),
                &mut values,
                source,
                item,
                Some(callees),
            )?;
        }
    }
    Ok(())
}

fn check_types(
    operation: &NominalOp,
    values: &HashMap<String, Type>,
    types: &BinaryTypeTable,
) -> Result<(), FormatError> {
    let type_of = |name: &str| {
        values
            .get(name)
            .ok_or_else(|| malformed("nominal constructor SSA argument is undefined"))
            .and_then(|ty| type_index(types, ty))
    };
    match operation {
        NominalOp::StructNew { ty, args, .. } => {
            let definition = definition(types, type_index(types, ty)?, BinaryNominalKind::Struct)?;
            if args.len() != definition.fields.len()
                || !definition
                    .fields
                    .iter()
                    .all(|field| trivial(types, field.ty))
            {
                return Err(malformed("struct.new requires the declared trivial fields"));
            }
            for (name, field) in args.iter().zip(&definition.fields) {
                if type_of(name)? != field.ty {
                    return Err(malformed(
                        "struct.new argument type differs from the field type",
                    ));
                }
            }
        }
        NominalOp::EnumNew {
            ty, variant, args, ..
        } => {
            let definition = definition(types, type_index(types, ty)?, BinaryNominalKind::Enum)?;
            let payload = &definition
                .variants
                .iter()
                .find(|entry| entry.name == *variant)
                .ok_or_else(|| malformed("enum.new variant does not exist"))?
                .payload;
            if args.len() != payload.len() || !payload.iter().all(|index| trivial(types, *index)) {
                return Err(malformed("enum.new payload arity/type policy is invalid"));
            }
            for (name, expected) in args.iter().zip(payload) {
                if type_of(name)? != *expected {
                    return Err(malformed("enum.new payload argument type is incorrect"));
                }
            }
        }
        NominalOp::StructInsert {
            source,
            field,
            value,
            ..
        } => {
            let definition = definition(types, type_of(source)?, BinaryNominalKind::Struct)?;
            if !definition
                .fields
                .iter()
                .all(|field| trivial(types, field.ty))
            {
                return Err(malformed("struct.insert requires trivial fields"));
            }
            let expected = definition
                .fields
                .get(*field as usize)
                .ok_or_else(|| malformed("struct.insert field index is outside the struct"))?
                .ty;
            if type_of(value)? != expected {
                return Err(malformed(
                    "struct.insert replacement has the wrong field type",
                ));
            }
        }
        NominalOp::StructExtract { source, field, .. } => {
            let definition = definition(types, type_of(source)?, BinaryNominalKind::Struct)?;
            let ty = definition
                .fields
                .get(*field as usize)
                .ok_or_else(|| malformed("struct.extract field index exceeds struct fields"))?
                .ty;
            if !trivial(types, ty) {
                return Err(malformed("struct.extract requires a trivial field"));
            }
        }
        NominalOp::EnumTag { source, .. } => {
            let definition = definition(types, type_of(source)?, BinaryNominalKind::Enum)?;
            enum_tag_width(definition)?;
        }
    }
    Ok(())
}

pub(crate) fn encode(
    operation: &NominalOp,
    names: &[&str],
    types: &BinaryTypeTable,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index =
        |ty: &Type| type_index(types, ty).map_err(|issue| error(item, span, issue.to_string()));
    let delta = |name: &str| {
        names
            .iter()
            .position(|value| *value == name)
            .map(|position| (names.len() - position) as u64)
            .ok_or_else(|| error(item, span, "nominal SSA operand is undefined"))
    };
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    match operation {
        NominalOp::StructNew { ty, args, .. } => {
            let ty = index(ty)?;
            let definition = definition(types, ty, BinaryNominalKind::Struct)
                .map_err(|issue| error(item, span, issue.to_string()))?;
            if args.len() != definition.fields.len()
                || definition
                    .fields
                    .iter()
                    .any(|field| !trivial(types, field.ty))
            {
                return Err(error(
                    item,
                    span,
                    "struct.new requires matching trivial fields",
                ));
            }
            write(0x100)?;
            write(0)?;
            write(1)?;
            write(ty)?;
            write(args.len() as u64)?;
            for arg in args {
                write(delta(arg)?)?;
            }
        }
        NominalOp::StructInsert {
            source,
            field,
            value,
            ..
        } => {
            write(0x102)?;
            write(0)?;
            write(1)?;
            write(delta(source)?)?;
            write(*field)?;
            write(delta(value)?)?;
        }
        NominalOp::StructExtract { source, field, .. } => {
            write(0x101)?;
            write(0)?;
            write(1)?;
            write(delta(source)?)?;
            write(*field)?;
        }
        NominalOp::EnumNew {
            ty, variant, args, ..
        } => {
            let ty = index(ty)?;
            let definition = definition(types, ty, BinaryNominalKind::Enum)
                .map_err(|issue| error(item, span, issue.to_string()))?;
            let variant = definition
                .variants
                .iter()
                .position(|entry| entry.name == *variant)
                .ok_or_else(|| error(item, span, "enum.new variant does not exist"))?;
            let payload = &definition.variants[variant].payload;
            if args.len() != payload.len() || payload.iter().any(|ty| !trivial(types, *ty)) {
                return Err(error(
                    item,
                    span,
                    "enum.new requires trivial payload of declared arity",
                ));
            }
            write(0x10a)?;
            write(0)?;
            write(1)?;
            write(ty)?;
            write(variant as u64)?;
            write(args.len() as u64)?;
            for arg in args {
                write(delta(arg)?)?;
            }
        }
        NominalOp::EnumTag { source, .. } => {
            write(0x10b)?;
            write(0)?;
            write(1)?;
            write(delta(source)?)?;
        }
    }
    Ok(())
}

pub(super) fn enum_tag_width(
    definition: &crate::types::BinaryNominalDefinition,
) -> Result<u64, FormatError> {
    use crate::types::BinaryLayout;
    match definition.layout {
        BinaryLayout::C => Ok(32),
        BinaryLayout::Ridl | BinaryLayout::Packed(_) => match definition.variants.len() {
            0..=256 => Ok(8),
            257..=65_536 => Ok(16),
            _ => Ok(32),
        },
        BinaryLayout::Explicit { .. } => Err(malformed("enum.tag cannot use explicit layout")),
    }
}

pub(super) fn malformed(message: impl Into<String>) -> FormatError {
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
