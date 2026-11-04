use tir_core::ContentHash;
use tir_target::{Platform, Profile};

use crate::leb::write_uleb;
use crate::types::{
    BinaryConstantTable, BinaryEntryKind, BinaryFpMode, BinaryGpuEntry, BinaryGpuInterfaceRole,
    BinaryGpuTable, BinarySignatureTable, BinaryTypeTable,
};
use crate::{BinaryModule, FormatError, Header, Section, StringTable};

pub(super) fn encode_gpu_table(
    table: &BinaryGpuTable,
    strings: &StringTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    constants: &BinaryConstantTable,
) -> Result<Vec<u8>, FormatError> {
    if table.entries.is_empty() {
        return Err(malformed(
            "GPU section is omitted when there are no entry functions",
        ));
    }

    let mut output = Vec::new();
    write_count(table.entries.len(), "entry", &mut output)?;
    let expected_entries = signatures
        .records
        .iter()
        .enumerate()
        .filter(|(_, signature)| signature.entry_kind.is_some())
        .map(|(index, _)| u64::try_from(index))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| malformed("GPU entry index overflows u64"))?;
    if table.entries.len() != expected_entries.len() {
        return Err(malformed(
            "GPU entry count does not match SIGNATURES entries",
        ));
    }
    for (entry, expected_index) in table.entries.iter().zip(expected_entries) {
        if entry.function_index != expected_index {
            return Err(malformed(
                "GPU entries are not in SIGNATURES declaration order",
            ));
        }
        write_entry(entry, strings, signatures, types, &mut output)?;
    }

    let strings_bytes = encode_strings(strings)?;
    let types_bytes = types.encode(strings)?;
    let constants_bytes = constants.encode(strings, types)?;
    let signatures_bytes = signatures.encode(strings, types)?;
    let module = BinaryModule {
        header: Header {
            year: 1,
            month: 1,
            week: 1,
            revision: 0,
            flags: 0,
            profile: Profile::App,
            platform: Platform::Tessera,
            section_count: 5,
            file_size: 0,
        },
        sections: vec![
            Section::from_uncompressed(1, strings_bytes),
            Section::from_uncompressed(4, types_bytes),
            Section::from_uncompressed(5, constants_bytes),
            Section::from_uncompressed(7, signatures_bytes),
            Section::from_uncompressed(10, output.clone()),
        ],
        content_hash: ContentHash::of(&[]),
    };
    if module.gpu_table(u64::MAX, signatures, types, constants)? != *table {
        return Err(malformed(
            "encoded GPU entries do not match the typed table",
        ));
    }
    Ok(output)
}

fn write_entry(
    entry: &BinaryGpuEntry,
    strings: &StringTable,
    signatures: &BinarySignatureTable,
    types: &BinaryTypeTable,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let signature = signatures
        .records
        .get(
            usize::try_from(entry.function_index)
                .map_err(|_| malformed("GPU entry function index overflows memory"))?,
        )
        .ok_or_else(|| malformed("GPU entry function index is out of range"))?;
    if signature.entry_kind != Some(entry.kind) {
        return Err(malformed("GPU entry kind disagrees with SIGNATURES"));
    }
    let function_type = super::function_type(signature.function_type, types, output.len())?;
    super::validate_entry_result(entry.kind, function_type, types, output.len())?;
    super::validate_threads(entry.kind, entry.threads, output.len())?;
    super::validate_payload(entry.kind, entry.payload_size, output.len())?;
    super::validate_mesh(
        entry.kind,
        entry.mesh_maximum_vertices,
        entry.mesh_maximum_primitives,
        entry.mesh_topology,
        output.len(),
    )?;
    if entry.capabilities & !((1 << 19) - 1) != 0 {
        return Err(malformed("GPU capability mask has unknown bits"));
    }
    if entry.subgroup_size != 0
        && (!(4..=128).contains(&entry.subgroup_size) || !entry.subgroup_size.is_power_of_two())
    {
        return Err(malformed("invalid GPU subgroup size"));
    }
    if entry.floating_point_mode != signature.fp_mode {
        return Err(malformed("GPU FP mode disagrees with SIGNATURES"));
    }

    write_index(entry.function_index, "entry function", output)?;
    output.push(entry_kind_code(entry.kind));
    for dimension in entry.threads {
        write_index(dimension, "thread dimension", output)?;
    }
    write_interfaces(&entry.interfaces, output)?;
    let payload_size = entry
        .payload_size
        .map_or(Some(0), |size| size.checked_add(1))
        .ok_or_else(|| malformed("GPU payload size overflows"))?;
    write_index(payload_size, "payload size", output)?;
    write_index(entry.threadgroup_memory, "threadgroup memory", output)?;
    write_index(entry.mesh_maximum_vertices, "mesh vertex limit", output)?;
    write_index(
        entry.mesh_maximum_primitives,
        "mesh primitive limit",
        output,
    )?;
    output.push(entry.mesh_topology);
    write_index(entry.capabilities, "capability mask", output)?;
    write_index(entry.subgroup_size, "subgroup size", output)?;
    output.push(fp_mode_code(entry.floating_point_mode));
    write_count(
        entry.specialization_constants.len(),
        "specialization constant",
        output,
    )?;
    for constant in &entry.specialization_constants {
        write_index(u64::from(constant.id), "specialization constant id", output)?;
        write_index(constant.ty, "specialization constant type", output)?;
        write_index(
            constant.default_constant,
            "specialization constant default",
            output,
        )?;
    }
    match (entry.kind, &entry.node) {
        (BinaryEntryKind::Node, Some(node)) => write_node(node, strings, output)?,
        (BinaryEntryKind::Node, None) => return Err(malformed("node entry has no node record")),
        (_, Some(_)) => return Err(malformed("non-node entry has a node record")),
        (_, None) => {}
    }
    Ok(())
}

