use std::collections::HashSet;

use crate::leb::{read_uleb, write_uleb};
use crate::{BinaryModule, FormatError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryAssemblyRecord {
    pub template_index: u64,
    pub constraints_index: u64,
}

/// Encodes ASM records using existing STRINGS indices.
pub fn encode_assembly_table(
    records: &[BinaryAssemblyRecord],
    strings: &crate::StringTable,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    write_uleb(
        u64::try_from(records.len()).map_err(|_| malformed(0, "ASM count overflows ULEB128"))?,
        &mut bytes,
    )
    .map_err(|error| malformed(0, format!("ASM encoding failed: {error}")))?;
    let mut pairs = HashSet::new();
    for record in records {
        let template = strings
            .get(
                usize::try_from(record.template_index)
                    .map_err(|_| malformed(0, "ASM template index overflows memory"))?,
            )
            .ok_or_else(|| malformed(0, "ASM template index is invalid"))?;
        let constraints = strings
            .get(
                usize::try_from(record.constraints_index)
                    .map_err(|_| malformed(0, "ASM constraint index overflows memory"))?,
            )
            .ok_or_else(|| malformed(0, "ASM constraint index is invalid"))?;
        if !pairs.insert((template, constraints)) {
            return Err(malformed(0, "duplicate ASM template/constraint pair"));
        }
        write_uleb(record.template_index, &mut bytes)
            .map_err(|error| malformed(0, format!("ASM encoding failed: {error}")))?;
        write_uleb(record.constraints_index, &mut bytes)
            .map_err(|error| malformed(0, format!("ASM encoding failed: {error}")))?;
    }
    Ok(bytes)
}

impl BinaryModule {
    pub fn assembly_table(
        &self,
        memory_limit: u64,
    ) -> Result<Vec<BinaryAssemblyRecord>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 16) else {
            return Ok(Vec::new());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let count = read_index(&bytes, &mut cursor, "ASM count")?;
        if count > bytes.len().saturating_sub(cursor) as u64 / 2 {
            return Err(malformed(cursor, "ASM record count exceeds section size"));
        }
        let capacity = usize::try_from(count)
            .map_err(|_| malformed(cursor, "ASM record count does not fit in memory"))?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(capacity)
            .map_err(|_| malformed(cursor, "ASM allocation failed"))?;
        let mut pairs = HashSet::new();
        pairs
            .try_reserve(capacity)
            .map_err(|_| malformed(cursor, "ASM uniqueness allocation failed"))?;
        for _ in 0..capacity {
            let start = cursor;
            let template_index = read_index(&bytes, &mut cursor, "template index")?;
            let constraints_index = read_index(&bytes, &mut cursor, "constraint index")?;
            let template = usize::try_from(template_index)
                .ok()
                .and_then(|index| strings.get(index))
                .ok_or_else(|| malformed(start, "ASM template index is invalid"))?;
            let constraints = usize::try_from(constraints_index)
                .ok()
                .and_then(|index| strings.get(index))
                .ok_or_else(|| malformed(start, "ASM constraint index is invalid"))?;
            if !pairs.insert((template, constraints)) {
                return Err(malformed(start, "duplicate ASM template/constraint pair"));
            }
            records.push(BinaryAssemblyRecord {
                template_index,
                constraints_index,
            });
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in ASM section"));
        }
        Ok(records)
    }
}

fn read_index(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid ASM {label} ULEB128: {error:?}")))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
