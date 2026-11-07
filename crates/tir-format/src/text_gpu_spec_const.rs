//! GPU specialization defaults (0x401): typed, CONSTANTS-backed scalar/vector literals.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{FloatType, Type};

use crate::types::{
    BinaryConstantTable, BinaryConstantValue, BinaryInstruction, BinaryOperand, BinaryTypeTable,
};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct GpuSpecialization {
    pub(crate) result: String,
    pub(crate) id: u32,
    pub(crate) ty: Type,
    pub(crate) default: BinaryConstantValue,
}

pub(crate) fn scalar_supported(ty: &Type) -> bool {
    matches!(ty, Type::Integer(1..=64))
        || matches!(
            ty,
            Type::Float(FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64)
        )
}

pub(crate) fn supported(ty: &Type) -> bool {
    scalar_supported(ty)
        || matches!(ty, Type::FixedVector { lanes: 2..=4, element }
        if scalar_supported(element))
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<GpuSpecialization, TextParseError> {
    if instruction.opcode != "gpu.spec_const" {
        return Err(error(item, instruction.span, "expected gpu.spec_const"));
    }
    let [result] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "gpu.spec_const requires one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .and_then(|line| {
            line.trim()
                .strip_prefix(&format!("%{result} = gpu.spec_const "))
        })
        .ok_or_else(|| error(item, instruction.span, "invalid gpu.spec_const syntax"))?;
    let mut parts = line.splitn(3, ',');
    let id = parts
        .next()
        .and_then(|id| crate::numeric_literals::parse_unsigned_integer(id.trim()))
        .and_then(|id| u32::try_from(id).ok())
        .ok_or_else(|| error(item, instruction.span, "gpu.spec_const id must fit u32"))?;
    let ty = parts
        .next()
        .and_then(|text| crate::parse_type(text.trim()).ok())
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "gpu.spec_const requires a valid type",
            )
        })?;
    if !supported(&ty) {
        return Err(error(
            item,
            instruction.span,
            "gpu.spec_const supports i1..i64, f16/bf16/f32/f64, and 2-4 lane vectors of those types",
        ));
    }
    let default = parts
        .next()
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "gpu.spec_const requires a default literal",
            )
        })?
        .trim();
    let default = match &ty {
        Type::FixedVector { lanes, element } => {
            let text = default
                .strip_prefix('[')
                .and_then(|text| text.strip_suffix(']'))
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "gpu.spec_const vector default must be bracketed",
                    )
                })?;
            let elements = text.split(',').map(str::trim).collect::<Vec<_>>();
            if elements.len() != *lanes as usize {
                return Err(error(
                    item,
                    instruction.span,
                    "gpu.spec_const vector lane count is incorrect",
                ));
            }
            BinaryConstantValue::Vector(
                elements
                    .into_iter()
                    .map(|text| scalar_default(element, text, item, instruction.span))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        scalar => scalar_default(scalar, default, item, instruction.span)?,
    };
    Ok(GpuSpecialization {
        result: result.clone(),
        id,
        ty,
        default,
    })
}