fn write_interfaces(
    interfaces: &[crate::types::BinaryGpuInterface],
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(interfaces.len(), "interface", output)?;
    for interface in interfaces {
        output.push(match interface.role {
            BinaryGpuInterfaceRole::Root => 0,
            BinaryGpuInterfaceRole::Immediate => 1,
            BinaryGpuInterfaceRole::StageParameter => 2,
            BinaryGpuInterfaceRole::IntersectionAttributes => 3,
        });
        match (interface.role, interface.parameter_index) {
            (BinaryGpuInterfaceRole::IntersectionAttributes, None) => {}
            (BinaryGpuInterfaceRole::IntersectionAttributes, Some(_)) => {
                return Err(malformed(
                    "intersection-attributes interface cannot name a parameter",
                ));
            }
            (_, Some(index)) => write_index(index, "interface parameter", output)?,
            (_, None) => return Err(malformed("GPU interface is missing its parameter index")),
        }
        write_index(interface.ty, "interface type", output)?;
    }
    Ok(())
}

fn write_node(
    node: &crate::types::BinaryGpuNodeRecord,
    strings: &StringTable,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if node.launch_mode > 2 || node.maximum_dispatch_grid.contains(&0) {
        return Err(malformed("invalid GPU node launch mode or grid"));
    }
    output.push(node.launch_mode);
    for dimension in node.maximum_dispatch_grid {
        write_index(dimension, "node dispatch-grid dimension", output)?;
    }
    write_index(node.input_record_size, "node input record size", output)?;
    write_count(node.outputs.len(), "node output", output)?;
    for record in &node.outputs {
        if record.name.is_empty() || record.record_size == 0 || record.maximum_records == 0 {
            return Err(malformed("invalid GPU node output record"));
        }
        let index = strings
            .index_of(&record.name)
            .ok_or_else(|| malformed("GPU node output name is absent from STRINGS"))?;
        let index =
            u64::try_from(index).map_err(|_| malformed("GPU node output name index overflows"))?;
        write_index(index, "node output name", output)?;
        output.push(record.kind as u8);
        write_index(record.record_size, "node output record size", output)?;
        write_index(
            record.maximum_records,
            "node output maximum records",
            output,
        )?;
    }
    Ok(())
}

fn encode_strings(strings: &StringTable) -> Result<Vec<u8>, FormatError> {
    let mut output = Vec::new();
    strings
        .encode(&mut output)
        .map_err(|error| malformed(format!("STRINGS encoding failed: {error}")))?;
    Ok(output)
}

fn write_count(count: usize, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let count =
        u64::try_from(count).map_err(|_| malformed(format!("GPU {label} count overflows")))?;
    if count >= 1 << 32 {
        return Err(malformed(format!("GPU {label} count exceeds the limit")));
    }
    write_index(count, label, output)
}

fn write_index(value: u64, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output).map_err(|error| malformed(format!("GPU {label}: {error}")))
}

fn entry_kind_code(kind: BinaryEntryKind) -> u8 {
    match kind {
        BinaryEntryKind::Compute => 1,
        BinaryEntryKind::Object => 2,
        BinaryEntryKind::Mesh => 3,
        BinaryEntryKind::Vertex => 4,
        BinaryEntryKind::Fragment => 5,
        BinaryEntryKind::Tile => 6,
        BinaryEntryKind::Raygen => 7,
        BinaryEntryKind::Intersection => 8,
        BinaryEntryKind::Anyhit => 9,
        BinaryEntryKind::Closesthit => 10,
        BinaryEntryKind::Miss => 11,
        BinaryEntryKind::Callable => 12,
        BinaryEntryKind::Node => 13,
    }
}

fn fp_mode_code(mode: Option<BinaryFpMode>) -> u8 {
    match mode {
        None => 0,
        Some(BinaryFpMode::Precise) => 1,
        Some(BinaryFpMode::Fast) => 2,
        Some(BinaryFpMode::IeeeDenormals) => 3,
    }
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
