use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{FloatType, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct VectorConstant {
    pub(crate) name: String,
    pub(crate) ty: Type,
    bytes: Vec<u8>,
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<VectorConstant, TextParseError> {
    if instruction.opcode != "const.vec" || instruction.results.len() != 1 {
        return Err(error(
            item,
            instruction.span,
            "const.vec must define one SSA value",
        ));
    }
    let name = &instruction.results[0];
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid const.vec text span"))?
        .trim();
    let prefix = format!("%{name} = const.vec ");
    let (type_text, elements) = line
        .strip_prefix(&prefix)
        .and_then(|raw| raw.split_once(", ["))
        .ok_or_else(|| error(item, instruction.span, "const.vec requires T, [elements]"))?;
    let ty = crate::parse_type(type_text)
        .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    let Type::FixedVector { lanes, element } = &ty else {
        return Err(error(
            item,
            instruction.span,
            "const.vec requires a fixed-vector type",
        ));
    };
    let bits = element_width(element).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "const.vec requires i8/i16/i32/i64 or IEEE float elements",
        )
    })?;
    let _layout = tir_layout::LayoutEngine::new().layout(&ty).map_err(|_| {
        error(
            item,
            instruction.span,
            "const.vec type has no static layout",
        )
    })?;
    // Storage over 16 bytes is represented by a CONSTANTS index in CODE.
    let elements = elements
        .strip_suffix(']')
        .ok_or_else(|| error(item, instruction.span, "const.vec list is not closed"))?;
    let literals = if elements.is_empty() {
        Vec::new()
    } else {
        elements.split(',').map(str::trim).collect::<Vec<_>>()
    };
    if literals.len() != *lanes as usize {
        return Err(error(
            item,
            instruction.span,
            "const.vec lane count differs from its vector type",
        ));
    }
    let mut bytes =
        Vec::with_capacity(literals.len().saturating_mul(usize::from(bits.div_ceil(8))));
    for (lane, literal) in literals.into_iter().enumerate() {
        if let Type::Float(float) = element.as_ref() {
            let lane = crate::numeric_literals::parse_ieee_float_literal_bytes(literal, *float)
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "const.vec floating lane is not exactly representable",
                    )
                })?;
            bytes.extend_from_slice(&lane);
            continue;
        }
        let unsigned = if bits == 1 && literal == "true" {
            Some(1)
        } else if bits == 1 && literal == "false" {
            Some(0)
        } else {
            crate::numeric_literals::integer_literal_bits(literal, bits)
        }
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "const.vec integer lane is out of range",
            )
        })?;
        if bits == 1 {
            if lane % 8 == 0 {
                bytes.push(0);
            }
            if unsigned == 1 {
                *bytes.last_mut().expect("allocated bitmask byte") |= 1 << (lane % 8);
            }
        } else {
            bytes.extend_from_slice(&unsigned.to_le_bytes()[..usize::from(bits / 8)]);
        }
    }
    Ok(VectorConstant {
        name: name.clone(),
        ty,
        bytes,
    })
}

pub(crate) fn validate(
    constant: &VectorConstant,
    values: &HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&constant.name) {
        return Err(error(
            item,
            span,
            "const.vec result name is already defined",
        ));
    }
    Ok((constant.name.clone(), constant.ty.clone()))
}

pub(crate) fn encode(
    constant: &VectorConstant,
    types: &BinaryTypeTable,
    constant_index: Option<(u64, u64)>,
    bytes: &mut Vec<u8>,
    item: TextItem,
    span: Span,
) -> Result<(), TextParseError> {
    let index = crate::text_descriptor_reference::type_index(types, &constant.ty)
        .ok_or_else(|| error(item, span, "const.vec vector type is not interned in TYPES"))?;
    let mut write = |value| {
        crate::leb::write_uleb(value, bytes).map_err(|issue| error(item, span, issue.to_string()))
    };
    write(0x008)?;
    write(0)?;
    write(1)?;
    write(index as u64)?;
    if constant.is_inline() {
        bytes.extend_from_slice(&constant.bytes);
    } else {
        let (constant_type, constant_index) = constant_index
            .ok_or_else(|| error(item, span, "large const.vec was not interned in CONSTANTS"))?;
        if constant_type != index as u64 {
            return Err(error(
                item,
                span,
                "const.vec CONSTANTS type disagrees with TYPES",
            ));
        }
        write(constant_index)?;
    }
    Ok(())
}

impl VectorConstant {
    pub(crate) fn is_inline(&self) -> bool {
        tir_layout::LayoutEngine::new()
            .layout(&self.ty)
            .is_ok_and(|layout| layout.size <= 16)
    }

    pub(crate) fn constant_value(&self) -> crate::types::BinaryConstantValue {
        use crate::types::BinaryConstantValue;
        let Type::FixedVector { element, .. } = &self.ty else {
            unreachable!()
        };
        let bits = element_width(element).expect("validated const.vec element width");
        if bits == 1 {
            let Type::FixedVector { lanes, .. } = &self.ty else {
                unreachable!()
            };
            return BinaryConstantValue::Vector(
                (0..*lanes)
                    .map(|lane| {
                        let bit = (self.bytes[(lane / 8) as usize] >> (lane % 8)) & 1;
                        BinaryConstantValue::Integer(vec![bit])
                    })
                    .collect(),
            );
        }
        BinaryConstantValue::Vector(
            self.bytes
                .chunks_exact(usize::from(bits / 8))
                .map(|lane| match element.as_ref() {
                    Type::Integer(_) => BinaryConstantValue::Integer(lane.to_vec()),
                    Type::Float(_) => BinaryConstantValue::Float(lane.to_vec()),
                    _ => unreachable!(),
                })
                .collect(),
        )
    }
}

