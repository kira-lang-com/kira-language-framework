use tir_format::StringTable;
use tir_format::leb::write_uleb;

use crate::binary::FormatError;

use super::{GpuEntryKind, GpuEntryRecord, GpuNodeRecord, decode_gpu_entries};

/// Encodes concatenated GPU entry records and checks the result by decoding it.
pub fn encode_gpu_entry_records(
    records: &[GpuEntryRecord],
    strings: &StringTable,
    value_type_count: u64,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    for record in records {
        write_record(record, strings, &mut bytes)?;
    }
    let decoded = decode_gpu_entries(&bytes, strings, value_type_count)?;
    if decoded != records {
        return Err(malformed("GPU entry record contains non-encoded fields"));
    }
    Ok(bytes)
}

fn write_record(
    record: &GpuEntryRecord,
    strings: &StringTable,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_index(record.function_index, output)?;
    output.push(record.kind as u8);
    if record.kind == GpuEntryKind::Visible {
        let type_id = record
            .visible_type_id
            .ok_or_else(|| malformed("visible GPU entry has no type id"))?;
        output.extend_from_slice(&type_id.to_le_bytes());
        return Ok(());
    }

    for thread_count in record.threads {
        write_index(thread_count, output)?;
    }
    if let Some(root) = &record.root {
        write_index(root.size, output)?;
        output.push(root.alignment_log2);
        output.extend_from_slice(&root.type_id.to_le_bytes());
        output.push(root.address_space);
    } else {
        write_index(0, output)?;
        output.push(0);
        output.extend_from_slice(&0u64.to_le_bytes());
        output.push(0);
    }
    write_index(record.immediate_size, output)?;
    output.extend_from_slice(&record.immediate_type_id.to_le_bytes());
    write_stage_fields(&record.stage_input, output)?;
    write_stage_fields(&record.stage_output, output)?;
    for size in record.payload_sizes {
        write_index(size, output)?;
    }
    for limit in record.mesh_maximum {
        write_index(limit, output)?;
    }
    output.push(record.mesh_topology);
    write_index(record.threadgroup_memory, output)?;
    write_index(record.capabilities, output)?;
    output.push(record.subgroup_size_log2);
    output.push(record.floating_point_flags);
    write_index(record.specialization_constants.len() as u64, output)?;
    for constant in &record.specialization_constants {
        write_index(u64::from(constant.id), output)?;
        write_index(constant.type_index, output)?;
        write_index(constant.default_constant, output)?;
    }
    if record.kind == GpuEntryKind::Node {
        let node = record
            .node
            .as_ref()
            .ok_or_else(|| malformed("node GPU entry has no node record"))?;
        write_node(node, strings, output)?;
    }
    if record.kind.is_ray_tracing() {
        let depth = record
            .ray_recursion_depth
            .ok_or_else(|| malformed("ray-tracing GPU entry has no recursion depth"))?;
        output.push(depth);
    }
    if record.kind == GpuEntryKind::Fragment {
        let flags = record
            .fragment_flags
            .ok_or_else(|| malformed("fragment GPU entry has no fragment flags"))?;
        output.push(flags);
    }
    Ok(())
}

fn write_stage_fields(
    fields: &[super::GpuStageField],
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_index(fields.len() as u64, output)?;
    for field in fields {
        output.push(field.semantic);
        write_index(field.location, output)?;
        write_index(field.type_index, output)?;
        output.push(field.interpolation);
        write_index(field.byte_offset, output)?;
    }
    Ok(())
}

fn write_node(
    node: &GpuNodeRecord,
    strings: &StringTable,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    output.push(node.launch_mode);
    for dimension in node.maximum_dispatch_grid {
        write_index(dimension, output)?;
    }
    write_index(node.input_record_size, output)?;
    write_index(node.outputs.len() as u64, output)?;
    for node_output in &node.outputs {
        let name_index = (0..strings.len())
            .find(|index| strings.get(*index) == Some(node_output.name.as_str()))
            .ok_or_else(|| malformed("GPU node output name is absent from STRINGS"))?;
        write_index(name_index as u64, output)?;
        write_index(node_output.record_size, output)?;
        write_index(node_output.maximum_records, output)?;
    }
    Ok(())
}

fn write_index(value: u64, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output).map_err(|error| malformed(error.to_string()))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
