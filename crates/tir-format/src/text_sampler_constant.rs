use std::collections::HashSet;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{FloatType, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

pub(crate) struct SamplerConstant {
    pub(crate) name: String,
    descriptor: [u8; 23],
}

fn defaults() -> [u8; 23] {
    let mut descriptor = [0_u8; 23];
    descriptor[7] = 1;
    descriptor[8] = u8::MAX;
    descriptor[10] = 1;
    descriptor[15..19].copy_from_slice(&f32::INFINITY.to_bits().to_le_bytes());
    descriptor
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<SamplerConstant, TextParseError> {
    if instruction.opcode != "const.sampler" {
        return Err(error(item, instruction.span, "expected const.sampler"));
    }
    let [name] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "const.sampler requires one SSA result",
        ));
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid sampler source span"))?
        .trim();
    let prefix = format!("%{name} = const.sampler {{");
    let fields = line
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix('}'))
        .ok_or_else(|| error(item, instruction.span, "const.sampler requires { fields }"))?;
    let mut descriptor = defaults();
    let mut seen = HashSet::new();
    let field_list = if fields.trim().is_empty() {
        Vec::new()
    } else {
        fields.split(',').map(str::trim).collect::<Vec<_>>()
    };
    for field in field_list {
        if field.is_empty() {
            return Err(error(item, instruction.span, "empty const.sampler field"));
        }
        let (name, value) = field
            .split_once(':')
            .ok_or_else(|| error(item, instruction.span, "sampler fields require name: value"))?;
        let (name, value) = (name.trim(), value.trim());
        if !seen.insert(name) {
            return Err(error(
                item,
                instruction.span,
                "duplicate const.sampler field",
            ));
        }
        let slot = match name {
            "min" => Some((0, &["nearest", "linear"][..])),
            "mag" => Some((1, &["nearest", "linear"][..])),
            "mip" => Some((2, &["nearest", "linear"][..])),
            "address_u" => Some((3, &ADDRESS_MODES[..])),
            "address_v" => Some((4, &ADDRESS_MODES[..])),
            "address_w" => Some((5, &ADDRESS_MODES[..])),
            "border" => Some((
                6,
                &["transparent_black", "opaque_black", "opaque_white"][..],
            )),
            "compare" => Some((8, &COMPARE_MODES[..])),
            "reduction" => Some((9, &["weighted_average", "min", "max"][..])),
            _ => None,
        };
        if let Some((index, choices)) = slot {
            descriptor[index] = choices
                .iter()
                .position(|choice| *choice == value)
                .and_then(|index| u8::try_from(index).ok())
                .ok_or_else(|| error(item, instruction.span, "unrecognized sampler enum value"))?;
        } else {
            match name {
                "max_anisotropy" => {
                    descriptor[7] = value
                        .parse::<u8>()
                        .ok()
                        .filter(|value| (1..=16).contains(value))
                        .ok_or_else(|| {
                            error(item, instruction.span, "sampler anisotropy must be 1..16")
                        })?;
                }
                "normalized_coordinates" => {
                    descriptor[10] = match value {
                        "true" => 1,
                        "false" => 0,
                        _ => {
                            return Err(error(
                                item,
                                instruction.span,
                                "normalized_coordinates requires true or false",
                            ));
                        }
                    };
                }
                "lod_min" | "lod_max" | "lod_bias" => {
                    let offset = match name {
                        "lod_min" => 11,
                        "lod_max" => 15,
                        _ => 19,
                    };
                    let bytes = crate::numeric_literals::parse_ieee_float_literal_bytes(
                        value,
                        FloatType::F32,
                    )
                    .ok_or_else(|| {
                        error(
                            item,
                            instruction.span,
                            "sampler LOD must be exactly representable f32",
                        )
                    })?;
                    descriptor[offset..offset + 4].copy_from_slice(&bytes);
                }
                _ => {
                    return Err(error(
                        item,
                        instruction.span,
                        "unrecognized const.sampler field",
                    ));
                }
            }
        }
    }
    Ok(SamplerConstant {
        name: name.clone(),
        descriptor,
    })
}

pub(crate) fn validate(
    constant: &SamplerConstant,
    values: &std::collections::HashMap<String, Type>,
    item: TextItem,
    span: Span,
) -> Result<(String, Type), TextParseError> {
    if values.contains_key(&constant.name) {
        return Err(error(item, span, "duplicate sampler SSA result"));
    }
    Ok((constant.name.clone(), Type::Sampler))
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let constant = parse(instruction, source, item)?;
    for word in [0x011, 0, 1] {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    bytes.extend_from_slice(&constant.descriptor);
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x011 || instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "const.sampler requires zero modifiers and one result",
        ));
    }
    let [BinaryOperand::Bytes(bytes)] = instruction.operands.as_slice() else {
        return Err(malformed("const.sampler must have a 23-byte payload"));
    };
    if bytes.len() != 23 {
        return Err(malformed("const.sampler descriptor length must be 23"));
    }
    let mut fields = Vec::new();
    let defaults = defaults();
    for (index, name, choices) in [
        (0, "min", &["nearest", "linear"][..]),
        (1, "mag", &["nearest", "linear"][..]),
        (2, "mip", &["nearest", "linear"][..]),
        (3, "address_u", &ADDRESS_MODES[..]),
        (4, "address_v", &ADDRESS_MODES[..]),
        (5, "address_w", &ADDRESS_MODES[..]),
        (
            6,
            "border",
            &["transparent_black", "opaque_black", "opaque_white"][..],
        ),
        (8, "compare", &COMPARE_MODES[..]),
        (9, "reduction", &["weighted_average", "min", "max"][..]),
    ] {
        if bytes[index] != defaults[index] {
            let choice = choices
                .get(bytes[index] as usize)
                .ok_or_else(|| malformed("const.sampler has unrecognized descriptor value"))?;
            fields.push(format!("{name}: {choice}"));
        }
    }
    if bytes[7] != defaults[7] {
        fields.push(format!("max_anisotropy: {}", bytes[7]));
    }
    if bytes[10] != defaults[10] {
        fields.push(format!(
            "normalized_coordinates: {}",
            if bytes[10] == 0 { "false" } else { "true" }
        ));
    }
    for (offset, name) in [(11, "lod_min"), (15, "lod_max"), (19, "lod_bias")] {
        if bytes[offset..offset + 4] != defaults[offset..offset + 4] {
            let value = crate::text_global_disassembly::format_ieee_float_literal(
                &bytes[offset..offset + 4],
                FloatType::F32,
            )
            .ok_or_else(|| malformed("const.sampler has unrepresentable LOD bits"))?;
            fields.push(format!("{name}: {value}"));
        }
    }
    let ty = types
        .entries
        .iter()
        .position(|ty| matches!(ty, BinaryType::Sampler))
        .ok_or_else(|| malformed("const.sampler requires sampler in TYPES"))?;
    let name = format!("v{}", values.len());
    writeln!(
        text,
        "    %{name} = const.sampler {{{}}}",
        if fields.is_empty() {
            String::new()
        } else {
            format!(" {} ", fields.join(", "))
        }
    )
    .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, ty as u64));
    Ok(())
}

const ADDRESS_MODES: [&str; 5] = [
    "clamp_to_edge",
    "repeat",
    "mirrored_repeat",
    "clamp_to_border",
    "mirror_clamp_to_edge",
];
const COMPARE_MODES: [&str; 8] = [
    "never",
    "less",
    "equal",
    "less_equal",
    "greater",
    "not_equal",
    "greater_equal",
    "always",
];
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
