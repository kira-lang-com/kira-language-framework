use crate::gpu_entries::GpuEntryKind;
use crate::text::error;
use crate::text_entries::{TextEntryField, TextEntryValue, TextEntryValueKind};
use crate::text_entry_specialization_validation::validate_specialization_constants;
use crate::text_entry_value::{
    array, atom, is_atom, member, object, positive, require_atom_in, require_hex, require_type,
    string, unsigned,
};
use tir_diag::Span;

pub(super) const CAPABILITIES: &[&str] = &[
    "float64",
    "atomic128",
    "float16.atomics",
    "tile",
    "framebuffer_fetch",
    "raster_order",
    "work_graphs",
    "work_graphs.mesh",
    "coop.fp8",
    "coop.blockscaled",
    "coop.reduce",
    "compute_derivatives",
    "float32.denorm_preserve",
    "sparse_residency",
    "sampler_feedback",
    "subgroup_size_control",
    "triangle_vertex_positions",
    "shading_rate",
    "printf",
];

const SEMANTICS: &[&str] = &[
    "user",
    "position",
    "stencil",
    "sample_mask",
    "point_size",
    "layer",
    "viewport",
    "primitive_id",
    "view_index",
    "cull",
    "shading_rate",
];

pub(super) fn validate_record_fields(
    kind: GpuEntryKind,
    fields: &[TextEntryField],
    entry_span: Span,
) -> Result<(), crate::text::TextParseError> {
    let expected = if kind == GpuEntryKind::Visible {
        &["typeid"][..]
    } else {
        &[
            "threads",
            "root",
            "immediate",
            "stage_input",
            "stage_output",
            "payload",
            "mesh",
            "threadgroup_memory",
            "capabilities",
            "subgroup_size",
            "floating_point",
            "specialization_constants",
            "node",
            "ray_tracing",
            "fragment",
        ][..]
    };
    check_member_names(fields, expected, entry_span, "GPU entry")?;
    for field in fields {
        match field.name.as_str() {
            "typeid" => require_hex(&field.value, "typeid", 64)?,
            "threads" => validate_threads(kind, field)?,
            "root" => validate_root(field)?,
            "immediate" => validate_immediate(field)?,
            "stage_input" | "stage_output" => validate_stage_fields(kind, field)?,
            "payload" => validate_payload(field)?,
            "mesh" => validate_mesh(kind, field)?,
            "threadgroup_memory" => {
                unsigned(&field.value, "threadgroup_memory", 64)?;
            }
            "capabilities" => validate_capabilities(field)?,
            "subgroup_size" => validate_subgroup(field)?,
            "floating_point" => validate_floating_point(field)?,
            "specialization_constants" => validate_specialization_constants(field)?,
            "node" => validate_node(kind, field)?,
            "ray_tracing" => validate_ray_tracing(kind, field)?,
            "fragment" => validate_fragment(kind, field)?,
            _ => unreachable!("field names were checked against the schema"),
        }
    }
    Ok(())
}

fn validate_threads(
    kind: GpuEntryKind,
    field: &TextEntryField,
) -> Result<(), crate::text::TextParseError> {
    let values = array(&field.value, "threads")?;
    if values.len() != 3 {
        return Err(error(
            field.value.span,
            "threads must have three dimensions",
        ));
    }
    let mut dimensions = [0_u64; 3];
    for (index, value) in values.iter().enumerate() {
        dimensions[index] = unsigned(value, "thread count", 64)?;
    }
    let dynamic = dimensions == [0, 0, 0];
    if kind.has_threads() {
        if dynamic && kind != GpuEntryKind::Compute {
            return Err(error(
                field.value.span,
                "this threaded entry kind requires fixed thread counts",
            ));
        }
        if !dynamic && dimensions.contains(&0) {
            return Err(error(
                field.value.span,
                "fixed thread dimensions must all be positive",
            ));
        }
        if kind == GpuEntryKind::Tile && !dynamic && dimensions[2] != 1 {
            return Err(error(
                field.value.span,
                "tile entries require z thread count one",
            ));
        }
    } else if !dynamic {
        return Err(error(
            field.value.span,
            "this GPU entry kind must use [0, 0, 0] threads",
        ));
    }
    Ok(())
}

