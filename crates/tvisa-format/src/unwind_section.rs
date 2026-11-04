use tir_format::leb::{read_uleb, write_uleb};

use crate::binary::{FormatError, TvisaImage};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnwindSection {
    pub functions: Vec<UnwindFunctionRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnwindFunctionRecord {
    pub function_index: u64,
    pub terminate_kind: u8,
    pub type_ids: Vec<u64>,
    pub filters: Vec<u64>,
}

impl UnwindSection {
    /// Encode UNWIND records in their required function-index order.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        if self.functions.is_empty() {
            return Err(malformed(0, "UNWIND must be absent when it has no records"));
        }

        let mut bytes = Vec::new();
        write_index(
            u64::try_from(self.functions.len())
                .map_err(|_| malformed(0, "UNWIND function count overflows"))?,
            &mut bytes,
        )?;
        for function in &self.functions {
            write_index(function.function_index, &mut bytes)?;
            bytes.push(function.terminate_kind);
            write_index(
                u64::try_from(function.type_ids.len())
                    .map_err(|_| malformed(bytes.len(), "UNWIND type-id count overflows"))?,
                &mut bytes,
            )?;
            for type_id in &function.type_ids {
                bytes.extend_from_slice(&type_id.to_le_bytes());
            }
            write_index(
                u64::try_from(function.filters.len())
                    .map_err(|_| malformed(bytes.len(), "UNWIND filter count overflows"))?,
                &mut bytes,
            )?;
            for filter in &function.filters {
                write_index(*filter, &mut bytes)?;
            }
        }

        let decoded = decode(&bytes)?;
        if decoded != *self {
            return Err(malformed(
                bytes.len(),
                "UNWIND records are inconsistent with their binary encoding",
            ));
        }
        Ok(bytes)
    }
}

impl TvisaImage {
    pub fn unwind_section(&self, memory_limit: u64) -> Result<UnwindSection, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 12) else {
            return Ok(UnwindSection::default());
        };
        decode(&section.read_uncompressed(memory_limit)?)
    }
}

fn decode(bytes: &[u8]) -> Result<UnwindSection, FormatError> {
    let mut cursor = 0;
    let count = read_index(bytes, &mut cursor, "UNWIND function count")?;
    if count == 0 {
        return Err(malformed(0, "UNWIND must be absent when it has no records"));
    }
    if count > bytes.len() as u64 / 4 {
        return Err(malformed(
            cursor,
            "UNWIND function count exceeds the section size",
        ));
    }
    let capacity = usize::try_from(count)
        .map_err(|_| malformed(cursor, "UNWIND function count does not fit in memory"))?;
    let mut functions = Vec::new();
    functions
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(cursor, "UNWIND function allocation failed"))?;
    let mut previous_function = None;
    for _ in 0..count {
        let start = cursor;
        let function_index = read_index(bytes, &mut cursor, "UNWIND function index")?;
        if previous_function.is_some_and(|previous| function_index <= previous) {
            return Err(malformed(
                start,
                "UNWIND function indexes are not increasing",
            ));
        }
        previous_function = Some(function_index);
        let terminate_kind = read_byte(bytes, &mut cursor, "UNWIND terminate kind")?;
        if terminate_kind > 2 {
            return Err(malformed(start, "invalid UNWIND terminate kind"));
        }
        let type_count = read_index(bytes, &mut cursor, "UNWIND type-id count")?;
        let type_bytes = type_count
            .checked_mul(8)
            .ok_or_else(|| malformed(cursor, "UNWIND type-id count overflows"))?;
        let type_bytes = usize::try_from(type_bytes)
            .map_err(|_| malformed(cursor, "UNWIND type-id table does not fit in memory"))?;
        let type_end = cursor
            .checked_add(type_bytes)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| malformed(cursor, "truncated UNWIND type-id table"))?;
        let mut type_ids = Vec::new();
        type_ids
            .try_reserve_exact(
                usize::try_from(type_count).map_err(|_| {
                    malformed(cursor, "UNWIND type-id count does not fit in memory")
                })?,
            )
            .map_err(|_| malformed(cursor, "UNWIND type-id allocation failed"))?;
        while cursor < type_end {
            let encoded: [u8; 8] = bytes[cursor..cursor + 8]
                .try_into()
                .map_err(|_| malformed(cursor, "truncated UNWIND type ID"))?;
            let value = u64::from_le_bytes(encoded);
            type_ids.push(value);
            cursor += 8;
        }
        let filter_count = read_index(bytes, &mut cursor, "UNWIND filter count")?;
        if filter_count > bytes.len().saturating_sub(cursor) as u64 {
            return Err(malformed(
                cursor,
                "UNWIND filter count exceeds the section size",
            ));
        }
        let filter_capacity = usize::try_from(filter_count)
            .map_err(|_| malformed(cursor, "UNWIND filter count does not fit in memory"))?;
        let mut filters = Vec::new();
        filters
            .try_reserve_exact(filter_capacity)
            .map_err(|_| malformed(cursor, "UNWIND filter allocation failed"))?;
        for _ in 0..filter_count {
            let filter = read_index(bytes, &mut cursor, "UNWIND filter function index")?;
            filters.push(filter);
        }
        functions.push(UnwindFunctionRecord {
            function_index,
            terminate_kind,
            type_ids,
            filters,
        });
    }
    if cursor != bytes.len() {
        return Err(malformed(cursor, "trailing bytes in UNWIND section"));
    }
    Ok(UnwindSection { functions })
}

fn read_index(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid {field} ULEB128: {error:?}")))
}

fn write_index(value: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, bytes).map_err(|error| malformed(bytes.len(), error.to_string()))
}

fn read_byte(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u8, FormatError> {
    let value = bytes
        .get(*cursor)
        .copied()
        .ok_or_else(|| malformed(*cursor, format!("truncated {field}")))?;
    *cursor += 1;
    Ok(value)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
