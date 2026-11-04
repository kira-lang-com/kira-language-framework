use tir_format::StringTable;

use crate::binary::FormatError;
use crate::text::{TextGpuEntry, TextImage};

use super::text_values as value;
use super::{GpuEntryKind, GpuEntryRecord, encode_gpu_entry_records};

mod interface;
mod metadata;

const ENTRY_FIELDS: &[&str] = &[
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
];
const VISIBLE_FIELDS: &[&str] = &["typeid"];

/// Lowers parsed GPU entry text records using indexes assigned by the caller.
/// Function symbols are supplied without their leading `@`; the kind tells the
/// resolver whether to use the GPU-function or GPU-entry function space.
pub fn encode_text_gpu_entry_records(
    image: &TextImage,
    strings: &StringTable,
    value_type_count: u64,
    constant_count: u64,
    mut resolve_function: impl FnMut(&str, GpuEntryKind) -> Option<u64>,
    mut resolve_type: impl FnMut(&tir_types::Type) -> Option<u64>,
    mut resolve_constant: impl FnMut(&tir_types::Type, &str) -> Option<u64>,
) -> Result<Vec<u8>, FormatError> {
    let mut records = Vec::with_capacity(image.gpu_entries.len());
    for entry in &image.gpu_entries {
        records.push(encode_entry(
            image,
            entry,
            value_type_count,
            constant_count,
            &mut resolve_function,
            &mut resolve_type,
            &mut resolve_constant,
        )?);
    }
    encode_gpu_entry_records(&records, strings, value_type_count)
}

fn encode_entry(
    image: &TextImage,
    entry: &TextGpuEntry,
    value_type_count: u64,
    constant_count: u64,
    resolve_function: &mut impl FnMut(&str, GpuEntryKind) -> Option<u64>,
    resolve_type: &mut impl FnMut(&tir_types::Type) -> Option<u64>,
    resolve_constant: &mut impl FnMut(&tir_types::Type, &str) -> Option<u64>,
) -> Result<GpuEntryRecord, FormatError> {
    if let Some(error) = &entry.error {
        return Err(value::invalid(
            error.span,
            format!("invalid GPU entry record: {}", error.message),
        ));
    }
    if !entry.header_valid {
        return Err(value::invalid(entry.span, "GPU entry header is invalid"));
    }
    let kind = entry
        .kind
        .as_deref()
        .and_then(entry_kind)
        .ok_or_else(|| value::invalid(entry.span, "unknown GPU entry kind"))?;
    let function = entry
        .function
        .as_deref()
        .ok_or_else(|| value::invalid(entry.span, "GPU entry has no function symbol"))?;
    let function_index = resolve_function(function, kind).ok_or_else(|| {
        value::invalid(entry.span, format!("unresolved GPU function @{function}"))
    })?;
    let expected = if kind == GpuEntryKind::Visible {
        VISIBLE_FIELDS
    } else {
        ENTRY_FIELDS
    };
    value::fields(&entry.fields, expected, "GPU entry record")?;

    if kind == GpuEntryKind::Visible {
        return Ok(empty_entry(
            function_index,
            kind,
            Some(value::hexadecimal(
                value::field(&entry.fields, "typeid")?,
                "visible typeid",
            )?),
        ));
    }

    let threads = interface::triple(value::field(&entry.fields, "threads")?, "threads")?;
    let root = interface::optional_root(value::field(&entry.fields, "root")?)?;
    let (immediate_size, immediate_type_id) =
        interface::optional_immediate(value::field(&entry.fields, "immediate")?)?;
    let stage_input = interface::stage_fields(
        value::field(&entry.fields, "stage_input")?,
        value_type_count,
        resolve_type,
    )?;
    let stage_output = interface::stage_fields(
        value::field(&entry.fields, "stage_output")?,
        value_type_count,
        resolve_type,
    )?;
    let payload_sizes = metadata::payload(value::field(&entry.fields, "payload")?)?;
    let (mesh_maximum, mesh_topology) = metadata::mesh(kind, value::field(&entry.fields, "mesh")?)?;
    let threadgroup_memory = value::number(
        value::field(&entry.fields, "threadgroup_memory")?,
        "threadgroup_memory",
    )?;
    let capabilities = metadata::capabilities(value::field(&entry.fields, "capabilities")?)?;
    let subgroup_size_log2 =
        metadata::subgroup_size(value::field(&entry.fields, "subgroup_size")?)?;
    let floating_point_flags =
        metadata::floating_point(value::field(&entry.fields, "floating_point")?)?;
    let specialization_constants = metadata::specialization_constants(
        image,
        value::field(&entry.fields, "specialization_constants")?,
        value_type_count,
        constant_count,
        resolve_type,
        resolve_constant,
    )?;
    let node = metadata::node(kind, value::field(&entry.fields, "node")?)?;
    let ray_recursion_depth =
        metadata::ray_tracing(kind, value::field(&entry.fields, "ray_tracing")?)?;
    let fragment_flags = metadata::fragment(kind, value::field(&entry.fields, "fragment")?)?;

    Ok(GpuEntryRecord {
        function_index,
        kind,
        visible_type_id: None,
        threads,
        root,
        immediate_size,
        immediate_type_id,
        stage_input,
        stage_output,
        payload_sizes,
        mesh_maximum,
        mesh_topology,
        threadgroup_memory,
        capabilities,
        subgroup_size_log2,
        floating_point_flags,
        specialization_constants,
        node,
        ray_recursion_depth,
        fragment_flags,
    })
}

fn entry_kind(name: &str) -> Option<GpuEntryKind> {
    Some(match name {
        "compute" => GpuEntryKind::Compute,
        "object" => GpuEntryKind::Object,
        "mesh" => GpuEntryKind::Mesh,
        "vertex" => GpuEntryKind::Vertex,
        "fragment" => GpuEntryKind::Fragment,
        "tile" => GpuEntryKind::Tile,
        "raygen" => GpuEntryKind::Raygen,
        "intersection" => GpuEntryKind::Intersection,
        "anyhit" => GpuEntryKind::Anyhit,
        "closesthit" => GpuEntryKind::Closesthit,
        "miss" => GpuEntryKind::Miss,
        "callable" => GpuEntryKind::Callable,
        "node" => GpuEntryKind::Node,
        "visible" => GpuEntryKind::Visible,
        _ => return None,
    })
}

fn empty_entry(
    function_index: u64,
    kind: GpuEntryKind,
    visible_type_id: Option<u64>,
) -> GpuEntryRecord {
    GpuEntryRecord {
        function_index,
        kind,
        visible_type_id,
        threads: [0; 3],
        root: None,
        immediate_size: 0,
        immediate_type_id: 0,
        stage_input: Vec::new(),
        stage_output: Vec::new(),
        payload_sizes: [0; 4],
        mesh_maximum: [0; 2],
        mesh_topology: 0,
        threadgroup_memory: 0,
        capabilities: 0,
        subgroup_size_log2: 0,
        floating_point_flags: 0,
        specialization_constants: Vec::new(),
        node: None,
        ray_recursion_depth: None,
        fragment_flags: None,
    }
}
