//! Typed texture queries and raw texel read/write (CODE 0x488, 0x489, 0x48b).
use std::collections::HashMap;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_types::{TextureAccess, TextureDimension, Type};

use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};
use crate::{FormatError, TextInstruction, TextItem, TextParseError};

const FIELDS: [&str; 7] = [
    "width", "height", "depth", "size", "layers", "levels", "samples",
];
const QUERY_SELECTOR_SHIFT: u64 = 48;
const SPARSE: u64 = 1 << 35;

pub(crate) struct TextureOp {
    opcode: u64,
    result: Option<String>,
    residency: Option<String>,
    pub(crate) arguments: Vec<String>,
    selector: u64,
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(opcode, "tex.query" | "tex.read" | "tex.write")
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<TextureOp, TextParseError> {
    let opcode = match instruction.opcode.as_str() {
        "tex.read" => 0x488,
        "tex.write" => 0x489,
        "tex.query" => 0x48b,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "expected tex.query/read/write",
            ));
        }
    };
    let sparse = source
        .get(instruction.span.start..instruction.span.end)
        .is_some_and(|text| text.trim_end().ends_with(" [sparse]"));
    let (result, residency) = match (opcode, sparse, instruction.results.as_slice()) {
        (0x489, false, []) => (None, None),
        (0x488 | 0x48b, false, [name]) => (Some(name.clone()), None),
        (0x488, true, [name, residency]) if name != residency => {
            (Some(name.clone()), Some(residency.clone()))
        }
        _ => {
            return Err(error(
                item,
                instruction.span,
                "texture CODE has invalid SSA result count",
            ));
        }
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "texture instruction source span is invalid",
            )
        })?
        .trim();
    let prefix = match (&result, &residency) {
        (Some(name), Some(residency)) => format!("%{name}, %{residency} = {}", instruction.opcode),
        (Some(name), None) => format!("%{name} = {}", instruction.opcode),
        (None, _) => instruction.opcode.clone(),
    };
    let operands = line
        .strip_prefix(&prefix)
        .filter(|text| text.starts_with(' '))
        .ok_or_else(|| error(item, instruction.span, "invalid texture CODE operands"))?
        .trim();
    let operands = if sparse {
        operands
            .strip_suffix(" [sparse]")
            .ok_or_else(|| error(item, instruction.span, "texture sparse flag is misplaced"))?
    } else {
        operands
    };
    let mut fields = operands.split(',').map(str::trim).collect::<Vec<_>>();
    let selector = if opcode == 0x48b {
        let selector = fields
            .pop()
            .and_then(|field| FIELDS.iter().position(|name| *name == field))
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "tex.query requires a valid query field",
                )
            })?;
        selector as u64
    } else {
        0
    };
    if fields.is_empty()
        || fields
            .iter()
            .any(|text| !text.starts_with('%') || !valid_name(&text[1..]))
    {
        return Err(error(
            item,
            instruction.span,
            "texture operands must be comma-separated SSA names",
        ));
    }
    if opcode == 0x48b && fields.len() != 1 {
        return Err(error(
            item,
            instruction.span,
            "tex.query requires exactly one texture handle",
        ));
    }
    Ok(TextureOp {
        opcode,
        result,
        residency,
        selector,
        arguments: fields
            .into_iter()
            .map(|text| text[1..].to_owned())
            .collect(),
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

pub(crate) fn query_type(dimension: TextureDimension, selector: u64) -> Option<Type> {
    if selector >= FIELDS.len() as u64 {
        return None;
    }
    let lanes = if selector == 3 {
        match dimension {
            TextureDimension::D1 => 1,
            TextureDimension::D1Array | TextureDimension::D2 | TextureDimension::Cube => 2,
            TextureDimension::D2Array | TextureDimension::D3 | TextureDimension::CubeArray => 3,
        }
    } else {
        1
    };
    Some(if lanes == 1 {
        Type::Integer(32)
    } else {
        Type::FixedVector {
            lanes,
            element: Box::new(Type::Integer(32)),
        }
    })
}

/// Returns a CODE-local result root when the texture handle is a typed block
/// parameter. In particular, an unused `tex.query` may produce a vec2/vec3
/// result not otherwise named by a function signature or another type root.
pub(crate) fn required_result_root(
    instruction: &TextInstruction,
    body: &crate::FunctionBody,
    source: &str,
    item: TextItem,
) -> Result<Option<Type>, TextParseError> {
    let op = parse(instruction, source, item)?;
    if op.result.is_none() {
        return Ok(None);
    }
    let Some(ty) = body
        .blocks
        .iter()
        .flat_map(|block| &block.parameters)
        .find(|parameter| parameter.name == op.arguments[0])
        .map(|parameter| &parameter.ty)
    else {
        return Ok(None);
    };
    let Type::Texture {
        dimension,
        component,
        ..
    } = ty
    else {
        return Err(error(
            item,
            instruction.span,
            "texture operand is not a texture handle",
        ));
    };
    Ok(Some(if op.opcode == 0x48b {
        query_type(*dimension, op.selector).expect("query selector checked by parser")
    } else {
        component.as_ref().clone()
    }))
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    if op
        .result
        .as_ref()
        .is_some_and(|name| values.contains_key(name))
    {
        return Err(error(
            item,
            instruction.span,
            "duplicate texture SSA result",
        ));
    }
    if op
        .residency
        .as_ref()
        .is_some_and(|name| values.contains_key(name))
    {
        return Err(error(
            item,
            instruction.span,
            "duplicate texture residency SSA result",
        ));
    }
    let inputs = op
        .arguments
        .iter()
        .map(|name| {
            values
                .get(name)
                .ok_or_else(|| error(item, instruction.span, "texture operand is undefined"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Type::Texture {
        dimension,
        component,
        access,
        multisampled,
    } = inputs[0]
    else {
        return Err(error(
            item,
            instruction.span,
            "first texture operand must be a texture handle",
        ));
    };
    let result_ty = match op.opcode {
        0x48b => query_type(*dimension, op.selector).expect("validated selector"),
        0x488 | 0x489 => {
            if !matches!(
                (op.opcode, access),
                (0x488, TextureAccess::Read | TextureAccess::ReadWrite)
                    | (0x489, TextureAccess::Write | TextureAccess::ReadWrite)
            ) {
                return Err(error(
                    item,
                    instruction.span,
                    "texture access mode forbids this operation",
                ));
            }
            let dims = coordinate_lanes(*dimension);
            let shape_ok = match (dims, inputs.get(1)) {
                (1, Some(Type::Integer(1..=64))) => true,
                (2 | 3, Some(Type::FixedVector { lanes, element })) if *lanes == dims => {
                    matches!(element.as_ref(), Type::Integer(1..=64))
                }
                _ => false,
            };
            if !shape_ok {
                return Err(error(
                    item,
                    instruction.span,
                    "texel coordinate shape must match texture dimension",
                ));
            }
            let layer = is_array(*dimension);
            let mut cursor = 2;
            if layer {
                if !inputs
                    .get(cursor)
                    .is_some_and(|ty| matches!(ty, Type::Integer(1..=64)))
                {
                    return Err(error(
                        item,
                        instruction.span,
                        "texture array layer must be integer",
                    ));
                }
                cursor += 1;
            }
            if *multisampled {
                if !inputs
                    .get(cursor)
                    .is_some_and(|ty| matches!(ty, Type::Integer(1..=64)))
                {
                    return Err(error(
                        item,
                        instruction.span,
                        "multisampled texture requires an integer sample index",
                    ));
                }
                cursor += 1;
            }
            if op.opcode == 0x489 {
                if inputs.get(cursor).copied() != Some(component.as_ref()) {
                    return Err(error(
                        item,
                        instruction.span,
                        "tex.write payload must match texture component type",
                    ));
                }
                cursor += 1;
            }
            if inputs.len() != cursor {
                return Err(error(
                    item,
                    instruction.span,
                    "texture read/write operand count is invalid",
                ));
            }
            component.as_ref().clone()
        }
        _ => unreachable!(),
    };
    if let Some(name) = op.result {
        values.insert(name, result_ty);
    }
    if let Some(name) = op.residency {
        values.insert(name, Type::Integer(1));
    }
    Ok(())
}

fn is_array(dimension: TextureDimension) -> bool {
    matches!(
        dimension,
        TextureDimension::D1Array | TextureDimension::D2Array | TextureDimension::CubeArray
    )
}
fn coordinate_lanes(dimension: TextureDimension) -> u32 {
    match dimension {
        TextureDimension::D1 | TextureDimension::D1Array => 1,
        TextureDimension::D2 | TextureDimension::D2Array => 2,
        TextureDimension::D3 | TextureDimension::Cube | TextureDimension::CubeArray => 3,
    }
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &[&str],
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    let words = [
        op.opcode,
        (op.selector << QUERY_SELECTOR_SHIFT) | if op.residency.is_some() { SPARSE } else { 0 },
        u64::from(op.result.is_some()) + u64::from(op.residency.is_some()),
        op.arguments.len() as u64,
    ];
    for word in words {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    for name in &op.arguments {
        let pos = values
            .iter()
            .position(|value| *value == name)
            .ok_or_else(|| error(item, instruction.span, "undefined texture SSA operand"))?;
        crate::leb::write_uleb((values.len() - pos) as u64, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    crate::leb::write_uleb(0, bytes)
        .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    Ok(())
}

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    if !matches!(instruction.opcode, 0x488 | 0x489 | 0x48b) {
        return Err(malformed("unexpected texture CODE opcode"));
    }
    let selector = (instruction.modifiers >> QUERY_SELECTOR_SHIFT) & 0xf;
    let sparse = instruction.opcode == 0x488 && instruction.modifiers & SPARSE != 0;
    let allowed = if instruction.opcode == 0x488 {
        SPARSE
    } else {
        0xf << QUERY_SELECTOR_SHIFT
    };
    if instruction.modifiers & !allowed != 0
        || (instruction.opcode != 0x48b && selector != 0)
        || (instruction.opcode == 0x48b && selector >= FIELDS.len() as u64)
        || instruction.result_count != u64::from(instruction.opcode != 0x489) + u64::from(sparse)
    {
        return Err(malformed(
            "invalid texture query/read/write modifiers, selector or results",
        ));
    }
    let [
        BinaryOperand::Values(indices),
        BinaryOperand::Integers(offsets),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed("texture CODE requires values and offset fields"));
    };
    if !offsets.is_empty() || indices.is_empty() {
        return Err(malformed("texture CODE offsets or SSA count are invalid"));
    }
    let operands = indices
        .iter()
        .map(|idx| {
            usize::try_from(*idx)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.clone(), *ty))
                .ok_or_else(|| malformed("texture CODE has an invalid SSA value index"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let Some(BinaryType::Texture {
        dimension,
        component,
        access,
        multisampled,
    }) = types.entries.get(operands[0].1 as usize)
    else {
        return Err(malformed(
            "texture CODE first operand must be a texture handle",
        ));
    };
    let name = match instruction.opcode {
        0x488 => "tex.read",
        0x489 => "tex.write",
        _ => "tex.query",
    };
    let ty_index = if instruction.opcode == 0x48b {
        if operands.len() != 1 {
            return Err(malformed("tex.query expects a single texture handle"));
        }
        let dimension =
            dim_from_binary(*dimension).ok_or_else(|| malformed("invalid texture dimension"))?;
        let expected =
            query_type(dimension, selector).ok_or_else(|| malformed("invalid query selector"))?;
        crate::text_descriptor_reference::type_index(types, &expected)
            .ok_or_else(|| malformed("texture query result TYPES entry is absent"))? as u64
    } else {
        let dim =
            dim_from_binary(*dimension).ok_or_else(|| malformed("invalid texture dimension"))?;
        let needed = 2
            + usize::from(is_array(dim))
            + usize::from(*multisampled)
            + usize::from(instruction.opcode == 0x489);
        if operands.len() != needed
            || !matches!(
                (instruction.opcode, *access),
                (0x488, 1 | 3) | (0x489, 2 | 3)
            )
        {
            return Err(malformed("texture access or read/write SSA count invalid"));
        }
        let coordinate_ty = types
            .entries
            .get(operands[1].1 as usize)
            .ok_or_else(|| malformed("texel coordinate TYPES is invalid"))?;
        let lanes = coordinate_lanes(dim) as u64;
        let coords_valid = match (lanes, coordinate_ty) {
            (1, BinaryType::Integer(1..=64)) => true,
            (
                2 | 3,
                BinaryType::FixedVector {
                    lanes: actual,
                    element,
                },
            ) if *actual == lanes => matches!(
                types.entries.get(*element as usize),
                Some(BinaryType::Integer(1..=64))
            ),
            _ => false,
        };
        if !coords_valid {
            return Err(malformed("texture coordinate TYPES shape invalid"));
        }
        let mut cursor = 2;
        if is_array(dim) || *multisampled {
            for _ in 0..usize::from(is_array(dim)) + usize::from(*multisampled) {
                if !matches!(
                    types.entries.get(operands[cursor].1 as usize),
                    Some(BinaryType::Integer(1..=64))
                ) {
                    return Err(malformed(
                        "texture layer or sample index has non-integer type",
                    ));
                }
                cursor += 1;
            }
        }
        if instruction.opcode == 0x489 && operands[cursor].1 != *component {
            return Err(malformed("texture write component TYPES mismatch"));
        }
        *component
    };
    let result_name = (instruction.opcode != 0x489).then(|| format!("v{}", values.len()));
    let residency_name = sparse.then(|| format!("v{}", values.len() + 1));
    if let Some(result) = &result_name {
        write!(text, "    %{result}").map_err(|issue| malformed(issue.to_string()))?;
        if let Some(residency) = &residency_name {
            write!(text, ", %{residency}").map_err(|issue| malformed(issue.to_string()))?;
        }
        write!(text, " = {name}").map_err(|issue| malformed(issue.to_string()))?;
    } else {
        write!(text, "    {name}").map_err(|issue| malformed(issue.to_string()))?;
    }
    for (index, (name, _)) in operands.iter().enumerate() {
        write!(text, "{}%{name}", if index == 0 { " " } else { ", " })
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if instruction.opcode == 0x48b {
        write!(text, ", {}", FIELDS[selector as usize])
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if sparse {
        text.push_str(" [sparse]");
    }
    text.push('\n');
    if let Some(name) = result_name {
        values.push((name, ty_index));
    }
    if let Some(name) = residency_name {
        let ty = crate::text_descriptor_reference::type_index(types, &Type::Integer(1))
            .ok_or_else(|| malformed("sparse texture residency i1 TYPES entry is missing"))?;
        values.push((name, ty as u64));
    }
    Ok(())
}

fn dim_from_binary(value: u8) -> Option<TextureDimension> {
    Some(match value {
        0 => TextureDimension::D1,
        1 => TextureDimension::D2,
        2 => TextureDimension::D3,
        3 => TextureDimension::Cube,
        4 => TextureDimension::D1Array,
        5 => TextureDimension::D2Array,
        6 => TextureDimension::CubeArray,
        _ => return None,
    })
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
