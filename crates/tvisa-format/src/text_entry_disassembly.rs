use tir_diag::Span;

use crate::gpu_entries::{GpuEntryKind, GpuEntryRecord};
use crate::text::{TextParseError, error};
use crate::text_entry_validation::CAPABILITIES;

/// Resolves image table indexes while formatting GPU entry records as TVISA text.
pub trait TextGpuEntryFormatter {
    fn function_symbol(&self, function_index: u64, kind: GpuEntryKind) -> Option<String>;
    fn value_type_text(&self, type_index: u64) -> Option<String>;
    fn constant_text(&self, type_index: u64, constant_index: u64) -> Option<String>;
}

/// Formats binary GPU entry records using the canonical field order.
pub fn format_gpu_entry_records(
    records: &[GpuEntryRecord],
    formatter: &impl TextGpuEntryFormatter,
) -> Result<String, TextParseError> {
    records
        .iter()
        .map(|record| format_gpu_entry_record(record, formatter))
        .collect::<Result<Vec<_>, _>>()
        .map(|entries| entries.join("\n"))
}

fn format_gpu_entry_record(
    record: &GpuEntryRecord,
    formatter: &impl TextGpuEntryFormatter,
) -> Result<String, TextParseError> {
    let symbol = formatter
        .function_symbol(record.function_index, record.kind)
        .ok_or_else(|| failure("GPU entry function index has no symbol"))?;
    if symbol.is_empty() || symbol.starts_with('@') || symbol.chars().any(char::is_whitespace) {
        return Err(failure("GPU entry function symbol is invalid"));
    }
    let mut output = format!("entry {} @{symbol} record {{\n", kind_name(record.kind));
    if record.kind == GpuEntryKind::Visible {
        let type_id = record
            .visible_type_id
            .ok_or_else(|| failure("visible GPU entry has no type id"))?;
        field(&mut output, "typeid", format!("0x{type_id:016x}"));
    } else {
        field(&mut output, "threads", triple(record.threads));
        field(&mut output, "root", format_root(record.root.as_ref())?);
        field(
            &mut output,
            "immediate",
            format_immediate(record.immediate_size, record.immediate_type_id),
        );
        field(
            &mut output,
            "stage_input",
            format_stage_fields(&record.stage_input, formatter)?,
        );
        field(
            &mut output,
            "stage_output",
            format_stage_fields(&record.stage_output, formatter)?,
        );
        field(&mut output, "payload", format_payload(record.payload_sizes));
        field(
            &mut output,
            "mesh",
            format_mesh(record.mesh_maximum, record.mesh_topology)?,
        );
        field(
            &mut output,
            "threadgroup_memory",
            record.threadgroup_memory.to_string(),
        );
        field(
            &mut output,
            "capabilities",
            format_capabilities(record.capabilities)?,
        );
        field(
            &mut output,
            "subgroup_size",
            format_subgroup(record.subgroup_size_log2)?,
        );
        field(
            &mut output,
            "floating_point",
            format_floating_point(record.floating_point_flags)?,
        );
        field(
            &mut output,
            "specialization_constants",
            format_specialization_constants(&record.specialization_constants, formatter)?,
        );
        field(&mut output, "node", format_node(record.node.as_ref())?);
        field(
            &mut output,
            "ray_tracing",
            format_ray_tracing(record.ray_recursion_depth),
        );
        field(
            &mut output,
            "fragment",
            format_fragment(record.fragment_flags)?,
        );
    }
    output.push_str("}\n");
    Ok(output)
}

fn format_root(root: Option<&crate::GpuRootRecord>) -> Result<String, TextParseError> {
    let Some(root) = root else {
        return Ok("none".to_owned());
    };
    let space = match root.address_space {
        1 => "device",
        2 => "constant",
        _ => return Err(failure("unknown GPU root address space")),
    };
    Ok(format!(
        "{{ size: {}, align_log2: {}, typeid: 0x{:016x}, space: {space} }}",
        root.size, root.alignment_log2, root.type_id
    ))
}

fn format_immediate(size: u64, type_id: u64) -> String {
    if size == 0 {
        "none".to_owned()
    } else {
        format!("{{ size: {size}, typeid: 0x{type_id:016x} }}")
    }
}

