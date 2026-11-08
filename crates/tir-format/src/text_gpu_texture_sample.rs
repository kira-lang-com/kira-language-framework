//! Typed sampler CODE (0x480–0x487, 0x48c–0x48d), with optional integer offsets.
use std::collections::HashMap;

use tir_diag::Span;
use tir_types::{FloatType, TextureAccess, TextureDimension, Type};

use crate::{TextInstruction, TextItem, TextParseError};

pub(crate) const UNCHECKED: u64 = 1 << 34;
pub(crate) const SPARSE: u64 = 1 << 35;

pub(crate) struct SampleOp {
    pub(crate) opcode: u64,
    pub(crate) result: String,
    pub(crate) residency: Option<String>,
    pub(crate) operands: Vec<String>,
    pub(crate) offsets: Vec<i8>,
    pub(crate) min_lod: bool,
    pub(crate) unchecked: bool,
    pub(crate) selector: u64,
}

pub(crate) fn handles(opcode: &str) -> bool {
    matches!(
        opcode,
        "tex.sample"
            | "tex.sample_level"
            | "tex.sample_grad"
            | "tex.sample_bias"
            | "tex.sample_compare"
            | "tex.sample_compare_level"
            | "tex.gather"
            | "tex.gather_compare"
            | "tex.lod"
            | "tex.feedback"
    )
}