fn validate_root(field: &TextEntryField) -> Result<(), crate::text::TextParseError> {
    let Some(fields) = optional_object(field, &["size", "align_log2", "typeid", "space"])? else {
        return Ok(());
    };
    positive(member(fields, "size")?, "root size", 64)?;
    let alignment = unsigned(member(fields, "align_log2")?, "root align_log2", 8)?;
    if alignment > 63 {
        return Err(error(
            member(fields, "align_log2")?.span,
            "root align_log2 exceeds 63",
        ));
    }
    require_hex(member(fields, "typeid")?, "root typeid", 64)?;
    require_atom_in(
        member(fields, "space")?,
        &["device", "constant"],
        "root space",
    )
}

fn validate_immediate(field: &TextEntryField) -> Result<(), crate::text::TextParseError> {
    let Some(fields) = optional_object(field, &["size", "typeid"])? else {
        return Ok(());
    };
    let size = positive(member(fields, "size")?, "immediate size", 64)?;
    if size > 256 {
        return Err(error(
            member(fields, "size")?.span,
            "immediate size exceeds 256",
        ));
    }
    require_hex(member(fields, "typeid")?, "immediate typeid", 64)
}

fn validate_stage_fields(
    kind: GpuEntryKind,
    field: &TextEntryField,
) -> Result<(), crate::text::TextParseError> {
    let values = array(&field.value, &field.name)?;
    if field.name == "stage_input" && kind != GpuEntryKind::Fragment && !values.is_empty() {
        return Err(error(
            field.value.span,
            "only fragment entries can declare stage inputs",
        ));
    }
    if field.name == "stage_output"
        && !matches!(
            kind,
            GpuEntryKind::Vertex | GpuEntryKind::Mesh | GpuEntryKind::Fragment
        )
        && !values.is_empty()
    {
        return Err(error(
            field.value.span,
            "this entry kind cannot declare stage outputs",
        ));
    }
    let mut seen = Vec::new();
    let mut has_position_output = false;
    for value in values {
        let fields = object(value, "stage field")?;
        check_member_names(
            fields,
            &["semantic", "location", "type", "interpolation", "offset"],
            value.span,
            "stage field",
        )?;
        let semantic = member(fields, "semantic")?;
        validate_semantic(semantic)?;
        has_position_output |=
            field.name == "stage_output" && semantic_key(semantic)? == "position";
        if field.name == "stage_input" && semantic_key(semantic)? == "shading_rate" {
            return Err(error(semantic.span, "shading_rate is output-only"));
        }
        let location = unsigned(member(fields, "location")?, "stage location", 64)?;
        require_type(member(fields, "type")?)?;
        let interpolation = array(member(fields, "interpolation")?, "interpolation")?;
        for mode in interpolation {
            require_atom_in(
                mode,
                &[
                    "flat",
                    "perspective",
                    "noperspective",
                    "centroid",
                    "sample",
                    "invariant",
                    "per_primitive",
                ],
                "interpolation",
            )?;
        }
        let offset = unsigned(member(fields, "offset")?, "stage offset", 64)?;
        let key = format!("{}:{location}", semantic_key(semantic)?);
        if seen.contains(&key) {
            return Err(error(value.span, "duplicate stage semantic and location"));
        }
        seen.push(key);
        let _ = offset;
    }
    if field.name == "stage_output" && kind == GpuEntryKind::Vertex && !has_position_output {
        return Err(error(
            field.value.span,
            "vertex entries require a position stage output",
        ));
    }
    Ok(())
}

fn validate_semantic(value: &TextEntryValue) -> Result<(), crate::text::TextParseError> {
    match &value.kind {
        TextEntryValueKind::Atom(name) if SEMANTICS.contains(&name.as_str()) => Ok(()),
        TextEntryValueKind::Call(name, arguments)
            if matches!(
                name.as_str(),
                "color" | "color_index1" | "clip_distance" | "cull_distance"
            ) && arguments.len() == 1 =>
        {
            unsigned(&arguments[0], "semantic index", 32)?;
            Ok(())
        }
        TextEntryValueKind::Call(name, arguments)
            if name == "depth"
                && arguments.len() == 1
                && atom(&arguments[0])
                    .is_ok_and(|mode| ["any", "greater", "less"].contains(&mode)) =>
        {
            Ok(())
        }
        _ => Err(error(value.span, "unknown GPU stage semantic")),
    }
}

