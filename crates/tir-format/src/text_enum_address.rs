use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::Type;

use crate::text_direct_call::Callees;
use crate::types::{
    BinaryInstruction, BinaryNominalKind, BinaryOperand, BinaryType, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) enum EnumAddress {
    Payload {
        result: String,
        base: String,
        variant: String,
    },
    Inject {
        base: String,
        variant: String,
    },
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "enum.payload.addr" | "enum.inject")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<EnumAddress, TextParseError> {
    if !handles(&instruction.opcode) {
        return Err(error(
            item,
            instruction.span,
            "unexpected enum address opcode",
        ));
    }
    let text = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid enum address source span"))?
        .trim();
    let statement = if instruction.opcode == "enum.payload.addr" {
        let [result] = instruction.results.as_slice() else {
            return Err(error(
                item,
                instruction.span,
                "enum.payload.addr requires one SSA result",
            ));
        };
        text.strip_prefix(&format!("%{result} = enum.payload.addr "))
            .ok_or_else(|| error(item, instruction.span, "invalid enum.payload.addr syntax"))?
    } else {
        if !instruction.results.is_empty() {
            return Err(error(
                item,
                instruction.span,
                "enum.inject cannot define an SSA result",
            ));
        }
        text.strip_prefix("enum.inject ")
            .ok_or_else(|| error(item, instruction.span, "invalid enum.inject syntax"))?
    };
    let (base, variant) = statement.split_once(", ").ok_or_else(|| {
        error(
            item,
            instruction.span,
            "enum address requires base and variant",
        )
    })?;
    let base = ssa(base, item, instruction.span)?;
    if !valid_name(variant) {
        return Err(error(
            item,
            instruction.span,
            "enum address variant name/index is invalid",
        ));
    }
    if instruction.opcode == "enum.payload.addr" {
        Ok(EnumAddress::Payload {
            result: instruction.results[0].clone(),
            base,
            variant: variant.to_owned(),
        })
    } else {
        Ok(EnumAddress::Inject {
            base,
            variant: variant.to_owned(),
        })
    }
}

pub(crate) fn validate(
    operation: &EnumAddress,
    values: &HashMap<String, Type>,
    symbols: Option<&Callees>,
    item: TextItem,
    span: Span,
) -> Result<Option<(String, Type)>, TextParseError> {
    let (base, variant) = match operation {
        EnumAddress::Payload { base, variant, .. } | EnumAddress::Inject { base, variant } => {
            (base, variant)
        }
    };
    let Some(Type::Address { pointee, space }) = values.get(base) else {
        return Err(error(
            item,
            span,
            "enum address requires a typed address base",
        ));
    };
    let Type::Nominal { path, arguments } = pointee.as_ref() else {
        return Err(error(
            item,
            span,
            "enum address base must point to a nominal enum",
        ));
    };
    if !arguments.is_empty() {
        return Err(error(item, span, "enum address requires a nongeneric enum"));
    }
    let variants = symbols
        .and_then(|symbols| symbols.enum_payloads.get(path))
        .ok_or_else(|| {
            error(
                item,
                span,
                "enum address requires a declared enum with trivial payloads",
            )
        })?;
    let (_, payload) = variant_index(variants, variant)
        .ok_or_else(|| error(item, span, "enum address variant is unknown"))?;
    match operation {
        EnumAddress::Payload { result, .. } => {
            if values.contains_key(result) {
                return Err(error(
                    item,
                    span,
                    "enum payload address result is already defined",
                ));
            }
            Ok(Some((
                result.clone(),
                Type::Address {
                    pointee: Box::new(payload.clone()),
                    space: *space,
                },
            )))
        }
        EnumAddress::Inject { .. } => {
            if *payload != Type::Unit {
                return Err(error(
                    item,
                    span,
                    "enum.inject currently supports payloadless variants only",
                ));
            }
            Ok(None)
        }
    }
}

