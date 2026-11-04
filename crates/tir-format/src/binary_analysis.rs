use std::collections::HashSet;

use crate::leb::{read_uleb, write_uleb};
use crate::{BinaryModule, FormatError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryFunctionAnalysis {
    pub trust_class: u8,
    pub reason_mask: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinarySandboxRegion {
    pub memory_index: u64,
    pub base_global_index: u64,
    pub size_global_index: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryAssemblyRegion {
    pub function_index: u64,
    pub instruction_ordinal: u64,
    pub assembly_record_index: u64,
    pub architecture: u8,
    pub effects: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryAnalysisSummary {
    pub functions: Vec<BinaryFunctionAnalysis>,
    pub sandbox_regions: Vec<BinarySandboxRegion>,
    pub indirect_call_typeids: Vec<u64>,
    pub assembly_regions: Vec<BinaryAssemblyRegion>,
}

impl BinaryAnalysisSummary {
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        let mut output = Vec::new();
        write_count(self.functions.len(), "function", &mut output)?;
        for function in &self.functions {
            if function.trust_class > 2 {
                return Err(FormatError::malformed(None, "invalid ANALYSIS trust class"));
            }
            output.extend_from_slice(&[function.trust_class, function.reason_mask]);
        }

        write_count(self.sandbox_regions.len(), "sandbox-region", &mut output)?;
        let mut used_globals = HashSet::new();
        for (index, region) in self.sandbox_regions.iter().enumerate() {
            if u64::try_from(index).ok() != Some(region.memory_index)
                || region.base_global_index == region.size_global_index
                || !used_globals.insert(region.base_global_index)
                || !used_globals.insert(region.size_global_index)
            {
                return Err(FormatError::malformed(
                    None,
                    "invalid or reused ANALYSIS sandbox-region index",
                ));
            }
            for value in [
                region.memory_index,
                region.base_global_index,
                region.size_global_index,
            ] {
                write_index(value, "sandbox-region index", &mut output)?;
            }
        }

        write_count(
            self.indirect_call_typeids.len(),
            "indirect-call typeid",
            &mut output,
        )?;
        let mut previous_typeid = None;
        for typeid in &self.indirect_call_typeids {
            if previous_typeid.is_some_and(|previous| *typeid <= previous) {
                return Err(FormatError::malformed(
                    None,
                    "ANALYSIS typeids are not strictly ordered",
                ));
            }
            previous_typeid = Some(*typeid);
            output.extend_from_slice(&typeid.to_le_bytes());
        }

        write_count(self.assembly_regions.len(), "assembly-region", &mut output)?;
        let mut previous_region = None;
        for region in &self.assembly_regions {
            let key = (region.function_index, region.instruction_ordinal);
            if previous_region.is_some_and(|previous| key <= previous)
                || !(1..=3).contains(&region.architecture)
                || region.effects & !0x0f != 0
                || region.effects & (1 << 2) != 0 && region.effects & 0b0011 != 0
            {
                return Err(FormatError::malformed(
                    None,
                    "invalid or unordered ANALYSIS assembly region",
                ));
            }
            previous_region = Some(key);
            write_index(
                region.function_index,
                "assembly function index",
                &mut output,
            )?;
            write_index(
                region.instruction_ordinal,
                "assembly instruction ordinal",
                &mut output,
            )?;
            write_index(
                region.assembly_record_index,
                "assembly record index",
                &mut output,
            )?;
            output.extend_from_slice(&[region.architecture, region.effects]);
        }
        Ok(output)
    }
}

impl BinaryModule {
    pub fn analysis_summary(
        &self,
        memory_limit: u64,
    ) -> Result<BinaryAnalysisSummary, FormatError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == 14)
            .ok_or_else(|| FormatError::malformed(None, "ANALYSIS section is missing"))?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let types = self.type_table(memory_limit)?;
        let signatures = self.signature_table(memory_limit, &types)?;
        let constants = self.constant_table(memory_limit, &types)?;
        let globals = self.global_table(memory_limit, &types, &constants)?;
        let assemblies = self.assembly_table(memory_limit)?;

        let function_count = read_index(&bytes, &mut cursor, "function count")?;
        if function_count != signatures.records.len() as u64 {
            return Err(malformed(
                0,
                "ANALYSIS function count does not match SIGNATURES",
            ));
        }
        let function_capacity = capacity(function_count, &bytes, cursor, 2, "function")?;
        let mut functions = Vec::new();
        functions
            .try_reserve_exact(function_capacity)
            .map_err(|_| malformed(cursor, "ANALYSIS function allocation failed"))?;
        for _ in 0..function_capacity {
            let start = cursor;
            let trust_class = read_byte(&bytes, &mut cursor, "trust class")?;
            if trust_class > 2 {
                return Err(malformed(start, "invalid ANALYSIS trust class"));
            }
            let reason_mask = read_byte(&bytes, &mut cursor, "reason mask")?;
            functions.push(BinaryFunctionAnalysis {
                trust_class,
                reason_mask,
            });
        }

        let sandbox_count = read_index(&bytes, &mut cursor, "sandbox-region count")?;
        let sandbox_capacity = capacity(sandbox_count, &bytes, cursor, 3, "sandbox region")?;
        let mut sandbox_regions = Vec::new();
        sandbox_regions
            .try_reserve_exact(sandbox_capacity)
            .map_err(|_| malformed(cursor, "ANALYSIS sandbox-region allocation failed"))?;
        let mut used_sandbox_globals = HashSet::new();
        for _ in 0..sandbox_capacity {
            let start = cursor;
            let memory_index = read_index(&bytes, &mut cursor, "linear-memory index")?;
            let expected_memory_index = u64::try_from(sandbox_regions.len())
                .map_err(|_| malformed(start, "sandbox-region count overflows u64"))?;
            if memory_index != expected_memory_index {
                return Err(malformed(
                    start,
                    "sandbox regions are not contiguous from linear-memory index zero",
                ));
            }
            let base_global_index = read_index(&bytes, &mut cursor, "sandbox base global")?;
            let size_global_index = read_index(&bytes, &mut cursor, "sandbox size global")?;
            if base_global_index >= globals.records.len() as u64
                || size_global_index >= globals.records.len() as u64
                || base_global_index == size_global_index
            {
                return Err(malformed(start, "ANALYSIS sandbox global index is invalid"));
            }
            if !used_sandbox_globals.insert(base_global_index)
                || !used_sandbox_globals.insert(size_global_index)
            {
                return Err(malformed(
                    start,
                    "ANALYSIS sandbox globals are reused across linear memories",
                ));
            }
            sandbox_regions.push(BinarySandboxRegion {
                memory_index,
                base_global_index,
                size_global_index,
            });
        }

        let type_count = read_index(&bytes, &mut cursor, "indirect-call type count")?;
        let type_capacity = capacity(type_count, &bytes, cursor, 8, "indirect-call type")?;
        let mut indirect_call_typeids = Vec::new();
        indirect_call_typeids
            .try_reserve_exact(type_capacity)
            .map_err(|_| malformed(cursor, "ANALYSIS type-table allocation failed"))?;
        let mut previous_typeid = None;
        for _ in 0..type_capacity {
            let typeid = read_u64(&bytes, &mut cursor)?;
            if previous_typeid.is_some_and(|previous| typeid <= previous) {
                return Err(malformed(
                    cursor - 8,
                    "ANALYSIS typeids are not strictly ordered",
                ));
            }
            previous_typeid = Some(typeid);
            indirect_call_typeids.push(typeid);
        }

        let assembly_count = read_index(&bytes, &mut cursor, "assembly-region count")?;
        let assembly_capacity = capacity(assembly_count, &bytes, cursor, 5, "assembly region")?;
        let mut assembly_regions = Vec::new();
        assembly_regions
            .try_reserve_exact(assembly_capacity)
            .map_err(|_| malformed(cursor, "ANALYSIS assembly-region allocation failed"))?;
        let mut previous_region = None;
        let mut region_keys = HashSet::new();
        for _ in 0..assembly_capacity {
            let start = cursor;
            let function_index = read_index(&bytes, &mut cursor, "assembly function index")?;
            let instruction_ordinal =
                read_index(&bytes, &mut cursor, "assembly instruction ordinal")?;
            let assembly_record_index = read_index(&bytes, &mut cursor, "assembly record index")?;
            let architecture = read_byte(&bytes, &mut cursor, "assembly architecture")?;
            let effects = read_byte(&bytes, &mut cursor, "assembly effects")?;
            let key = (function_index, instruction_ordinal);
            if function_index >= signatures.records.len() as u64
                || usize::try_from(assembly_record_index)
                    .map_or(true, |index| index >= assemblies.len())
                || !(1..=3).contains(&architecture)
                || effects & !0x0f != 0
                || effects & (1 << 2) != 0 && effects & 0b0011 != 0
                || !region_keys.insert(key)
                || previous_region.is_some_and(|previous| key <= previous)
            {
                return Err(malformed(
                    start,
                    "invalid or unordered ANALYSIS assembly region",
                ));
            }
            previous_region = Some(key);
            assembly_regions.push(BinaryAssemblyRegion {
                function_index,
                instruction_ordinal,
                assembly_record_index,
                architecture,
                effects,
            });
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in ANALYSIS section"));
        }

        Ok(BinaryAnalysisSummary {
            functions,
            sandbox_regions,
            indirect_call_typeids,
            assembly_regions,
        })
    }
}

fn capacity(
    count: u64,
    bytes: &[u8],
    cursor: usize,
    minimum_record_size: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let count = usize::try_from(count).map_err(|_| {
        malformed(
            cursor,
            format!("ANALYSIS {label} count does not fit in memory"),
        )
    })?;
    if count > bytes.len().saturating_sub(cursor) / minimum_record_size {
        return Err(malformed(
            cursor,
            format!("ANALYSIS {label} records exceed section size"),
        ));
    }
    Ok(count)
}

fn read_index(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor).map_err(|error| {
        malformed(
            offset,
            format!("invalid ANALYSIS {label} ULEB128: {error:?}"),
        )
    })
}

fn read_byte(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, format!("truncated ANALYSIS {label}")))?;
    *cursor += 1;
    Ok(value)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    let end = offset
        .checked_add(8)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(offset, "truncated ANALYSIS typeid"))?;
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[offset..end]);
    *cursor = end;
    Ok(u64::from_le_bytes(value))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}

fn write_count(count: usize, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let count = u64::try_from(count)
        .ok()
        .filter(|count| *count < 1 << 32)
        .ok_or_else(|| {
            FormatError::malformed(None, format!("ANALYSIS {label} count is invalid"))
        })?;
    write_index(count, label, output)
}

fn write_index(value: u64, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output)
        .map_err(|_| FormatError::malformed(None, format!("failed to write ANALYSIS {label}")))
}
