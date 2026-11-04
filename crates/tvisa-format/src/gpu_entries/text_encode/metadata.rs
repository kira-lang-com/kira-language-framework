use std::collections::HashSet;

use crate::binary::FormatError;
use crate::text::{TextGpuEntryValue, TextImage};

use super::super::text_values as value;
use super::super::{
    GPU_CAPABILITY_NAMES, GpuEntryKind, GpuNodeOutput, GpuNodeOutputKind, GpuNodeRecord,
    GpuSpecializationConstant,
};
use super::interface::triple;

pub(super) fn payload(value: &TextGpuEntryValue) -> Result<[u64; 4], FormatError> {
    if value::is_none(value) {
        return Ok([0; 4]);
    }
    let fields = value::object(value, "payload")?;
    value::fields(
        fields,
        &["object", "ray", "hit_attribute", "callable"],
        "payload",
    )?;
    let sizes = [
        value::number(value::field(fields, "object")?, "object payload size")?,
        value::number(value::field(fields, "ray")?, "ray payload size")?,
        value::number(value::field(fields, "hit_attribute")?, "hit attribute size")?,
        value::number(value::field(fields, "callable")?, "callable data size")?,
    ];
    if sizes == [0; 4] {
        return Err(value::invalid(
            value.span,
            "empty payload must be written as none",
        ));
    }
    Ok(sizes)
}

pub(super) fn mesh(
    kind: GpuEntryKind,
    value: &TextGpuEntryValue,
) -> Result<([u64; 2], u8), FormatError> {
    if value::is_none(value) {
        return Ok(([0; 2], 0));
    }
    let fields = value::object(value, "mesh")?;
    value::fields(
        fields,
        &["max_vertices", "max_primitives", "topology"],
        "mesh",
    )?;
    let maximum = [
        value::number(value::field(fields, "max_vertices")?, "mesh max_vertices")?,
        value::number(
            value::field(fields, "max_primitives")?,
            "mesh max_primitives",
        )?,
    ];
    let topology = match value::scalar(value::field(fields, "topology")?, "mesh topology")? {
        "points" => 0,
        "lines" => 1,
        "triangles" => 2,
        _ => return Err(value::invalid(value.span, "invalid mesh topology")),
    };
    if kind != GpuEntryKind::Mesh {
        return Err(value::invalid(
            value.span,
            "mesh metadata is only valid for mesh entries",
        ));
    }
    Ok((maximum, topology))
}

pub(super) fn capabilities(value: &TextGpuEntryValue) -> Result<u64, FormatError> {
    let names = value::array(value, "capabilities")?;
    let mut mask = 0;
    let mut seen = HashSet::new();
    let mut previous = None;
    for name in names {
        let name = value::scalar(name, "capability name")?;
        let bit = GPU_CAPABILITY_NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .ok_or_else(|| {
                value::invalid(value.span, format!("unknown GPU capability '{name}'"))
            })?;
        if !seen.insert(name) {
            return Err(value::invalid(
                value.span,
                format!("duplicate GPU capability '{name}'"),
            ));
        }
        if previous.is_some_and(|previous| previous > bit) {
            return Err(value::invalid(
                value.span,
                "capabilities are not in canonical bit order",
            ));
        }
        previous = Some(bit);
        mask |= 1u64 << bit;
    }
    Ok(mask)
}

pub(super) fn subgroup_size(value: &TextGpuEntryValue) -> Result<u8, FormatError> {
    if value::scalar(value, "subgroup_size")? == "any" {
        return Ok(0);
    }
    let size = value::number(value, "subgroup_size")?;
    if !(4..=128).contains(&size) || !size.is_power_of_two() {
        return Err(value::invalid(
            value.span,
            "subgroup_size must be any or a power of two from 4 through 128",
        ));
    }
    Ok(size.trailing_zeros() as u8)
}