pub(crate) fn encode(
    operation: &EnumAddress,
    names: &[&str],
    inferred: &HashMap<String, Type>,
    symbols: &Callees,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let (base, variant, payload) = match operation {
        EnumAddress::Payload { base, variant, .. } => (base, variant, true),
        EnumAddress::Inject { base, variant } => (base, variant, false),
    };
    let delta = names
        .iter()
        .position(|name| *name == base)
        .map(|index| (names.len() - index) as u64)
        .ok_or_else(|| error(item, span, "enum address base is undefined"))?;
    let Type::Address { pointee, .. } = inferred
        .get(base)
        .ok_or_else(|| error(item, span, "enum address base type is unavailable"))?
    else {
        return Err(error(item, span, "enum address requires an address"));
    };
    let Type::Nominal { path, arguments } = pointee.as_ref() else {
        return Err(error(
            item,
            span,
            "enum address must address a nominal enum",
        ));
    };
    if !arguments.is_empty() {
        return Err(error(item, span, "generic enum addressing is unsupported"));
    }
    let variants = symbols.enum_payloads.get(path).ok_or_else(|| {
        error(
            item,
            span,
            "enum address has no complete trivial variant table",
        )
    })?;
    let (index, selected) = variant_index(variants, variant)
        .ok_or_else(|| error(item, span, "enum address variant is unknown"))?;
    if !payload && *selected != Type::Unit {
        return Err(error(
            item,
            span,
            "cannot inject a variant with unproven payload initialization",
        ));
    }
    let mut write = |word| {
        crate::leb::write_uleb(word, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(if payload { 0x124 } else { 0x125 })?;
    write(0)?;
    write(u64::from(payload))?;
    write(delta)?;
    write(index as u64)?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let payload = match instruction.opcode {
        0x124 => true,
        0x125 => false,
        _ => return Err(malformed("unexpected enum address opcode")),
    };
    if instruction.modifiers != 0 || instruction.result_count != u64::from(payload) {
        return Err(malformed(
            "enum address result count or modifiers are invalid",
        ));
    }
    let [BinaryOperand::Value(base), BinaryOperand::Index(variant)] =
        instruction.operands.as_slice()
    else {
        return Err(malformed(
            "enum address needs base SSA value and variant index",
        ));
    };
    let (base_name, base_type) = usize::try_from(*base)
        .ok()
        .and_then(|index| values.get(index))
        .map(|(name, ty)| (name.as_str(), *ty))
        .ok_or_else(|| malformed("enum address base SSA value is invalid"))?;
    let Some(BinaryType::Address { pointee, space }) = types.entries.get(base_type as usize) else {
        return Err(malformed("enum address base must be typed address"));
    };
    let Some(BinaryType::Nominal {
        declaration,
        arguments,
    }) = types.entries.get(*pointee as usize)
    else {
        return Err(malformed("enum address base pointee must be nominal"));
    };
    if !arguments.is_empty()
        || types
            .declarations
            .get(*declaration as usize)
            .is_none_or(|decl| decl.kind != BinaryNominalKind::Enum)
    {
        return Err(malformed(
            "enum address requires a nongeneric declared enum",
        ));
    }
    let definition = types
        .definitions
        .iter()
        .find(|def| def.declaration == *declaration)
        .ok_or_else(|| malformed("enum address type definition is missing"))?;
    let selected = usize::try_from(*variant)
        .ok()
        .and_then(|i| definition.variants.get(i))
        .ok_or_else(|| malformed("enum address variant index is invalid"))?;
    if selected.payload.iter().any(|index| !trivial(types, *index)) {
        return Err(malformed("enum address payload is nontrivial"));
    }
    if payload {
        let payload_type = crate::text_enum_payload::payload_type(types, &selected.payload)
            .ok_or_else(|| malformed("enum address payload type is not interned"))?;
        let ty = types
            .entries
            .iter()
            .position(|ty| {
                matches!(ty,
            BinaryType::Address { pointee: field, space: actual }
                if *field == payload_type && *actual == *space)
            })
            .ok_or_else(|| malformed("enum address projected type is not interned"))?;
        writeln!(
            text,
            "    %v{} = enum.payload.addr %{base_name}, {}",
            values.len(),
            selected.name
        )
        .map_err(|issue| malformed(issue.to_string()))?;
        values.push((format!("v{}", values.len()), ty as u64));
    } else {
        if !selected.payload.is_empty() {
            return Err(malformed(
                "enum.inject has a payload whose initialization is unproven",
            ));
        }
        writeln!(text, "    enum.inject %{base_name}, {}", selected.name)
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    Ok(())
}

fn variant_index<'a>(variants: &'a [(String, Type)], name: &str) -> Option<(usize, &'a Type)> {
    if let Ok(index) = name.parse::<usize>() {
        variants.get(index).map(|(_, ty)| (index, ty))
    } else {
        variants
            .iter()
            .enumerate()
            .find(|(_, (variant, _))| variant == name)
            .map(|(index, (_, ty))| (index, ty))
    }
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
fn ssa(value: &str, item: TextItem, span: Span) -> Result<String, TextParseError> {
    value
        .strip_prefix('%')
        .filter(|name| valid_name(name))
        .map(str::to_owned)
        .ok_or_else(|| error(item, span, "enum address requires SSA base"))
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
