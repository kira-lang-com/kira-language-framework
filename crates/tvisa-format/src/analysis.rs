use std::collections::HashSet;

use tir_format::leb::{read_uleb, write_uleb};

use crate::binary::{FormatError, TvisaImage};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionAnalysisRecord {
    pub trust_class: u8,
    pub reason_mask: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SandboxRegionSummary {
    pub memory_index: u64,
    pub base_data_index: u64,
    pub size_data_index: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IndirectCallType {
    pub signature_index: u64,
    pub type_tag: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssemblyRegionSummary {
    pub function_index: u64,
    pub instruction_ordinal: u64,
    pub assembly_record_index: u64,
    pub architecture: u8,
    pub effects: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalysisSummary {
    pub functions: Vec<FunctionAnalysisRecord>,
    pub sandbox_regions: Vec<SandboxRegionSummary>,
    pub indirect_call_types: Vec<IndirectCallType>,
    pub assembly_regions: Vec<AssemblyRegionSummary>,
}

impl AnalysisSummary {
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        let mut output = Vec::new();
        write_count(self.functions.len(), "ANALYSIS function", &mut output)?;
        for function in &self.functions {
            if function.trust_class > 2 {
                return Err(encode_error("invalid ANALYSIS trust class"));
            }
            output.extend_from_slice(&[function.trust_class, function.reason_mask]);
        }

        write_count(
            self.sandbox_regions.len(),
            "ANALYSIS sandbox-region",
            &mut output,
        )?;
        let mut used_data = HashSet::new();
        for (index, region) in self.sandbox_regions.iter().enumerate() {
            if u64::try_from(index).ok() != Some(region.memory_index)
                || region.base_data_index == region.size_data_index
                || !used_data.insert(region.base_data_index)
                || !used_data.insert(region.size_data_index)
            {
                return Err(encode_error(
                    "invalid or reused ANALYSIS sandbox-region index",
                ));
            }
            for value in [
                region.memory_index,
                region.base_data_index,
                region.size_data_index,
            ] {
                write_index(value, "ANALYSIS sandbox-region index", &mut output)?;
            }
        }

        write_count(
            self.indirect_call_types.len(),
            "ANALYSIS indirect-call type",
            &mut output,
        )?;
        let mut previous_type = None;
        for indirect_call_type in &self.indirect_call_types {
            if previous_type.is_some_and(|previous| *indirect_call_type <= previous) {
                return Err(encode_error(
                    "ANALYSIS indirect-call types are not strictly ordered",
                ));
            }
            previous_type = Some(*indirect_call_type);
            write_index(
                indirect_call_type.signature_index,
                "ANALYSIS indirect-call signature",
                &mut output,
            )?;
            output.extend_from_slice(&indirect_call_type.type_tag.to_le_bytes());
        }

        write_count(
            self.assembly_regions.len(),
            "ANALYSIS assembly-region",
            &mut output,
        )?;
        let mut previous_assembly = None;
        for region in &self.assembly_regions {
            let key = (region.function_index, region.instruction_ordinal);
            if previous_assembly.is_some_and(|previous| key <= previous)
                || !(1..=3).contains(&region.architecture)
                || region.effects & !0x0f != 0
                || region.effects & (1 << 2) != 0 && region.effects & 0b0011 != 0
            {
                return Err(encode_error(
                    "invalid or unordered ANALYSIS assembly region",
                ));
            }
            previous_assembly = Some(key);
            write_index(
                region.function_index,
                "ANALYSIS assembly function",
                &mut output,
            )?;
            write_index(
                region.instruction_ordinal,
                "ANALYSIS assembly instruction ordinal",
                &mut output,
            )?;
            write_index(
                region.assembly_record_index,
                "ANALYSIS assembly record index",
                &mut output,
            )?;
            output.extend_from_slice(&[region.architecture, region.effects]);
        }
        Ok(output)
    }
}

impl TvisaImage {
    pub fn analysis_summary(&self, memory_limit: u64) -> Result<AnalysisSummary, FormatError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == 20)
            .ok_or_else(|| FormatError::malformed(None, "ANALYSIS section is missing"))?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;

        let function_count = read_count(&bytes, &mut cursor, "ANALYSIS function")?;
        if function_count != self.header.function_count as u64 {
            return Err(malformed(
                0,
                "ANALYSIS function count does not match the TVISA header",
            ));
        }
        let function_capacity = usize::try_from(function_count)
            .map_err(|_| malformed(cursor, "ANALYSIS function count does not fit in memory"))?;
        if function_capacity > bytes.len().saturating_sub(cursor) / 2 {
            return Err(malformed(
                cursor,
                "ANALYSIS function records exceed section size",
            ));
        }
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
            functions.push(FunctionAnalysisRecord {
                trust_class,
                reason_mask,
            });
        }

        let imports = self.import_counts(memory_limit)?;
        let data = self.data_section(memory_limit)?;
        let total_data = imports
            .data
            .checked_add(data.objects.len() as u64)
            .ok_or_else(|| malformed(cursor, "ANALYSIS DATA index space overflows"))?;

        let sandbox_count = read_count(&bytes, &mut cursor, "sandbox-region")?;
        let sandbox_capacity = bounded_capacity(sandbox_count, &bytes, cursor, 3, "sandbox")?;
        let mut sandbox_regions = Vec::new();
        sandbox_regions
            .try_reserve_exact(sandbox_capacity)
            .map_err(|_| malformed(cursor, "ANALYSIS sandbox-region allocation failed"))?;
        let mut used_sandbox_data = HashSet::new();
        for _ in 0..sandbox_capacity {
            let start = cursor;
            let memory_index = read_index(&bytes, &mut cursor, "linear-memory index")?;
            let expected_memory_index = u64::try_from(sandbox_regions.len())
                .map_err(|_| malformed(start, "sandbox-region count overflows u64"))?;
            if memory_index != expected_memory_index {
                return Err(malformed(
                    start,
                    "ANALYSIS sandbox regions are not contiguous from linear-memory index zero",
                ));
            }
            let base_data_index = read_index(&bytes, &mut cursor, "sandbox base DATA index")?;
            let size_data_index = read_index(&bytes, &mut cursor, "sandbox size DATA index")?;
            if base_data_index >= total_data
                || size_data_index >= total_data
                || base_data_index < imports.data
                || size_data_index < imports.data
                || base_data_index == size_data_index
            {
                return Err(malformed(
                    start,
                    "ANALYSIS sandbox DATA index must name a distinct defined object",
                ));
            }
            if !used_sandbox_data.insert(base_data_index)
                || !used_sandbox_data.insert(size_data_index)
            {
                return Err(malformed(
                    start,
                    "ANALYSIS sandbox DATA objects are reused across linear memories",
                ));
            }
            sandbox_regions.push(SandboxRegionSummary {
                memory_index,
                base_data_index,
                size_data_index,
            });
        }

        let types = self.type_section(memory_limit)?;
        let type_count = read_count(&bytes, &mut cursor, "indirect-call type")?;
        let type_capacity = bounded_capacity(type_count, &bytes, cursor, 5, "indirect-call type")?;
        let mut indirect_call_types = Vec::new();
        indirect_call_types
            .try_reserve_exact(type_capacity)
            .map_err(|_| malformed(cursor, "ANALYSIS type-table allocation failed"))?;
        let mut previous_type = None;
        for _ in 0..type_capacity {
            let start = cursor;
            let signature_index = read_index(&bytes, &mut cursor, "indirect-call signature")?;
            if signature_index >= types.signatures.len() as u64 {
                return Err(malformed(start, "ANALYSIS signature index is invalid"));
            }
            let type_tag = read_u32(&bytes, &mut cursor)?;
            let current = IndirectCallType {
                signature_index,
                type_tag,
            };
            if previous_type.is_some_and(|previous| current <= previous) {
                return Err(malformed(
                    start,
                    "ANALYSIS indirect-call types are not strictly ordered",
                ));
            }
            previous_type = Some(current);
            indirect_call_types.push(current);
        }

        let assemblies = self.assembly_records(memory_limit)?;
        let assembly_count = read_count(&bytes, &mut cursor, "assembly-region")?;
        let assembly_capacity =
            bounded_capacity(assembly_count, &bytes, cursor, 5, "assembly-region")?;
        let mut assembly_regions = Vec::new();
        assembly_regions
            .try_reserve_exact(assembly_capacity)
            .map_err(|_| malformed(cursor, "ANALYSIS assembly-region allocation failed"))?;
        let mut previous_assembly = None;
        let function_end = imports
            .functions
            .checked_add(u64::from(self.header.function_count))
            .ok_or_else(|| malformed(cursor, "ANALYSIS function index space overflows"))?;
        let mut unique_regions = HashSet::new();
        for _ in 0..assembly_capacity {
            let start = cursor;
            let function_index = read_index(&bytes, &mut cursor, "assembly function index")?;
            let instruction_ordinal =
                read_index(&bytes, &mut cursor, "assembly instruction ordinal")?;
            let assembly_record_index = read_index(&bytes, &mut cursor, "assembly record index")?;
            let architecture = read_byte(&bytes, &mut cursor, "assembly architecture")?;
            let effects = read_byte(&bytes, &mut cursor, "assembly effects")?;
            let key = (function_index, instruction_ordinal);
            if function_index < imports.functions
                || function_index >= function_end
                || usize::try_from(assembly_record_index)
                    .map_or(true, |index| index >= assemblies.len())
                || !(1..=3).contains(&architecture)
                || effects & !0x0f != 0
                || effects & (1 << 2) != 0 && effects & 0b0011 != 0
                || !unique_regions.insert(key)
                || previous_assembly.is_some_and(|previous| key <= previous)
            {
                return Err(malformed(
                    start,
                    "invalid or unordered ANALYSIS assembly region",
                ));
            }
            previous_assembly = Some(key);
            assembly_regions.push(AssemblyRegionSummary {
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

        Ok(AnalysisSummary {
            functions,
            sandbox_regions,
            indirect_call_types,
            assembly_regions,
        })
    }
}

fn bounded_capacity(
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

fn read_count(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, FormatError> {
    read_index(bytes, cursor, label)
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

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, FormatError> {
    let offset = *cursor;
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(offset, "truncated ANALYSIS type tag"))?;
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..end]);
    *cursor = end;
    Ok(u32::from_le_bytes(value))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}

fn write_count(count: usize, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let count = u64::try_from(count)
        .ok()
        .filter(|count| *count < 1 << 32)
        .ok_or_else(|| encode_error(format!("{label} count is invalid")))?;
    write_index(count, label, output)
}

fn write_index(value: u64, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output).map_err(|_| encode_error(format!("failed to write {label}")))
}

fn encode_error(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
