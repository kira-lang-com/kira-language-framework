//! Texture atomic CODE (0x48A), scalar i32/f32 operations and typed texel coordinates.
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{FloatType, TextureAccess, TextureDimension, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const OPERATIONS: [&str; 15] = [
    "xchg", "add", "sub", "and", "nand", "or", "xor", "max", "min", "umax", "umin", "fadd", "fsub",
    "fmax", "fmin",
];

pub(crate) struct TextureAtomic {
    pub(crate) result: String,
    pub(crate) inputs: Vec<String>,
    pub(crate) operation: u64,
}

pub(crate) fn handles(opcode: &str) -> bool {
    opcode == "tex.atomic"
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

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<TextureAtomic, TextParseError> {
    if !handles(&instruction.opcode) {
        return Err(error(item, instruction.span, "expected tex.atomic"));
    }
    let [name] = instruction.results.as_slice() else {
        return Err(error(
            item,
            instruction.span,
            "tex.atomic must define exactly one SSA result",
        ));
    };
    let source = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "texture atomic instruction span is invalid",
            )
        })?
        .trim();
    let prefix = format!("%{name} = tex.atomic ");
    let operands = source.strip_prefix(&prefix).ok_or_else(|| {
        error(
            item,
            instruction.span,
            "texture atomic requires an operation and SSA values",
        )
    })?;
    let (before, trailing) = operands.rsplit_once(',').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "texture atomic is missing its operation or value",
        )
    })?;
    let (operation_name, value) = trailing.trim().split_once(' ').ok_or_else(|| {
        error(
            item,
            instruction.span,
            "texture atomic requires operation followed by SSA value",
        )
    })?;
    let operation = OPERATIONS
        .iter()
        .position(|name| *name == operation_name)
        .ok_or_else(|| error(item, instruction.span, "unknown texture atomic operation"))?
        as u64;
    let value = value
        .trim()
        .strip_prefix('%')
        .filter(|value| valid_name(value))
        .ok_or_else(|| error(item, instruction.span, "texture atomic value must be SSA"))?;
    let mut inputs = before
        .split(',')
        .map(|operand| {
            operand
                .trim()
                .strip_prefix('%')
                .filter(|name| valid_name(name))
                .map(str::to_owned)
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "texture atomic texture/coordinates must be SSA",
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(inputs.len(), 2 | 3) {
        return Err(error(
            item,
            instruction.span,
            "texture atomic needs texture, coordinates and optional layer",
        ));
    }
    inputs.push(value.to_owned());
    Ok(TextureAtomic {
        result: name.clone(),
        inputs,
        operation,
    })
}

fn coordinate_lanes(dimension: TextureDimension) -> u32 {
    match dimension {
        TextureDimension::D1 | TextureDimension::D1Array => 1,
        TextureDimension::D2 | TextureDimension::D2Array => 2,
        TextureDimension::D3 | TextureDimension::Cube | TextureDimension::CubeArray => 3,
    }
}

fn is_array(dimension: TextureDimension) -> bool {
    matches!(
        dimension,
        TextureDimension::D1Array | TextureDimension::D2Array | TextureDimension::CubeArray
    )
}

fn value_type(operation: u64) -> Type {
    if operation >= 11 {
        Type::Float(FloatType::F32)
    } else {
        Type::Integer(32)
    }
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let atomic = parse(instruction, source, item)?;
    if values.contains_key(&atomic.result) {
        return Err(error(
            item,
            instruction.span,
            "texture atomic SSA result is duplicated",
        ));
    }
    let types = atomic
        .inputs
        .iter()
        .map(|name| {
            values.get(name).ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "texture atomic SSA operand is undefined",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Type::Texture {
        dimension,
        component,
        access,
        multisampled,
    } = types[0]
    else {
        return Err(error(
            item,
            instruction.span,
            "texture atomic first input must be a texture handle",
        ));
    };
    if *access != TextureAccess::ReadWrite || *multisampled {
        return Err(error(
            item,
            instruction.span,
            "texture atomics require non-multisampled readwrite textures",
        ));
    }
    let expected = value_type(atomic.operation);
    if component.as_ref() != &expected || types.last().copied() != Some(&expected) {
        return Err(error(
            item,
            instruction.span,
            "texture atomic operation/value must match scalar i32 or f32 component",
        ));
    }
    let lanes = coordinate_lanes(*dimension);
    let valid_coords = match (lanes, types[1]) {
        (1, Type::Integer(1..=64)) => true,
        (
            2 | 3,
            Type::FixedVector {
                lanes: count,
                element,
            },
        ) if *count == lanes => matches!(element.as_ref(), Type::Integer(1..=64)),
        _ => false,
    };
    if !valid_coords {
        return Err(error(
            item,
            instruction.span,
            "texture atomic coordinates must be integer texel coordinates",
        ));
    }
    if types.len() != 3 + usize::from(is_array(*dimension))
        || (is_array(*dimension) && !matches!(types[2], Type::Integer(1..=64)))
    {
        return Err(error(
            item,
            instruction.span,
            "texture atomic layer/operand count is invalid",
        ));
    }
    values.insert(atomic.result, expected);
    Ok(())
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &[&str],
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let atomic = parse(instruction, source, item)?;
    for word in [0x48a, 0, 1, atomic.inputs.len() as u64] {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    for input in &atomic.inputs {
        let pos = values
            .iter()
            .position(|value| *value == input)
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "texture atomic SSA operand is undefined",
                )
            })?;
        crate::leb::write_uleb((values.len() - pos) as u64, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    for word in [0, atomic.operation] {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if instruction.opcode != 0x48a || instruction.modifiers != 0 || instruction.result_count != 1 {
        return Err(malformed(
            "tex.atomic CODE has invalid opcode/modifier/result count",
        ));
    }
    let [
        BinaryOperand::Values(indices),
        BinaryOperand::Integers(offsets),
        BinaryOperand::Index(operation),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "tex.atomic CODE requires SSA values, empty offsets and an operation",
        ));
    };
    if !offsets.is_empty() {
        return Err(malformed("tex.atomic does not support constant offsets"));
    }
    let operation_name = usize::try_from(*operation)
        .ok()
        .and_then(|index| OPERATIONS.get(index))
        .ok_or_else(|| malformed("unknown texture atomic operation"))?;
    let inputs = indices
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.as_str(), *ty))
                .ok_or_else(|| malformed("tex.atomic references undefined SSA values"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !(inputs.len() == 3 || inputs.len() == 4) {
        return Err(malformed("texture atomic SSA operand count must be 3 or 4"));
    }
    let Some(BinaryType::Texture {
        dimension,
        component,
        access,
        multisampled,
    }) = types.entries.get(inputs[0].1 as usize)
    else {
        return Err(malformed(
            "texture atomic first operand is not a texture handle",
        ));
    };
    if *access != 3 || *multisampled {
        return Err(malformed(
            "texture atomic requires non-multisampled readwrite texture",
        ));
    }
    let lanes = match dimension {
        0 | 4 => 1,
        1 | 5 => 2,
        2 | 3 | 6 => 3,
        _ => return Err(malformed("unknown texture dimension")),
    };
    let valid_coords = match (lanes, types.entries.get(inputs[1].1 as usize)) {
        (1, Some(BinaryType::Integer(1..=64))) => true,
        (
            2 | 3,
            Some(BinaryType::FixedVector {
                lanes: count,
                element,
            }),
        ) if *count == lanes => matches!(
            types.entries.get(*element as usize),
            Some(BinaryType::Integer(1..=64))
        ),
        _ => false,
    };
    if !valid_coords {
        return Err(malformed("texture atomic texel coordinate type is invalid"));
    }
    let array = (4..=6).contains(dimension);
    if inputs.len() != 3 + usize::from(array)
        || (array
            && !matches!(
                types.entries.get(inputs[2].1 as usize),
                Some(BinaryType::Integer(1..=64))
            ))
    {
        return Err(malformed(
            "texture atomic array layer/operand count is invalid",
        ));
    }
    let expected = if *operation >= 11 {
        BinaryType::Float(FloatType::F32)
    } else {
        BinaryType::Integer(32)
    };
    let component_at = types.entries.get(*component as usize);
    if component_at != Some(&expected)
        || types
            .entries
            .get(inputs.last().expect("checked arity").1 as usize)
            != Some(&expected)
    {
        return Err(malformed(
            "texture atomic value/component type is incompatible with operation",
        ));
    }
    let name = format!("v{}", values.len());
    write!(text, "    %{name} = tex.atomic").map_err(|issue| malformed(issue.to_string()))?;
    for (position, (value, _)) in inputs.iter().take(inputs.len() - 1).enumerate() {
        write!(text, "{}%{value}", if position == 0 { " " } else { ", " })
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    write!(
        text,
        ", {operation_name} %{}\n",
        inputs.last().expect("checked arity").0
    )
    .map_err(|issue| malformed(issue.to_string()))?;
    values.push((name, *component));
    Ok(())
}