pub(super) fn floating_point(value: &TextGpuEntryValue) -> Result<u8, FormatError> {
    let modes = value::array(value, "floating_point")?;
    let mut flags = 0;
    for mode in modes {
        let name = value::scalar(mode, "floating-point mode")?;
        let bit = match name {
            "fast" => 0,
            "ieee_denormals" => 1,
            _ => return Err(value::invalid(mode.span, "unknown floating-point mode")),
        };
        let mask = 1u8 << bit;
        if flags & mask != 0 {
            return Err(value::invalid(mode.span, "duplicate floating-point mode"));
        }
        flags |= mask;
    }
    Ok(flags)
}

pub(super) fn specialization_constants(
    image: &TextImage,
    value: &TextGpuEntryValue,
    value_type_count: u64,
    constant_count: u64,
    resolve_type: &mut impl FnMut(&tir_types::Type) -> Option<u64>,
    resolve_constant: &mut impl FnMut(&tir_types::Type, &str) -> Option<u64>,
) -> Result<Vec<GpuSpecializationConstant>, FormatError> {
    let entries = value::array(value, "specialization_constants")?;
    let mut ids = HashSet::new();
    entries
        .iter()
        .map(|entry| {
            let fields = value::object(entry, "specialization constant")?;
            value::fields(
                fields,
                &["id", "type", "default"],
                "specialization constant",
            )?;
            let id_value = value::field(fields, "id")?;
            let id = value::number(id_value, "specialization constant id")?;
            let id = u32::try_from(id).map_err(|_| {
                value::invalid(id_value.span, "specialization constant id exceeds 32 bits")
            })?;
            if !ids.insert(id) {
                return Err(value::invalid(
                    id_value.span,
                    "duplicate specialization constant id",
                ));
            }
            let type_value = value::field(fields, "type")?;
            let ty = value::type_value(type_value, "specialization constant type")?;
            let type_index = resolve_type(&ty)
                .filter(|index| *index < value_type_count)
                .ok_or_else(|| {
                    value::invalid(
                        type_value.span,
                        "unresolved or out-of-range specialization type",
                    )
                })?;
            let default_value = value::field(fields, "default")?;
            let default_text =
                value::canonical_literal(image.source(), default_value, "specialization default")?;
            let default_constant = resolve_constant(&ty, &default_text)
                .filter(|index| *index < constant_count)
                .ok_or_else(|| {
                    value::invalid(
                        default_value.span,
                        "unresolved or out-of-range specialization default",
                    )
                })?;
            Ok(GpuSpecializationConstant {
                id,
                type_index,
                default_constant,
            })
        })
        .collect()
}