fn format_stage_fields(
    fields: &[crate::GpuStageField],
    formatter: &impl TextGpuEntryFormatter,
) -> Result<String, TextParseError> {
    let values = fields
        .iter()
        .map(|field| {
            let semantic = format_semantic(field.semantic, field.location)?;
            let ty = formatter
                .value_type_text(field.type_index)
                .ok_or_else(|| failure("GPU stage type index has no textual type"))?;
            let interpolation = format_interpolation(field.interpolation)?;
            Ok(format!(
                "{{ semantic: {semantic}, location: {}, type: {ty}, interpolation: [{interpolation}], offset: {} }}",
                field.location, field.byte_offset
            ))
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    Ok(format!("[{}]", values.join(", ")))
}

fn format_semantic(semantic: u8, location: u64) -> Result<String, TextParseError> {
    Ok(match semantic {
        0 => "user".to_owned(),
        1 => "position".to_owned(),
        2 => format!("color({location})"),
        3 => format!("color_index1({location})"),
        4 => "depth(any)".to_owned(),
        5 => "depth(greater)".to_owned(),
        6 => "depth(less)".to_owned(),
        7 => "stencil".to_owned(),
        8 => "sample_mask".to_owned(),
        9 => format!("clip_distance({location})"),
        10 => format!("cull_distance({location})"),
        11 => "point_size".to_owned(),
        12 => "layer".to_owned(),
        13 => "viewport".to_owned(),
        14 => "primitive_id".to_owned(),
        15 => "view_index".to_owned(),
        16 => "cull".to_owned(),
        17 => "shading_rate".to_owned(),
        _ => return Err(failure("unknown GPU stage semantic code")),
    })
}

fn format_interpolation(interpolation: u8) -> Result<&'static str, TextParseError> {
    [
        "flat",
        "perspective",
        "noperspective",
        "centroid",
        "sample",
        "invariant",
        "per_primitive",
    ]
    .get(usize::from(interpolation))
    .copied()
    .ok_or_else(|| failure("unknown GPU interpolation code"))
}

fn format_payload(sizes: [u64; 4]) -> String {
    if sizes == [0; 4] {
        "none".to_owned()
    } else {
        format!(
            "{{ object: {}, ray: {}, hit_attribute: {}, callable: {} }}",
            sizes[0], sizes[1], sizes[2], sizes[3]
        )
    }
}

fn format_mesh(maximum: [u64; 2], topology: u8) -> Result<String, TextParseError> {
    if maximum == [0; 2] && topology == 0 {
        return Ok("none".to_owned());
    }
    let topology = match topology {
        0 => "points",
        1 => "lines",
        2 => "triangles",
        _ => return Err(failure("unknown GPU mesh topology")),
    };
    Ok(format!(
        "{{ max_vertices: {}, max_primitives: {}, topology: {topology} }}",
        maximum[0], maximum[1]
    ))
}

fn format_capabilities(bits: u64) -> Result<String, TextParseError> {
    if bits >> CAPABILITIES.len() != 0 {
        return Err(failure("GPU capability set has unknown bits"));
    }
    let values = CAPABILITIES
        .iter()
        .enumerate()
        .filter_map(|(index, name)| (bits & (1 << index) != 0).then_some(*name))
        .collect::<Vec<_>>();
    Ok(format!("[{}]", values.join(", ")))
}

fn format_subgroup(log2: u8) -> Result<String, TextParseError> {
    match log2 {
        0 => Ok("any".to_owned()),
        2..=7 => Ok((1_u64 << log2).to_string()),
        _ => Err(failure("invalid GPU subgroup size exponent")),
    }
}

fn format_floating_point(flags: u8) -> Result<String, TextParseError> {
    if flags & !3 != 0 {
        return Err(failure("reserved GPU floating-point flags are set"));
    }
    let mut values = Vec::new();
    if flags & 1 != 0 {
        values.push("fast");
    }
    if flags & 2 != 0 {
        values.push("ieee_denormals");
    }
    Ok(format!("[{}]", values.join(", ")))
}

fn format_specialization_constants(
    constants: &[crate::GpuSpecializationConstant],
    formatter: &impl TextGpuEntryFormatter,
) -> Result<String, TextParseError> {
    let values = constants
        .iter()
        .map(|constant| {
            let ty = formatter
                .value_type_text(constant.type_index)
                .ok_or_else(|| failure("GPU specialization type index has no textual type"))?;
            let default = formatter
                .constant_text(constant.type_index, constant.default_constant)
                .ok_or_else(|| failure("GPU specialization default has no textual literal"))?;
            Ok(format!(
                "{{ id: {}, type: {ty}, default: {default} }}",
                constant.id
            ))
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    Ok(format!("[{}]", values.join(", ")))
}

fn format_node(node: Option<&crate::GpuNodeRecord>) -> Result<String, TextParseError> {
    let Some(node) = node else {
        return Ok("none".to_owned());
    };
    let launch = match node.launch_mode {
        0 => "broadcast",
        1 => "coalescing",
        2 => "thread",
        _ => return Err(failure("unknown GPU node launch mode")),
    };
    let outputs = node
        .outputs
        .iter()
        .map(|output| {
            Ok(format!(
                "{{ name: {}, record_size: {}, max_records: {} }}",
                quote_string(&output.name),
                output.record_size,
                output.maximum_records
            ))
        })
        .collect::<Result<Vec<_>, TextParseError>>()?;
    Ok(format!(
        "{{ launch: {launch}, max_dispatch_grid: {}, input_record_size: {}, outputs: [{}] }}",
        triple(node.maximum_dispatch_grid),
        node.input_record_size,
        outputs.join(", ")
    ))
}

fn format_ray_tracing(depth: Option<u8>) -> String {
    depth.map_or_else(
        || "none".to_owned(),
        |depth| format!("{{ max_recursion_depth: {depth} }}"),
    )
}

fn format_fragment(flags: Option<u8>) -> Result<String, TextParseError> {
    let Some(flags) = flags else {
        return Ok("none".to_owned());
    };
    if flags & !0x3f != 0 {
        return Err(failure("reserved GPU fragment flags are set"));
    }
    let has_depth = flags & 2 != 0;
    let depth_mode = (flags >> 2) & 3;
    if has_depth != (depth_mode != 0) {
        return Err(failure("GPU fragment depth flags disagree"));
    }
    let writes_depth = match (has_depth, depth_mode) {
        (false, 0) => "none",
        (true, 1) => "any",
        (true, 2) => "greater",
        (true, 3) => "less",
        _ => return Err(failure("invalid GPU fragment depth mode")),
    };
    Ok(format!(
        "{{ early_fragment_tests: {}, writes_depth: {writes_depth}, discards_or_demotes: {}, reads_attachments: {} }}",
        bool_text(flags & 1 != 0),
        bool_text(flags & 0x10 != 0),
        bool_text(flags & 0x20 != 0)
    ))
}

fn field(output: &mut String, name: &str, value: String) {
    output.push_str("  ");
    output.push_str(name);
    output.push_str(": ");
    output.push_str(&value);
    output.push('\n');
}

fn triple(values: [u64; 3]) -> String {
    format!("[{}, {}, {}]", values[0], values[1], values[2])
}

fn kind_name(kind: GpuEntryKind) -> &'static str {
    match kind {
        GpuEntryKind::Compute => "compute",
        GpuEntryKind::Object => "object",
        GpuEntryKind::Mesh => "mesh",
        GpuEntryKind::Vertex => "vertex",
        GpuEntryKind::Fragment => "fragment",
        GpuEntryKind::Tile => "tile",
        GpuEntryKind::Raygen => "raygen",
        GpuEntryKind::Intersection => "intersection",
        GpuEntryKind::Anyhit => "anyhit",
        GpuEntryKind::Closesthit => "closesthit",
        GpuEntryKind::Miss => "miss",
        GpuEntryKind::Callable => "callable",
        GpuEntryKind::Node => "node",
        GpuEntryKind::Visible => "visible",
    }
}

fn quote_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '\0' => output.push_str("\\0"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{1b}' => output.push_str("\\e"),
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            _ => output.push(character),
        }
    }
    output.push('"');
    output
}

fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn failure(message: &str) -> TextParseError {
    error(Span::new(0, 0), message)
}