pub(crate) fn encode_instruction(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    types: &BinaryTypeTable,
    constant_indices: &HashMap<usize, (u64, u64)>,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let vector = parse(instruction, source, item)?;
    encode(
        &vector,
        types,
        constant_indices.get(&instruction.span.start).copied(),
        bytes,
        item,
        instruction.span,
    )
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    constants: &crate::types::BinaryConstantTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x008 || instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed("const.vec requires one result and no modifiers"));
    }
    let [BinaryOperand::Type(ty), operand] = instruction.operands.as_slice() else {
        return Err(malformed("const.vec requires a type and payload/index"));
    };
    let Some(BinaryType::FixedVector { lanes, element }) = types.entries.get(*ty as usize) else {
        return Err(malformed("const.vec must declare a fixed-vector type"));
    };
    let element_ty = types
        .entries
        .get(*element as usize)
        .ok_or_else(|| malformed("const.vec element type index is invalid"))?;
    let width = match element_ty {
        BinaryType::Integer(1) => Some(0),
        BinaryType::Integer(bits @ (8 | 16 | 32 | 64)) => usize::try_from(*bits / 8).ok(),
        BinaryType::Float(float) => ieee_float_width(*float).map(|bits| usize::from(bits / 8)),
        _ => None,
    }
    .ok_or_else(|| malformed("const.vec requires supported scalar numeric lanes"))?;
    let bytes = match operand {
        BinaryOperand::Bytes(bytes) if bytes.len() <= 16 => bytes.clone(),
        BinaryOperand::Index(index) => {
            let constant = constants
                .records
                .get(*index as usize)
                .ok_or_else(|| malformed("const.vec CONSTANTS index is invalid"))?;
            if constant.ty != *ty {
                return Err(malformed("const.vec CONSTANTS type disagrees with TYPES"));
            }
            let crate::types::BinaryConstantValue::Vector(lanes) = &constant.value else {
                return Err(malformed("const.vec CONSTANTS value is not a vector"));
            };
            let mut bytes = Vec::new();
            for (index, lane) in lanes.iter().enumerate() {
                let lane = match (element_ty, lane) {
                    (BinaryType::Integer(_), crate::types::BinaryConstantValue::Integer(lane))
                    | (BinaryType::Float(_), crate::types::BinaryConstantValue::Float(lane)) => {
                        lane
                    }
                    _ => {
                        return Err(malformed(
                            "const.vec CONSTANTS lane kind is inconsistent with type",
                        ));
                    }
                };
                if lane.len() != width.max(1) {
                    return Err(malformed("const.vec CONSTANTS lane byte width is invalid"));
                }
                if width == 0 {
                    if index % 8 == 0 {
                        bytes.push(0);
                    }
                    if lane[0] > 1 {
                        return Err(malformed("const.vec i1 lane must be zero or one"));
                    }
                    *bytes.last_mut().expect("allocated bitmask byte") |= lane[0] << (index % 8);
                } else {
                    bytes.extend_from_slice(lane);
                }
            }
            bytes
        }
        _ => {
            return Err(malformed(
                "const.vec payload form does not match storage size",
            ));
        }
    };
    let expected_length = usize::try_from(*lanes).ok().and_then(|n| {
        if width == 0 {
            Some(n.div_ceil(8))
        } else {
            n.checked_mul(width)
        }
    });
    if Some(bytes.len()) != expected_length {
        return Err(malformed(
            "const.vec inline byte count is incompatible with type",
        ));
    }
    let mut literals = Vec::new();
    if width == 0 {
        let count = usize::try_from(*lanes)
            .map_err(|_| malformed("const.vec mask lane count overflows"))?;
        for lane in 0..count {
            literals.push(((bytes[lane / 8] >> (lane % 8)) & 1).to_string());
        }
    } else {
        for lane in bytes.chunks_exact(width) {
            match element_ty {
                BinaryType::Integer(_) => {
                    let mut value = [0u8; 8];
                    value[..width].copy_from_slice(lane);
                    literals.push(u64::from_le_bytes(value).to_string());
                }
                BinaryType::Float(float) => {
                    literals.push(
                        crate::text_global_disassembly::format_ieee_float_literal(lane, *float)
                            .ok_or_else(|| {
                                malformed("const.vec float lane cannot be formatted exactly")
                            })?,
                    );
                }
                _ => unreachable!(),
            }
        }
    }
    let name = format!("v{}", values.len());
    let type_text = crate::text_type_disassembly::format_type_reference(
        types,
        *ty,
        &crate::types::BinarySignatureTable::default(),
    )?;
    writeln!(
        text,
        "    %{name} = const.vec {type_text}, [{}]",
        literals.join(", ")
    )
    .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, *ty));
    Ok(())
}

fn ieee_float_width(float: FloatType) -> Option<u16> {
    matches!(
        float,
        FloatType::F16 | FloatType::Bf16 | FloatType::F32 | FloatType::F64 | FloatType::F128
    )
    .then_some(float.bits())
}

fn element_width(element: &Type) -> Option<u16> {
    match element {
        Type::Integer(bits @ (1 | 8 | 16 | 32 | 64)) => Some(*bits),
        Type::Float(float) => ieee_float_width(*float),
        _ => None,
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
