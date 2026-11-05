use crate::leb::write_uleb;
use crate::types::{
    BinaryEntryKind, BinaryFpMode, BinaryGpuEntry, BinaryGpuInterfaceRole, BinaryGpuTable,
    BinarySignatureTable, BinaryTypeTable,
};
use crate::{BinaryModule, FormatError, Header, Section};
use tir_core::ContentHash;
use tir_target::{Platform, Profile};

impl BinaryGpuTable {
    /// Encodes the GPU section and validates it against entries, types, and constants.
    pub fn encode(
        &self,
        signatures: &BinarySignatureTable,
        types: &BinaryTypeTable,
        constants: &crate::BinaryConstantTable,
    ) -> Result<Option<Vec<u8>>, FormatError> {
        if self.entries.is_empty()
            && signatures
                .records
                .iter()
                .all(|signature| signature.entry_kind.is_none())
        {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        write_count(&mut bytes, self.entries.len())?;
        for entry in &self.entries {
            encode_entry(&mut bytes, entry)?;
        }
        let module = BinaryModule {
            header: Header {
                year: 26,
                month: 11,
                week: 1,
                revision: 0,
                flags: 1 << 6,
                profile: Profile::App,
                platform: Platform::Portable,
                section_count: 1,
                file_size: 0,
            },
            sections: vec![Section::from_uncompressed(10, bytes.clone())],
            content_hash: ContentHash::of(&[]),
        };
        module.gpu_table(u64::MAX, signatures, types, constants)?;
        Ok(Some(bytes))
    }
}

fn encode_entry(bytes: &mut Vec<u8>, entry: &BinaryGpuEntry) -> Result<(), FormatError> {
    write_uleb(entry.function_index, bytes).map_err(io_error)?;
    bytes.push(entry_kind(entry.kind));
    for dimension in entry.threads {
        write_uleb(dimension, bytes).map_err(io_error)?;
    }
    write_count(bytes, entry.interfaces.len())?;
    for interface in &entry.interfaces {
        bytes.push(match interface.role {
            BinaryGpuInterfaceRole::Root => 0,
            BinaryGpuInterfaceRole::Immediate => 1,
            BinaryGpuInterfaceRole::StageParameter => 2,
            BinaryGpuInterfaceRole::IntersectionAttributes => 3,
        });
        if interface.role != BinaryGpuInterfaceRole::IntersectionAttributes {
            let parameter = interface
                .parameter_index
                .ok_or_else(|| malformed("GPU interface is missing its parameter index"))?;
            write_uleb(parameter, bytes).map_err(io_error)?;
        } else if interface.parameter_index.is_some() {
            return Err(malformed(
                "intersection-attributes interface cannot carry a parameter index",
            ));
        }
        write_uleb(interface.ty, bytes).map_err(io_error)?;
    }
    let payload_size = entry
        .payload_size
        .map_or(Some(0), |size| size.checked_add(1))
        .ok_or_else(|| malformed("GPU payload size overflows"))?;
    write_uleb(payload_size, bytes).map_err(io_error)?;
    write_uleb(entry.threadgroup_memory, bytes).map_err(io_error)?;
    write_uleb(entry.mesh_maximum_vertices, bytes).map_err(io_error)?;
    write_uleb(entry.mesh_maximum_primitives, bytes).map_err(io_error)?;
    bytes.push(entry.mesh_topology);
    write_uleb(entry.capabilities, bytes).map_err(io_error)?;
    write_uleb(entry.subgroup_size, bytes).map_err(io_error)?;
    bytes.push(fp_mode(entry.floating_point_mode));
    write_count(bytes, entry.specialization_constants.len())?;
    for constant in &entry.specialization_constants {
        write_uleb(u64::from(constant.id), bytes).map_err(io_error)?;
        write_uleb(constant.ty, bytes).map_err(io_error)?;
        write_uleb(constant.default_constant, bytes).map_err(io_error)?;
    }
    Ok(())
}

fn entry_kind(kind: BinaryEntryKind) -> u8 {
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

fn fp_mode(mode: Option<BinaryFpMode>) -> u8 {
    match mode {
        None => 0,
        Some(BinaryFpMode::Precise) => 1,
        Some(BinaryFpMode::Fast) => 2,
        Some(BinaryFpMode::IeeeDenormals) => 3,
    }
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    let count = u64::try_from(count).map_err(|_| malformed("GPU count overflows ULEB128"))?;
    if count >= 1 << 32 {
        return Err(malformed("GPU count must be less than 2^32"));
    }
    write_uleb(count, bytes).map_err(io_error)
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(format!("GPU encoding failed: {error}"))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