fn semantic_key(value: &TextEntryValue) -> Result<String, crate::text::TextParseError> {
    match &value.kind {
        TextEntryValueKind::Atom(name) => Ok(name.clone()),
        TextEntryValueKind::Call(name, arguments) => Ok(format!(
            "{name}({})",
            arguments
                .iter()
                .map(|argument| atom(argument).map(str::to_owned))
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        _ => Err(error(value.span, "invalid stage semantic")),
    }
}

fn validate_payload(field: &TextEntryField) -> Result<(), crate::text::TextParseError> {
    let Some(fields) = optional_object(field, &["object", "ray", "hit_attribute", "callable"])?
    else {
        return Ok(());
    };
    let mut total = 0_u64;
    for name in ["object", "ray", "hit_attribute", "callable"] {
        total = total.saturating_add(unsigned(member(fields, name)?, "payload size", 64)?);
    }
    if total == 0 {
        return Err(error(
            field.value.span,
            "an empty payload must be written as `none`",
        ));
    }
    Ok(())
}

fn validate_mesh(
    kind: GpuEntryKind,
    field: &TextEntryField,
) -> Result<(), crate::text::TextParseError> {
    let Some(fields) = optional_object(field, &["max_vertices", "max_primitives", "topology"])?
    else {
        if kind == GpuEntryKind::Mesh {
            return Err(error(field.value.span, "mesh entries require mesh limits"));
        }
        return Ok(());
    };
    if kind != GpuEntryKind::Mesh {
        return Err(error(
            field.value.span,
            "only mesh entries can declare mesh limits",
        ));
    }
    let vertices = positive(member(fields, "max_vertices")?, "mesh max_vertices", 64)?;
    let primitives = positive(member(fields, "max_primitives")?, "mesh max_primitives", 64)?;
    if vertices > 256 || primitives > 512 {
        return Err(error(
            field.value.span,
            "mesh limits exceed the supported range",
        ));
    }
    require_atom_in(
        member(fields, "topology")?,
        &["points", "lines", "triangles"],
        "mesh topology",
    )
}

fn validate_capabilities(field: &TextEntryField) -> Result<(), crate::text::TextParseError> {
    let values = array(&field.value, "capabilities")?;
    let mut seen = Vec::new();
    for value in values {
        let name = atom(value)?;
        if !CAPABILITIES.contains(&name) {
            return Err(error(value.span, "unknown optional GPU capability"));
        }
        if seen.contains(&name) {
            return Err(error(value.span, "duplicate GPU capability"));
        }
        seen.push(name);
    }
    Ok(())
}

fn validate_subgroup(field: &TextEntryField) -> Result<(), crate::text::TextParseError> {
    if atom(&field.value)? == "any" {
        return Ok(());
    }
    let size = unsigned(&field.value, "subgroup_size", 8)?;
    if !(4..=128).contains(&size) || !size.is_power_of_two() {
        return Err(error(
            field.value.span,
            "subgroup_size must be any or a power of two from 4 through 128",
        ));
    }
    Ok(())
}

fn validate_floating_point(field: &TextEntryField) -> Result<(), crate::text::TextParseError> {
    let values = array(&field.value, "floating_point")?;
    let mut seen = Vec::new();
    for value in values {
        let mode = atom(value)?;
        if !["fast", "ieee_denormals"].contains(&mode) {
            return Err(error(value.span, "unknown floating-point mode"));
        }
        if seen.contains(&mode) {
            return Err(error(value.span, "duplicate floating-point mode"));
        }
        seen.push(mode);
    }
    Ok(())
}

fn validate_node(
    kind: GpuEntryKind,
    field: &TextEntryField,
) -> Result<(), crate::text::TextParseError> {
    let Some(fields) = optional_object(
        field,
        &[
            "launch",
            "max_dispatch_grid",
            "input_record_size",
            "outputs",
        ],
    )?
    else {
        if kind == GpuEntryKind::Node {
            return Err(error(
                field.value.span,
                "node entries require a node record",
            ));
        }
        return Ok(());
    };
    if kind != GpuEntryKind::Node {
        return Err(error(
            field.value.span,
            "only node entries can declare node metadata",
        ));
    }
    require_atom_in(
        member(fields, "launch")?,
        &["broadcast", "coalescing", "thread"],
        "node launch",
    )?;
    validate_three_counts(member(fields, "max_dispatch_grid")?, "max_dispatch_grid")?;
    unsigned(
        member(fields, "input_record_size")?,
        "input_record_size",
        64,
    )?;
    let outputs = array(member(fields, "outputs")?, "node outputs")?;
    let mut names = Vec::new();
    for output in outputs {
        let members = object(output, "node output")?;
        check_member_names(
            members,
            &["name", "record_size", "max_records"],
            output.span,
            "node output",
        )?;
        let name = string(member(members, "name")?)?;
        if names.contains(&name) {
            return Err(error(
                member(members, "name")?.span,
                "duplicate node output name",
            ));
        }
        names.push(name);
        unsigned(
            member(members, "record_size")?,
            "node output record_size",
            64,
        )?;
        unsigned(
            member(members, "max_records")?,
            "node output max_records",
            64,
        )?;
    }
    Ok(())
}

fn validate_ray_tracing(
    kind: GpuEntryKind,
    field: &TextEntryField,
) -> Result<(), crate::text::TextParseError> {
    let Some(fields) = optional_object(field, &["max_recursion_depth"])? else {
        if is_ray_kind(kind) {
            return Err(error(
                field.value.span,
                "ray-tracing entries require a recursion-depth record",
            ));
        }
        return Ok(());
    };
    if !is_ray_kind(kind) {
        return Err(error(
            field.value.span,
            "ray-tracing metadata is only valid for ray-tracing entries",
        ));
    }
    let depth = unsigned(
        member(fields, "max_recursion_depth")?,
        "max_recursion_depth",
        8,
    )?;
    if depth > 31 {
        return Err(error(
            member(fields, "max_recursion_depth")?.span,
            "ray recursion depth exceeds 31",
        ));
    }
    Ok(())
}

fn validate_fragment(
    kind: GpuEntryKind,
    field: &TextEntryField,
) -> Result<(), crate::text::TextParseError> {
    let Some(fields) = optional_object(
        field,
        &[
            "early_fragment_tests",
            "writes_depth",
            "discards_or_demotes",
            "reads_attachments",
        ],
    )?
    else {
        return Ok(());
    };
    if kind != GpuEntryKind::Fragment {
        return Err(error(
            field.value.span,
            "fragment metadata is only valid for fragment entries",
        ));
    }
    for name in [
        "early_fragment_tests",
        "discards_or_demotes",
        "reads_attachments",
    ] {
        require_atom_in(member(fields, name)?, &["true", "false"], name)?;
    }
    require_atom_in(
        member(fields, "writes_depth")?,
        &["none", "any", "greater", "less"],
        "writes_depth",
    )
}

fn is_ray_kind(kind: GpuEntryKind) -> bool {
    matches!(
        kind,
        GpuEntryKind::Raygen
            | GpuEntryKind::Intersection
            | GpuEntryKind::Anyhit
            | GpuEntryKind::Closesthit
            | GpuEntryKind::Miss
            | GpuEntryKind::Callable
    )
}

fn validate_three_counts(
    value: &TextEntryValue,
    name: &str,
) -> Result<(), crate::text::TextParseError> {
    let values = array(value, name)?;
    if values.len() != 3 {
        return Err(error(value.span, "expected exactly three dimensions"));
    }
    for value in values {
        unsigned(value, name, 64)?;
    }
    Ok(())
}

fn optional_object<'a>(
    field: &'a TextEntryField,
    members: &[&str],
) -> Result<Option<&'a [TextEntryField]>, crate::text::TextParseError> {
    if is_atom(&field.value, "none") {
        return Ok(None);
    }
    let fields = object(&field.value, &field.name)?;
    check_member_names(fields, members, field.value.span, &field.name)?;
    Ok(Some(fields))
}

pub(super) fn check_member_names(
    fields: &[TextEntryField],
    expected: &[&str],
    span: Span,
    context: &str,
) -> Result<(), crate::text::TextParseError> {
    for field in fields {
        if !expected.contains(&field.name.as_str()) {
            return Err(error(
                field.span,
                &format!("unknown {context} field `{}`", field.name),
            ));
        }
        if fields
            .iter()
            .filter(|other| other.name == field.name)
            .count()
            != 1
        {
            return Err(error(
                field.span,
                &format!("duplicate {context} field `{}`", field.name),
            ));
        }
    }
    if let Some(missing) = expected
        .iter()
        .find(|name| !fields.iter().any(|field| field.name == **name))
    {
        return Err(error(span, &format!("{context} is missing `{missing}`")));
    }
    Ok(())
}