pub(super) fn node(
    kind: GpuEntryKind,
    value: &TextGpuEntryValue,
) -> Result<Option<GpuNodeRecord>, FormatError> {
    if value::is_none(value) {
        if kind == GpuEntryKind::Node {
            return Err(value::invalid(
                value.span,
                "node entries require node metadata",
            ));
        }
        return Ok(None);
    }
    if kind != GpuEntryKind::Node {
        return Err(value::invalid(
            value.span,
            "node metadata is only valid for node entries",
        ));
    }
    let fields = value::object(value, "node")?;
    value::fields(
        fields,
        &[
            "launch",
            "max_dispatch_grid",
            "input_record_size",
            "outputs",
        ],
        "node",
    )?;
    let launch_mode = match value::scalar(value::field(fields, "launch")?, "node launch")? {
        "broadcast" => 0,
        "coalescing" => 1,
        "thread" => 2,
        _ => return Err(value::invalid(value.span, "invalid node launch mode")),
    };
    let maximum_dispatch_grid = triple(
        value::field(fields, "max_dispatch_grid")?,
        "max_dispatch_grid",
    )?;
    let input_record_size = value::number(
        value::field(fields, "input_record_size")?,
        "input_record_size",
    )?;
    let output_values = value::array(value::field(fields, "outputs")?, "node outputs")?;
    let outputs = output_values
        .iter()
        .map(|output| {
            let fields = value::object(output, "node output")?;
            value::fields(
                fields,
                &["name", "kind", "record_size", "max_records"],
                "node output",
            )?;
            let name = value::string(value::field(fields, "name")?, "node output name")?;
            let kind = match value::scalar(value::field(fields, "kind")?, "node output kind")? {
                "node" => GpuNodeOutputKind::Node,
                "mesh" => GpuNodeOutputKind::Mesh,
                _ => {
                    return Err(value::invalid(
                        output.span,
                        "node output kind must be node or mesh",
                    ));
                }
            };
            Ok(GpuNodeOutput {
                name,
                kind,
                record_size: value::number(
                    value::field(fields, "record_size")?,
                    "node output record_size",
                )?,
                maximum_records: value::number(
                    value::field(fields, "max_records")?,
                    "node output max_records",
                )?,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(Some(GpuNodeRecord {
        launch_mode,
        maximum_dispatch_grid,
        input_record_size,
        outputs,
    }))
}

pub(super) fn ray_tracing(
    kind: GpuEntryKind,
    value: &TextGpuEntryValue,
) -> Result<Option<u8>, FormatError> {
    if value::is_none(value) {
        if kind.is_ray_tracing() {
            return Err(value::invalid(
                value.span,
                "ray-tracing entries require ray_tracing metadata",
            ));
        }
        return Ok(None);
    }
    if !kind.is_ray_tracing() {
        return Err(value::invalid(
            value.span,
            "ray_tracing metadata is only valid for ray-tracing entries",
        ));
    }
    let fields = value::object(value, "ray_tracing")?;
    value::fields(fields, &["max_recursion_depth"], "ray_tracing")?;
    let depth = value::number(
        value::field(fields, "max_recursion_depth")?,
        "max_recursion_depth",
    )?;
    Ok(Some(u8::try_from(depth).map_err(|_| {
        value::invalid(value.span, "ray recursion depth exceeds 255")
    })?))
}

pub(super) fn fragment(
    kind: GpuEntryKind,
    value: &TextGpuEntryValue,
) -> Result<Option<u8>, FormatError> {
    if value::is_none(value) {
        if kind == GpuEntryKind::Fragment {
            return Err(value::invalid(
                value.span,
                "fragment entries require fragment metadata",
            ));
        }
        return Ok(None);
    }
    if kind != GpuEntryKind::Fragment {
        return Err(value::invalid(
            value.span,
            "fragment metadata is only valid for fragment entries",
        ));
    }
    let fields = value::object(value, "fragment")?;
    value::fields(
        fields,
        &[
            "early_fragment_tests",
            "writes_depth",
            "discards_or_demotes",
            "reads_attachments",
        ],
        "fragment",
    )?;
    let mut flags = 0;
    if boolean(
        value::field(fields, "early_fragment_tests")?,
        "early_fragment_tests",
    )? {
        flags |= 1;
    }
    let depth_mode = match value::scalar(value::field(fields, "writes_depth")?, "writes_depth")? {
        "none" => 0,
        "any" => 1,
        "greater" => 2,
        "less" => 3,
        _ => {
            return Err(value::invalid(
                value.span,
                "writes_depth must be none, any, greater, or less",
            ));
        }
    };
    if depth_mode != 0 {
        flags |= 2 | (depth_mode << 2);
    }
    if boolean(
        value::field(fields, "discards_or_demotes")?,
        "discards_or_demotes",
    )? {
        flags |= 1 << 4;
    }
    if boolean(
        value::field(fields, "reads_attachments")?,
        "reads_attachments",
    )? {
        flags |= 1 << 5;
    }
    Ok(Some(flags))
}

fn boolean(value: &TextGpuEntryValue, label: &str) -> Result<bool, FormatError> {
    match value::scalar(value, label)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(value::invalid(
            value.span,
            format!("{label} must be true or false"),
        )),
    }
}
