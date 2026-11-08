//! Lossless text rendering of supported sampled textures and implicit LOD.
use std::fmt::Write as _;

use tir_types::{FloatType, TextureDimension, Type};

use crate::FormatError;
use crate::text_gpu_texture_sample as sample;
use crate::types::{BinaryInstruction, BinaryOperand, BinaryType, BinaryTypeTable};

pub(crate) fn format(
    text: &mut String,
    instruction: &BinaryInstruction,
    types: &BinaryTypeTable,
    values: &mut Vec<(String, u64)>,
) -> Result<(), FormatError> {
    let name = match instruction.opcode {
        0x480 => "tex.sample",
        0x481 => "tex.sample_level",
        0x482 => "tex.sample_grad",
        0x483 => "tex.sample_bias",
        0x484 => "tex.sample_compare",
        0x485 => "tex.sample_compare_level",
        0x486 => "tex.gather",
        0x487 => "tex.gather_compare",
        0x48c => "tex.lod",
        0x48d => "tex.feedback",
        _ => {
            return Err(malformed(
                "texture sample formatter received another opcode",
            ));
        }
    };
    let allowed = if matches!(instruction.opcode, 0x481 | 0x482 | 0x485) {
        0
    } else if instruction.opcode == 0x486 {
        sample::UNCHECKED | (0xf << 48)
    } else {
        sample::UNCHECKED
    };
    let sparse = instruction.modifiers & sample::SPARSE != 0;
    let allowed = allowed
        | if instruction.opcode == 0x48c {
            0
        } else {
            sample::SPARSE
        };
    if instruction.result_count != 1 + u64::from(sparse) || instruction.modifiers & !allowed != 0 {
        return Err(malformed(
            "texture sample/LOD result count or modifiers are invalid",
        ));
    }
    let selector = (instruction.modifiers >> 48) & 0xf;
    if instruction.opcode == 0x486 && selector > 3 {
        return Err(malformed("texture gather component selector must be 0–3"));
    }
    let [
        BinaryOperand::Values(indices),
        BinaryOperand::Integers(offsets),
    ] = instruction.operands.as_slice()
    else {
        return Err(malformed(
            "texture CODE requires counted SSA values and constant offsets",
        ));
    };
    let operands = indices
        .iter()
        .map(|index| {
            usize::try_from(*index)
                .ok()
                .and_then(|index| values.get(index))
                .map(|(name, ty)| (name.as_str(), *ty))
                .ok_or_else(|| malformed("texture CODE refers to undefined SSA value"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if operands.len() < 3 {
        return Err(malformed(
            "texture sample/LOD is missing texture, sampler or coordinates",
        ));
    }
    let Some(BinaryType::Texture {
        dimension,
        component,
        access,
        multisampled,
    }) = types.entries.get(operands[0].1 as usize)
    else {
        return Err(malformed(
            "texture sample/LOD first operand must be texture",
        ));
    };
    if !matches!(*access, 0 | 3) || *multisampled {
        return Err(malformed(
            "texture sample/LOD requires sampleable non-multisampled texture",
        ));
    }
    if !matches!(
        types.entries.get(operands[1].1 as usize),
        Some(BinaryType::Sampler)
    ) {
        return Err(malformed("texture sample/LOD requires a sampler handle"));
    }
    let dim =
        dimension_from_binary(*dimension).ok_or_else(|| malformed("invalid texture dimension"))?;
    if !valid_coordinate(types, operands[2].1, sample::coordinate_lanes(dim)) {
        return Err(malformed(
            "texture sample/LOD has invalid normalized coordinates",
        ));
    }
    let mut required = 3;
    if sample::is_array(dim) {
        if !operands.get(3).is_some_and(|(_, ty)| {
            matches!(
                types.entries.get(*ty as usize),
                Some(BinaryType::Integer(1..=64))
            )
        }) {
            return Err(malformed("array texture sample/LOD requires integer layer"));
        }
        required += 1;
    }
    let base_count = required;
    if matches!(instruction.opcode, 0x481 | 0x483) {
        if !matches!(
            types.entries.get(
                operands
                    .get(required)
                    .ok_or_else(|| malformed("texture LOD/bias operand is missing"))?
                    .1 as usize
            ),
            Some(BinaryType::Float(FloatType::F32))
        ) {
            return Err(malformed("texture LOD/bias operand must be f32"));
        }
        required += 1;
    }
    if matches!(instruction.opcode, 0x484 | 0x485 | 0x487) {
        if !matches!(
            types.entries.get(*component as usize),
            Some(BinaryType::Float(FloatType::F32))
        ) || !matches!(
            operands
                .get(required)
                .and_then(|(_, ty)| types.entries.get(*ty as usize)),
            Some(BinaryType::Float(FloatType::F32))
        ) {
            return Err(malformed(
                "comparison sampling requires f32 component and reference",
            ));
        }
        required += 1;
        if instruction.opcode == 0x485 {
            if !matches!(
                operands
                    .get(required)
                    .and_then(|(_, ty)| types.entries.get(*ty as usize)),
                Some(BinaryType::Float(FloatType::F32))
            ) {
                return Err(malformed("comparison LOD must be f32"));
            }
            required += 1;
        }
    }
    if instruction.opcode == 0x482 {
        let gradient_ty = if sample::coordinate_lanes(dim) == 1 {
            Type::Float(FloatType::F32)
        } else {
            Type::FixedVector {
                lanes: sample::coordinate_lanes(dim) as u32,
                element: Box::new(Type::Float(FloatType::F32)),
            }
        };
        let gradient_index = crate::text_descriptor_reference::type_index(types, &gradient_ty)
            .ok_or_else(|| malformed("texture f32 gradient TYPES entry is absent"))?
            as u64;
        if operands
            .get(required)
            .is_none_or(|(_, ty)| *ty != gradient_index)
            || operands
                .get(required + 1)
                .is_none_or(|(_, ty)| *ty != gradient_index)
        {
            return Err(malformed(
                "texture gradient operand types must match f32 coordinates",
            ));
        }
        required += 2;
    }
    let min_lod = if instruction.opcode != 0x48c && operands.len() == required + 1 {
        if !matches!(
            types.entries.get(operands[required].1 as usize),
            Some(BinaryType::Float(FloatType::F32))
        ) {
            return Err(malformed("tex.sample min_lod requires f32"));
        }
        true
    } else {
        false
    };
    if operands.len() != required + usize::from(min_lod) {
        return Err(malformed("texture sample/LOD has unexpected SSA operands"));
    }
    if instruction.opcode == 0x48c && !offsets.is_empty() {
        return Err(malformed("tex.lod cannot carry texture coordinate offsets"));
    }
    if !offsets.is_empty() && offsets.len() != sample::offset_lanes(dim) {
        return Err(malformed(
            "texture offset count differs from the spatial dimension",
        ));
    }
    let result_ty = match instruction.opcode {
        0x48c => {
            let ty = sample::result_type(0x48c, &Type::Integer(32));
            crate::text_descriptor_reference::type_index(types, &ty)
                .ok_or_else(|| malformed("tex.lod result TYPES entry is absent"))?
                as u64
        }
        0x486 | 0x487 => {
            let element = if instruction.opcode == 0x487 {
                types
                    .entries
                    .iter()
                    .position(|ty| matches!(ty, BinaryType::Float(FloatType::F32)))
                    .ok_or_else(|| malformed("gather comparison f32 TYPES entry is absent"))?
                    as u64
            } else {
                match types.entries.get(*component as usize) {
                    Some(BinaryType::FixedVector { element, .. }) => *element,
                    Some(_) => *component,
                    None => return Err(malformed("gather component TYPES is invalid")),
                }
            };
            types
                .entries
                .iter()
                .position(|ty| {
                    matches!(ty,
                BinaryType::FixedVector { lanes: 4, element: found } if *found == element)
                })
                .ok_or_else(|| malformed("vec<4 x T> gather result TYPES entry is absent"))?
                as u64
        }
        _ => *component,
    };
    let result_name = format!("v{}", values.len());
    let residency_name = sparse.then(|| format!("v{}", values.len() + 1));
    write!(text, "    %{result_name}").map_err(|issue| malformed(issue.to_string()))?;
    if let Some(residency) = &residency_name {
        write!(text, ", %{residency}").map_err(|issue| malformed(issue.to_string()))?;
    }
    write!(text, " = {name}").map_err(|issue| malformed(issue.to_string()))?;
    for (position, (name, _)) in operands.iter().take(base_count).enumerate() {
        write!(text, "{}%{name}", if position == 0 { " " } else { ", " })
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if matches!(instruction.opcode, 0x481 | 0x483) {
        let key = if instruction.opcode == 0x481 {
            "lod"
        } else {
            "bias"
        };
        write!(text, ", {key} %{}", operands[base_count].0)
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if instruction.opcode == 0x482 {
        write!(
            text,
            " grad %{}, %{}",
            operands[base_count].0,
            operands[base_count + 1].0
        )
        .map_err(|issue| malformed(issue.to_string()))?;
    }
    if matches!(instruction.opcode, 0x484 | 0x485) {
        write!(text, " compare %{}", operands[base_count].0)
            .map_err(|issue| malformed(issue.to_string()))?;
        if instruction.opcode == 0x485 {
            write!(text, ", lod %{}", operands[base_count + 1].0)
                .map_err(|issue| malformed(issue.to_string()))?;
        }
    }
    if instruction.opcode == 0x486 && selector != 0 {
        write!(text, " component {selector}").map_err(|issue| malformed(issue.to_string()))?;
    }
    if instruction.opcode == 0x487 {
        write!(text, ", %{}", operands[base_count].0)
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if !offsets.is_empty() {
        text.push_str(" offset [");
        for (position, offset) in offsets.iter().enumerate() {
            if position > 0 {
                text.push_str(", ");
            }
            if (offset.negative && (offset.magnitude == 0 || offset.magnitude > 8))
                || (!offset.negative && offset.magnitude > 7)
            {
                return Err(malformed("texture offset component outside -8..=7"));
            }
            if offset.negative {
                text.push('-');
            }
            write!(text, "{}", offset.magnitude).map_err(|issue| malformed(issue.to_string()))?;
        }
        text.push(']');
    }
    if min_lod {
        write!(text, " min_lod %{}", operands[required].0)
            .map_err(|issue| malformed(issue.to_string()))?;
    }
    if instruction.modifiers & sample::UNCHECKED != 0 {
        text.push_str(" [unchecked_uniformity]");
    }
    if sparse {
        text.push_str(" [sparse]");
    }
    text.push('\n');
    values.push((result_name, result_ty));
    if let Some(name) = residency_name {
        let ty = crate::text_descriptor_reference::type_index(types, &Type::Integer(1))
            .ok_or_else(|| malformed("sparse texture residency i1 TYPES entry is absent"))?;
        values.push((name, ty as u64));
    }
    Ok(())
}

fn valid_coordinate(types: &BinaryTypeTable, ty: u64, lanes: usize) -> bool {
    let floating = |ty: &BinaryType| {
        matches!(
            ty,
            BinaryType::Float(FloatType::F16 | FloatType::F32 | FloatType::F64)
        )
    };
    match types.entries.get(ty as usize) {
        Some(ty) if lanes == 1 && floating(ty) => true,
        Some(BinaryType::FixedVector {
            lanes: count,
            element,
        }) if *count as usize == lanes => {
            types.entries.get(*element as usize).is_some_and(floating)
        }
        _ => false,
    }
}
fn dimension_from_binary(dimension: u8) -> Option<TextureDimension> {
    Some(match dimension {
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
