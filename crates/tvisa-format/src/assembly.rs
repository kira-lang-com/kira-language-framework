use std::collections::HashSet;

use crate::binary::{FormatError, TvisaImage};
use tir_format::leb::read_uleb;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssemblyRecord {
    pub template_index: u64,
    pub constraints_index: u64,
}

impl TvisaImage {
    pub fn assembly_records(&self, memory_limit: u64) -> Result<Vec<AssemblyRecord>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 14) else {
            return Ok(Vec::new());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let count = read_index(&bytes, &mut cursor)?;
        let remaining = bytes.len().saturating_sub(cursor);
        if count > (remaining / 2) as u64 {
            return Err(malformed(
                cursor,
                "ASM record count exceeds the section data",
            ));
        }
        let count = usize::try_from(count)
            .map_err(|_| malformed(cursor, "ASM record count does not fit in memory"))?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(count)
            .map_err(|_| malformed(cursor, "ASM record allocation failed"))?;
        let mut unique = HashSet::new();
        unique
            .try_reserve(count)
            .map_err(|_| malformed(cursor, "ASM uniqueness allocation failed"))?;

        for _ in 0..count {
            let template_offset = cursor;
            let template_index = read_index(&bytes, &mut cursor)?;
            let constraints_offset = cursor;
            let constraints_index = read_index(&bytes, &mut cursor)?;
            let template_slot = usize::try_from(template_index)
                .map_err(|_| malformed(template_offset, "ASM template index does not fit"))?;
            let constraints_slot = usize::try_from(constraints_index)
                .map_err(|_| malformed(constraints_offset, "ASM constraints index does not fit"))?;
            let template = strings
                .get(template_slot)
                .ok_or_else(|| malformed(template_offset, "ASM template index is invalid"))?;
            let constraints = strings
                .get(constraints_slot)
                .ok_or_else(|| malformed(constraints_offset, "ASM constraints index is invalid"))?;
            if !unique.insert((template, constraints)) {
                return Err(malformed(
                    template_offset,
                    "duplicate ASM template/constraints pair",
                ));
            }
            records.push(AssemblyRecord {
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

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid ASM ULEB128: {error:?}")))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
