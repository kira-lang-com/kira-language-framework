use crate::leb::write_uleb;
use crate::types::{BinaryGlobalTable, BinarySignatureTable};
use crate::{BinaryAnalysisSummary, BinaryAssemblyRecord, FormatError};

impl BinaryAnalysisSummary {
    /// Encodes ANALYSIS after validating its function, global, and ASM references.
    pub fn encode(
        &self,
        signatures: &BinarySignatureTable,
        globals: &BinaryGlobalTable,
        assemblies: &[BinaryAssemblyRecord],
    ) -> Result<Vec<u8>, FormatError> {
        if self.functions.len() != signatures.records.len() {
            return Err(malformed(
                "ANALYSIS function count does not match SIGNATURES",
            ));
        }
        let mut bytes = Vec::new();
        write_count(&mut bytes, self.functions.len())?;
        for function in &self.functions {
            if function.trust_class > 2 {
                return Err(malformed("invalid ANALYSIS trust class"));
            }
            bytes.push(function.trust_class);
            bytes.push(function.reason_mask);
        }

        write_count(&mut bytes, self.sandbox_regions.len())?;
        let mut previous_memory = None;
        for region in &self.sandbox_regions {
            if previous_memory.is_some_and(|previous| region.memory_index <= previous)
                || region.base_global_index >= globals.records.len() as u64
                || region.size_global_index >= globals.records.len() as u64
                || region.base_global_index == region.size_global_index
            {
                return Err(malformed("invalid or unordered ANALYSIS sandbox region"));
            }
            previous_memory = Some(region.memory_index);
            write_uleb(region.memory_index, &mut bytes).map_err(io_error)?;
            write_uleb(region.base_global_index, &mut bytes).map_err(io_error)?;
            write_uleb(region.size_global_index, &mut bytes).map_err(io_error)?;
        }

        write_count(&mut bytes, self.indirect_call_typeids.len())?;
        let mut previous_typeid = None;
        for typeid in &self.indirect_call_typeids {
            if previous_typeid.is_some_and(|previous| *typeid <= previous) {
                return Err(malformed("ANALYSIS typeids are not strictly ordered"));
            }
            previous_typeid = Some(*typeid);
            bytes.extend_from_slice(&typeid.to_le_bytes());
        }

        write_count(&mut bytes, self.assembly_regions.len())?;
        let mut previous_region = None;
        for region in &self.assembly_regions {
            let key = (region.function_index, region.instruction_ordinal);
            if region.function_index >= signatures.records.len() as u64
                || region.assembly_record_index >= assemblies.len() as u64
                || !(1..=3).contains(&region.architecture)
                || region.effects & !0x0f != 0
                || region.effects & (1 << 2) != 0 && region.effects & 0b0011 != 0
                || previous_region.is_some_and(|previous| key <= previous)
            {
                return Err(malformed("invalid or unordered ANALYSIS assembly region"));
            }
            previous_region = Some(key);
            write_uleb(region.function_index, &mut bytes).map_err(io_error)?;
            write_uleb(region.instruction_ordinal, &mut bytes).map_err(io_error)?;
            write_uleb(region.assembly_record_index, &mut bytes).map_err(io_error)?;
            bytes.push(region.architecture);
            bytes.push(region.effects);
        }
        Ok(bytes)
    }
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    let count = u64::try_from(count).map_err(|_| malformed("ANALYSIS count overflows ULEB128"))?;
    write_uleb(count, bytes).map_err(io_error)
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(format!("ANALYSIS encoding failed: {error}"))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