fn scalar_default(
    ty: &Type,
    literal: &str,
    item: TextItem,
    span: Span,
) -> Result<BinaryConstantValue, TextParseError> {
    match ty {
        Type::Integer(1) => match literal {
            "true" => Ok(BinaryConstantValue::Integer(vec![1])),
            "false" => Ok(BinaryConstantValue::Integer(vec![0])),
            _ => Err(error(
                item,
                span,
                "gpu.spec_const i1 default requires true or false",
            )),
        },
        Type::Integer(bits @ 2..=64) => {
            let bits_value = crate::numeric_literals::integer_literal_bits(literal, *bits)
                .ok_or_else(|| {
                    error(item, span, "gpu.spec_const integer default is out of range")
                })?;
            let bits_value = bits_value & ((1u128 << *bits) - 1);
            Ok(BinaryConstantValue::Integer(
                bits_value.to_le_bytes()[..usize::from(bits.div_ceil(8))].to_vec(),
            ))
        }
        Type::Float(
            float @ (FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64),
        ) => {
            let bytes = crate::numeric_literals::parse_ieee_float_literal_bytes(literal, *float)
                .ok_or_else(|| {
                    error(item, span, "gpu.spec_const floating default must be exact")
                })?;
            Ok(BinaryConstantValue::Float(bytes))
        }
        _ => Err(error(item, span, "unsupported gpu.spec_const scalar type")),
    }
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    if values.contains_key(&op.result) {
        return Err(error(
            item,
            instruction.span,
            "gpu.spec_const SSA result already defined",
        ));
    }
    values.insert(op.result, op.ty);
    Ok(())
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    types: &BinaryTypeTable,
    indices: &HashMap<usize, (u64, u64)>,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    let expected_ty =
        crate::text_descriptor_reference::type_index(types, &op.ty).ok_or_else(|| {
            error(
                item,
                instruction.span,
                "gpu.spec_const result type was not interned",
            )
        })? as u64;
    let (ty, index) = indices
        .get(&instruction.span.start)
        .copied()
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "gpu.spec_const default was not interned in CONSTANTS",
            )
        })?;
    if expected_ty != ty {
        return Err(error(
            item,
            instruction.span,
            "gpu.spec_const type differs from CONSTANTS type",
        ));
    }
    for word in [0x401, 0, 1, u64::from(op.id), ty, index] {
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
    if instruction.opcode != 0x401 || instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "gpu.spec_const requires one result, zero modifiers",
        ));
    }
    let [
        BinaryOperand::Index(id),
        BinaryOperand::Type(ty),
        BinaryOperand::Index(index),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "gpu.spec_const requires id, type and CONSTANTS index",
        ));
    };
    if u32::try_from(*id).is_err() {
        return Err(malformed("gpu.spec_const id exceeds u32"));
    }
    let record = constants
        .records
        .get(*index as usize)
        .ok_or_else(|| malformed("gpu.spec_const CONSTANTS index is invalid"))?;
    if record.ty != *ty {
        return Err(malformed(
            "gpu.spec_const result type disagrees with CONSTANTS",
        ));
    }
    let type_name = crate::text_type_disassembly::format_type_reference(
        types,
        *ty,
        &crate::types::BinarySignatureTable::default(),
    )?;
    let parsed = crate::parse_type(&type_name)
        .map_err(|_| malformed("gpu.spec_const declared type cannot be formatted"))?;
    if !supported(&parsed) {
        return Err(malformed(
            "gpu.spec_const type is outside the supported scalar/vector subset",
        ));
    }
    let literal = match &parsed {
        Type::FixedVector { element, lanes } => {
            let BinaryConstantValue::Vector(items) = &record.value else {
                return Err(malformed("gpu.spec_const vector default kind is invalid"));
            };
            if items.len() != *lanes as usize {
                return Err(malformed("gpu.spec_const vector lane count is invalid"));
            }
            let formatted = items
                .iter()
                .map(|value| format_scalar(element, value))
                .collect::<Result<Vec<_>, _>>()?;
            format!("[{}]", formatted.join(", "))
        }
        ty => format_scalar(ty, &record.value)?,
    };
    let name = format!("v{}", values.len());
    writeln!(
        text,
        "    %{name} = gpu.spec_const {id}, {type_name}, {literal}"
    )
    .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, *ty));
    Ok(())
}

fn format_scalar(ty: &Type, value: &BinaryConstantValue) -> Result<String, FormatError> {
    match (ty, value) {
        (Type::Integer(bits @ 1..=64), BinaryConstantValue::Integer(bytes))
            if bytes.len() == usize::from(bits.div_ceil(8)) =>
        {
            let mut raw = [0u8; 8];
            raw[..bytes.len()].copy_from_slice(bytes);
            let integer = u64::from_le_bytes(raw);
            let max = if *bits == 64 {
                u64::MAX
            } else {
                (1u64 << *bits) - 1
            };
            if integer & !max != 0 {
                return Err(malformed(
                    "gpu.spec_const integer default has high padding bits set",
                ));
            }
            if *bits == 1 {
                Ok(if integer == 0 { "false" } else { "true" }.to_string())
            } else {
                Ok(integer.to_string())
            }
        }
        (Type::Float(float), BinaryConstantValue::Float(bytes)) => {
            crate::text_global_disassembly::format_ieee_float_literal(bytes, *float).ok_or_else(
                || malformed("gpu.spec_const float default cannot be formatted exactly"),
            )
        }
        _ => Err(malformed(
            "gpu.spec_const scalar default value does not match its type",
        )),
    }
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