pub(crate) fn parse(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
) -> Result<SampleOp, TextParseError> {
    let opcode = match instruction.opcode.as_str() {
        "tex.sample" => 0x480,
        "tex.sample_level" => 0x481,
        "tex.sample_grad" => 0x482,
        "tex.sample_bias" => 0x483,
        "tex.sample_compare" => 0x484,
        "tex.sample_compare_level" => 0x485,
        "tex.gather" => 0x486,
        "tex.gather_compare" => 0x487,
        "tex.lod" => 0x48c,
        "tex.feedback" => 0x48d,
        _ => {
            return Err(error(
                item,
                instruction.span,
                "expected tex.sample or tex.lod",
            ));
        }
    };
    let sparse = source
        .get(instruction.span.start..instruction.span.end)
        .is_some_and(|text| text.trim_end().ends_with(" [sparse]"));
    let (result, residency) = match (opcode, sparse, instruction.results.as_slice()) {
        (_, false, [name]) => (name.clone(), None),
        (0x480..=0x487 | 0x48d, true, [name, residency]) if name != residency => {
            (name.clone(), Some(residency.clone()))
        }
        _ => {
            return Err(error(
                item,
                instruction.span,
                "texture sampling requires one result, or value plus residency for [sparse]",
            ));
        }
    };
    let line = source
        .get(instruction.span.start..instruction.span.end)
        .ok_or_else(|| error(item, instruction.span, "invalid texture sampling span"))?
        .trim();
    let prefix = if let Some(residency) = &residency {
        format!("%{result}, %{residency} = {} ", instruction.opcode)
    } else {
        format!("%{result} = {} ", instruction.opcode)
    };
    let mut rest = line
        .strip_prefix(&prefix)
        .ok_or_else(|| {
            error(
                item,
                instruction.span,
                "texture sampling source has invalid operands",
            )
        })?
        .trim();
    if sparse {
        rest = rest
            .strip_suffix(" [sparse]")
            .ok_or_else(|| error(item, instruction.span, "sparse suffix is misplaced"))?
            .trim_end();
    }
    let unchecked = rest.ends_with(" [unchecked_uniformity]");
    if unchecked && matches!(opcode, 0x481 | 0x482 | 0x485) {
        return Err(error(
            item,
            instruction.span,
            "explicit LOD/gradient/comparison-level sampling has no unchecked_uniformity modifier",
        ));
    }
    if unchecked {
        rest = rest
            .strip_suffix(" [unchecked_uniformity]")
            .expect("suffix checked")
            .trim_end();
    }
    let mut min_lod = false;
    let mut min_lod_name = None;
    if let Some((before, name)) = rest.rsplit_once(" min_lod ") {
        if opcode == 0x48c || !name.starts_with('%') || !valid_name(&name[1..]) {
            return Err(error(
                item,
                instruction.span,
                "tex.sample min_lod requires one SSA value",
            ));
        }
        rest = before.trim_end();
        min_lod = true;
        min_lod_name = Some(name[1..].to_owned());
    }
    let mut offsets = Vec::new();
    if let Some((before, spec)) = rest.rsplit_once(" offset ") {
        if opcode == 0x48c {
            return Err(error(
                item,
                instruction.span,
                "tex.lod cannot carry a constant offset",
            ));
        }
        let fields = spec
            .strip_prefix('[')
            .and_then(|text| text.strip_suffix(']'))
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "tex.sample offset must be bracketed",
                )
            })?;
        for number in fields.split(',').map(str::trim) {
            let parsed = crate::numeric_literals::parse_signed_integer(number)
                .and_then(|value| i8::try_from(value).ok())
                .filter(|value| (-8..=7).contains(value))
                .ok_or_else(|| error(item, instruction.span, "texture offset must be in -8..=7"))?;
            offsets.push(parsed);
        }
        rest = before.trim_end();
        if offsets.is_empty() || offsets.len() > 3 {
            return Err(error(
                item,
                instruction.span,
                "texture offset needs one to three components",
            ));
        }
    }
    if rest.contains(" offset ") || rest.contains(" min_lod ") || rest.contains('[') {
        return Err(error(
            item,
            instruction.span,
            "texture options are duplicated or out of order",
        ));
    }
    let mandatory_operand = match opcode {
        0x481 => Some(", lod "),
        0x482 => Some(" grad "),
        0x483 => Some(", bias "),
        0x484 | 0x485 => Some(" compare "),
        _ => None,
    };
    let mandatory_name = if let Some(label) = mandatory_operand {
        let (before, name) = rest.rsplit_once(label).ok_or_else(|| {
            error(
                item,
                instruction.span,
                "sample_level/sample_bias requires a LOD/bias SSA operand",
            )
        })?;
        let names = if opcode == 0x485 {
            let (reference, lod) = name.split_once(", lod ").ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "comparison-level sampling requires compare %ref, lod %level",
                )
            })?;
            vec![reference.trim(), lod.trim()]
        } else {
            name.split(',').map(str::trim).collect::<Vec<_>>()
        };
        if names.len()
            != if matches!(opcode, 0x482 | 0x485) {
                2
            } else {
                1
            }
            || names
                .iter()
                .any(|name| !name.starts_with('%') || !valid_name(&name[1..]))
        {
            return Err(error(
                item,
                instruction.span,
                "LOD/bias/compare requires typed SSA operands; grad/compare_level require two",
            ));
        }
        rest = before.trim_end();
        Some(
            names
                .into_iter()
                .map(|name| name[1..].to_owned())
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };
    let selector = if opcode == 0x486 {
        if let Some((before, raw)) = rest.rsplit_once(" component ") {
            let parsed = crate::numeric_literals::parse_unsigned_integer(raw)
                .and_then(|value| u64::try_from(value).ok())
                .filter(|value| *value <= 3)
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "tex.gather component selector must be an integer 0–3",
                    )
                })?;
            rest = before.trim_end();
            parsed
        } else {
            0
        }
    } else {
        0
    };
    let mut operands = rest
        .split(',')
        .map(str::trim)
        .map(|value| {
            value
                .strip_prefix('%')
                .filter(|name| valid_name(name))
                .map(str::to_owned)
                .ok_or_else(|| {
                    error(
                        item,
                        instruction.span,
                        "texture operands must be SSA references",
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(operands.len(), 3 | 4) && !(opcode == 0x487 && operands.len() == 5) {
        return Err(error(
            item,
            instruction.span,
            "texture sampling requires texture, sampler, coordinates and optional layer",
        ));
    }
    if let Some(names) = mandatory_name {
        operands.extend(names);
    }
    if let Some(name) = min_lod_name {
        operands.push(name);
    }
    Ok(SampleOp {
        opcode,
        result,
        residency,
        operands,
        offsets,
        min_lod,
        unchecked,
        selector,
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.'))
}

pub(crate) fn coordinate_lanes(dimension: TextureDimension) -> usize {
    match dimension {
        TextureDimension::D1 | TextureDimension::D1Array => 1,
        TextureDimension::D2 | TextureDimension::D2Array => 2,
        TextureDimension::D3 | TextureDimension::Cube | TextureDimension::CubeArray => 3,
    }
}
pub(crate) fn is_array(dimension: TextureDimension) -> bool {
    matches!(
        dimension,
        TextureDimension::D1Array | TextureDimension::D2Array | TextureDimension::CubeArray
    )
}
pub(crate) fn offset_lanes(dimension: TextureDimension) -> usize {
    match dimension {
        TextureDimension::Cube | TextureDimension::CubeArray => 2,
        other => coordinate_lanes(other),
    }
}
pub(crate) fn result_type(opcode: u64, component: &Type) -> Type {
    if opcode == 0x48c {
        Type::FixedVector {
            lanes: 2,
            element: Box::new(Type::Float(FloatType::F32)),
        }
    } else if matches!(opcode, 0x486 | 0x487) {
        let element = if opcode == 0x487 {
            Type::Float(FloatType::F32)
        } else if let Type::FixedVector { element, .. } = component {
            element.as_ref().clone()
        } else {
            component.clone()
        };
        Type::FixedVector {
            lanes: 4,
            element: Box::new(element),
        }
    } else {
        component.clone()
    }
}

pub(crate) fn validate(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &mut HashMap<String, Type>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    if values.contains_key(&op.result)
        || op
            .residency
            .as_ref()
            .is_some_and(|name| values.contains_key(name))
    {
        return Err(error(
            item,
            instruction.span,
            "duplicate texture sampling SSA result",
        ));
    }
    let types = op
        .operands
        .iter()
        .map(|name| {
            values.get(name).ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "texture sampling operand is undefined",
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
            "texture sample requires a texture handle",
        ));
    };
    if !matches!(access, TextureAccess::Sample | TextureAccess::ReadWrite) || *multisampled {
        return Err(error(
            item,
            instruction.span,
            "sampler operation requires sampleable non-multisampled texture",
        ));
    }
    if types[1] != &Type::Sampler {
        return Err(error(
            item,
            instruction.span,
            "sampler operation requires a sampler handle",
        ));
    }
    let coordinate_valid = match (coordinate_lanes(*dimension), types[2]) {
        (1, Type::Float(FloatType::F16 | FloatType::F32 | FloatType::F64)) => true,
        (
            lanes @ 2..=3,
            Type::FixedVector {
                lanes: actual,
                element,
            },
        ) if *actual as usize == lanes => matches!(
            element.as_ref(),
            Type::Float(FloatType::F16 | FloatType::F32 | FloatType::F64)
        ),
        _ => false,
    };
    if !coordinate_valid {
        return Err(error(
            item,
            instruction.span,
            "normalized texture coordinates must match dimension",
        ));
    }
    let mut count = 3;
    if is_array(*dimension) {
        if !matches!(types.get(count), Some(Type::Integer(1..=64))) {
            return Err(error(
                item,
                instruction.span,
                "array texture layer must be integer",
            ));
        }
        count += 1;
    }
    if matches!(op.opcode, 0x481 | 0x483) {
        if types.get(count).copied() != Some(&Type::Float(FloatType::F32)) {
            return Err(error(
                item,
                instruction.span,
                "texture LOD/bias operand must have type f32",
            ));
        }
        count += 1;
    }
    if matches!(op.opcode, 0x484 | 0x485 | 0x487) {
        if component.as_ref() != &Type::Float(FloatType::F32)
            || types.get(count).copied() != Some(&Type::Float(FloatType::F32))
        {
            return Err(error(
                item,
                instruction.span,
                "comparison sampling requires an f32 texture component and f32 reference",
            ));
        }
        count += 1;
        if op.opcode == 0x485 {
            if types.get(count).copied() != Some(&Type::Float(FloatType::F32)) {
                return Err(error(
                    item,
                    instruction.span,
                    "comparison-level LOD operand must have type f32",
                ));
            }
            count += 1;
        }
    }
    if op.opcode == 0x482 {
        let grad_ty = match coordinate_lanes(*dimension) {
            1 => Type::Float(FloatType::F32),
            lanes => Type::FixedVector {
                lanes: lanes as u32,
                element: Box::new(Type::Float(FloatType::F32)),
            },
        };
        if types.get(count).copied() != Some(&grad_ty)
            || types.get(count + 1).copied() != Some(&grad_ty)
        {
            return Err(error(
                item,
                instruction.span,
                "texture gradients must be f32 values with the coordinate shape",
            ));
        }
        count += 2;
    }
    if op.min_lod {
        if types.get(count).copied() != Some(&Type::Float(FloatType::F32)) {
            return Err(error(
                item,
                instruction.span,
                "min_lod operand must have type f32",
            ));
        }
        count += 1;
    }
    if types.len() != count || (op.opcode == 0x48c && op.min_lod) {
        return Err(error(
            item,
            instruction.span,
            "texture sample/LOD has invalid operand count",
        ));
    }
    if !op.offsets.is_empty() && op.offsets.len() != offset_lanes(*dimension) {
        return Err(error(
            item,
            instruction.span,
            "texture offset width must match spatial dimension",
        ));
    }
    values.insert(op.result, result_type(op.opcode, component));
    if let Some(name) = op.residency {
        values.insert(name, Type::Integer(1));
    }
    Ok(())
}

pub(crate) fn required_result_root(
    instruction: &TextInstruction,
    body: &crate::FunctionBody,
    source: &str,
    item: TextItem,
) -> Result<Option<Type>, TextParseError> {
    let op = parse(instruction, source, item)?;
    if op.opcode == 0x48c {
        return Ok(Some(result_type(op.opcode, &Type::Integer(32))));
    }
    let component = body
        .blocks
        .iter()
        .flat_map(|block| &block.parameters)
        .find(|parameter| parameter.name == op.operands[0])
        .and_then(|parameter| match &parameter.ty {
            Type::Texture { component, .. } => Some(component.as_ref().clone()),
            _ => None,
        });
    Ok(component.map(|component| result_type(op.opcode, &component)))
}

pub(crate) fn encode(
    instruction: &TextInstruction,
    source: &str,
    item: TextItem,
    values: &[&str],
    bytes: &mut Vec<u8>,
) -> Result<(), TextParseError> {
    let op = parse(instruction, source, item)?;
    let mods = (if op.unchecked { UNCHECKED } else { 0 })
        | (if op.residency.is_some() { SPARSE } else { 0 })
        | (op.selector << 48);
    for word in [
        op.opcode,
        mods,
        1 + u64::from(op.residency.is_some()),
        op.operands.len() as u64,
    ] {
        crate::leb::write_uleb(word, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    for operand in &op.operands {
        let position = values
            .iter()
            .position(|value| *value == operand)
            .ok_or_else(|| {
                error(
                    item,
                    instruction.span,
                    "texture sampling operand is undefined",
                )
            })?;
        crate::leb::write_uleb((values.len() - position) as u64, bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    crate::leb::write_uleb(op.offsets.len() as u64, bytes)
        .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    for offset in op.offsets {
        crate::leb::write_integer_immediate(offset < 0, u128::from(offset.unsigned_abs()), bytes)
            .map_err(|issue| error(item, instruction.span, issue.to_string()))?;
    }
    Ok(())
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
