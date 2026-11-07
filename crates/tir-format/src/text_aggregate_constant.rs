//! Trivial tuple/array constant literals stored in the CONSTANTS section.
use std::collections::HashMap;
use std::fmt::Write as _;
use tir_diag::Span;
use tir_types::{FloatType, Type};

use crate::types::{
    BinaryConstantTable, BinaryConstantValue, BinaryInstruction, BinaryOperand, BinaryType,
    BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct AggregateConstant {
    pub(crate) name: String,
    pub(crate) ty: Type,
    pub(crate) value: BinaryConstantValue,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    symbols: Option<&crate::text_direct_call::Callees>,
) -> Result<AggregateConstant, TextParseError> {
    if instruction.opcode != "const.aggregate" || instruction.results.len() != 1 {
        return Err(error(
            item,
            instruction.span,
            "const.aggregate requires one SSA result",
        ));
    }
    let name = &instruction.results[0];
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "invalid const.aggregate source span",
            )
        })?
        .trim();
    let prefix = format!("%{name} = const.aggregate ");
    let values = line.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "const.aggregate requires type(values)",
        )
    })?;
    let (ty, arguments) = crate::text_aggregate_constant_nested::split_typed_arguments(
        values,
        item,
        instruction.span,
    )?;
    let value = crate::text_aggregate_constant_nested::parse_aggregate(
        &ty,
        arguments,
        item,
        instruction.span,
        0,
        symbols,
    )?;
    Ok(AggregateConstant {
        name: name.clone(),
        ty,
        value,
    })
}

pub(super) fn scalar(
    ty: &Type,
    literal: &str,
    item: TextItem,
    span: Span,
) -> Result<BinaryConstantValue, TextParseError> {
    match ty {
        Type::Integer(bits @ 1..=128) => {
            let bits_value = if *bits == 1 && literal == "true" {
                Some(1)
            } else if *bits == 1 && literal == "false" {
                Some(0)
            } else {
                crate::numeric_literals::integer_literal_bits(literal, *bits)
            };
            let bits_value = bits_value
                .ok_or_else(|| error(item, span, "aggregate integer constant is out of range"))?;
            let bits_value = if *bits == 128 {
                bits_value
            } else {
                bits_value & ((1u128 << *bits) - 1)
            };
            Ok(BinaryConstantValue::Integer(
                bits_value.to_le_bytes()[..usize::from(bits.div_ceil(8))].to_vec(),
            ))
        }
        Type::Float(
            float @ (FloatType::F16
            | FloatType::Bf16
            | FloatType::F32
            | FloatType::F64
            | FloatType::F128),
        ) => {
            let bytes = crate::numeric_literals::parse_ieee_float_literal_bytes(literal, *float)
                .ok_or_else(|| error(item, span, "aggregate float is not exactly representable"))?;
            Ok(BinaryConstantValue::Float(bytes))
        }
        Type::Pointer(_) if literal == "null" => Ok(BinaryConstantValue::Null),
        _ => Err(error(
            item,
            span,
            "const.aggregate requires trivial scalar fields",
        )),
    }
}

pub(crate) fn validate(
    constant: &AggregateConstant,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&constant.name) {
        return Err(error(
            item,
            span,
            "const.aggregate result SSA name is already defined",
        ));
    }
    Ok((constant.name.clone(), constant.ty.clone()))
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    types: &BinaryTypeTable,
    indices: &HashMap<usize, (u64, u64)>,
    symbols: &crate::text_direct_call::Callees,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let constant = parse(instruction, source, item, Some(symbols))?;
    let (ty, index) = indices
        .get(&instruction.span.start)
        .copied()
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "const.aggregate is missing a CONSTANTS record",
            )
        })?;
    if crate::text_descriptor_reference::type_index(types, &constant.ty) != usize::try_from(ty).ok()
    {
        return Err(error(
            item,
            instruction.span,
            "const.aggregate type index disagrees with TYPES",
        ));
    }
    for word in [0x009, 0, 1, ty, index] {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x009 || instruction.result_count != 1 || instruction.modifiers != 0 {
        return Err(malformed(
            "const.aggregate requires one result and no modifiers",
        ));
    }
    let [BinaryOperand::Type(ty), BinaryOperand::Index(index)] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "const.aggregate requires a type and CONSTANTS index",
        ));
    };
    let constant = constants
        .records
        .get(*index as usize)
        .ok_or_else(|| malformed("const.aggregate CONSTANTS index is invalid"))?;
    if constant.ty != *ty {
        return Err(malformed(
            "const.aggregate CONSTANTS type does not match CODE",
        ));
    }
    let literal =
        crate::text_aggregate_constant_nested::format_aggregate(types, *ty, &constant.value, 0)?;
    let type_name = crate::text_type_disassembly::format_type_reference(
        types,
        *ty,
        &crate::types::BinarySignatureTable::default(),
    )?;
    let name = format!("v{}", values.len());
    writeln!(text, "    %{name} = const.aggregate {type_name}{literal}",)
        .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, *ty));
    Ok(())
}

pub(super) fn scalar_text(
    types: &BinaryTypeTable,
    index: u64,
    value: &BinaryConstantValue,
) -> Result<String, FormatError> {
    match (types.entries.get(index as usize), value) {
        (Some(BinaryType::Integer(bits @ 1..=128)), BinaryConstantValue::Integer(bytes))
            if Some(bytes.len()) == usize::try_from(bits.div_ceil(8)).ok() =>
        {
            let mut raw = [0u8; 16];
            raw[..bytes.len()].copy_from_slice(bytes);
            Ok(u128::from_le_bytes(raw).to_string())
        }
        (Some(BinaryType::Float(float)), BinaryConstantValue::Float(bytes)) => {
            crate::text_global_disassembly::format_ieee_float_literal(bytes, *float)
                .ok_or_else(|| malformed("const.aggregate floating field is not representable"))
        }
        (Some(BinaryType::Pointer(_)), BinaryConstantValue::Null) => Ok("null".to_string()),
        _ => Err(malformed(
            "const.aggregate field has unsupported type or value",
        )),
    }
}

pub(super) fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
pub(super) fn error(item: TextItem, span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span: Span::new(
            item.span.start.saturating_add(span.start),
            item.span.start.saturating_add(span.end),
        ),
        message: message.into(),
    }
}
