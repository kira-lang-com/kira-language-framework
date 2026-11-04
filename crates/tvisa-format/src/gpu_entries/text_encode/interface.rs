use crate::binary::FormatError;
use crate::text::TextGpuEntryValue;

use super::super::text_values as value;
use super::super::{GpuRootRecord, GpuStageField};

const STAGE_FIELDS: &[&str] = &["semantic", "location", "type", "interpolation", "offset"];
const INTERPOLATIONS: &[&str] = &[
    "flat",
    "perspective",
    "noperspective",
    "centroid",
    "sample",
    "invariant",
    "per_primitive",
];

pub(super) fn triple(value: &TextGpuEntryValue, label: &str) -> Result<[u64; 3], FormatError> {
    let values = value::array(value, label)?;
    if values.len() != 3 {
        return Err(value::invalid(
            value.span,
            format!("{label} must have three elements"),
        ));
    }
    Ok([
        value::number(&values[0], label)?,
        value::number(&values[1], label)?,
        value::number(&values[2], label)?,
    ])
}

pub(super) fn optional_root(
    value: &TextGpuEntryValue,
) -> Result<Option<GpuRootRecord>, FormatError> {
    if value::is_none(value) {
        return Ok(None);
    }
    let fields = value::object(value, "root")?;
    value::fields(fields, &["size", "align_log2", "typeid", "space"], "root")?;
    let size = value::number(value::field(fields, "size")?, "root size")?;
    if size == 0 {
        return Err(value::invalid(
            value.span,
            "a present root must have nonzero size",
        ));
    }
    let alignment = value::number(value::field(fields, "align_log2")?, "root align_log2")?;
    let alignment_log2 = u8::try_from(alignment)
        .ok()
        .filter(|alignment| *alignment <= 63)
        .ok_or_else(|| value::invalid(value.span, "root align_log2 must be at most 63"))?;
    let address_space = match value::scalar(value::field(fields, "space")?, "root space")? {
        "device" => 1,
        "constant" => 2,
        _ => {
            return Err(value::invalid(
                value.span,
                "root space must be device or constant",
            ));
        }
    };
    Ok(Some(GpuRootRecord {
        size,
        alignment_log2,
        type_id: value::hexadecimal(value::field(fields, "typeid")?, "root typeid")?,
        address_space,
    }))
}

pub(super) fn optional_immediate(value: &TextGpuEntryValue) -> Result<(u64, u64), FormatError> {
    if value::is_none(value) {
        return Ok((0, 0));
    }
    let fields = value::object(value, "immediate")?;
    value::fields(fields, &["size", "typeid"], "immediate")?;
    let size = value::number(value::field(fields, "size")?, "immediate size")?;
    if size == 0 {
        return Err(value::invalid(
            value.span,
            "a present immediate must have nonzero size",
        ));
    }
    Ok((
        size,
        value::hexadecimal(value::field(fields, "typeid")?, "immediate typeid")?,
    ))
}

pub(super) fn stage_fields(
    value: &TextGpuEntryValue,
    value_type_count: u64,
    resolve_type: &mut impl FnMut(&tir_types::Type) -> Option<u64>,
) -> Result<Vec<GpuStageField>, FormatError> {
    let fields = value::array(value, "stage fields")?;
    fields
        .iter()
        .map(|field| {
            let object = value::object(field, "stage field")?;
            value::fields(object, STAGE_FIELDS, "stage field")?;
            let semantic_value = value::field(object, "semantic")?;
            let semantic_text = value::scalar(semantic_value, "stage semantic")?;
            let (semantic, semantic_index) = semantic(semantic_text)
                .ok_or_else(|| value::invalid(semantic_value.span, "invalid stage semantic"))?;
            let location = value::number(value::field(object, "location")?, "stage location")?;
            let ty = value::type_value(value::field(object, "type")?, "stage field type")?;
            let type_index = resolve_type(&ty)
                .filter(|index| *index < value_type_count)
                .ok_or_else(|| {
                    value::invalid(field.span, "unresolved or out-of-range stage type")
                })?;
            let interpolation = interpolation(value::field(object, "interpolation")?)?;
            let byte_offset = value::number(value::field(object, "offset")?, "stage field offset")?;
            Ok(GpuStageField {
                semantic,
                semantic_index,
                location,
                type_index,
                interpolation,
                byte_offset,
            })
        })
        .collect()
}

pub(super) fn semantic(text: &str) -> Option<(u8, u64)> {
    let text: String = text.split_whitespace().collect();
    let simple = match text.as_str() {
        "user" => Some(0),
        "position" => Some(1),
        "depth(any)" => Some(4),
        "depth(greater)" => Some(5),
        "depth(less)" => Some(6),
        "stencil" => Some(7),
        "sample_mask" => Some(8),
        "point_size" => Some(11),
        "layer" => Some(12),
        "viewport" => Some(13),
        "primitive_id" => Some(14),
        "view_index" => Some(15),
        "cull" => Some(16),
        "shading_rate" => Some(17),
        _ => None,
    };
    if let Some(code) = simple {
        return Some((code, 0));
    }
    let (name, index) = text.split_once('(')?;
    let index = index.strip_suffix(')')?;
    let index = tir_format::numeric_literals::parse_unsigned_integer(index)?;
    let index = u64::try_from(index).ok()?;
    let code = match name {
        "color" => 2,
        "color_index1" => 3,
        "clip_distance" => 9,
        "cull_distance" => 10,
        _ => return None,
    };
    Some((code, index))
}

pub(super) fn interpolation(value: &TextGpuEntryValue) -> Result<u8, FormatError> {
    let names = value::array(value, "interpolation")?;
    let mut flags = 0;
    for name in names {
        let name_text = value::scalar(name, "interpolation name")?;
        let bit = INTERPOLATIONS
            .iter()
            .position(|candidate| *candidate == name_text)
            .ok_or_else(|| value::invalid(name.span, "unknown interpolation name"))?;
        let mask = 1u8 << bit;
        if flags & mask != 0 {
            return Err(value::invalid(name.span, "duplicate interpolation name"));
        }
        flags |= mask;
    }
    Ok(flags)
}
