use tir_format::leb::read_uleb;

use crate::binary::{FormatError, Section, TvisaImage};

const MAX_BLOCKS_PER_FUNCTION: u64 = 1 << 24;
const MAX_VALUES_PER_FUNCTION: u64 = 1 << 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionBodyHeader {
    pub function_index: u64,
    pub offset: u64,
    pub size: u32,
    pub block_count: u64,
    pub value_count: u64,
}

impl TvisaImage {
    /// Reads body headers and checks each CODE body's owning function index.
    pub fn function_body_headers(
        &self,
        memory_limit: u64,
    ) -> Result<Vec<FunctionBodyHeader>, FormatError> {
        if self.header.function_count == 0 {
            return Ok(Vec::new());
        }
        let index = section(&self.sections, 8, "FUNCINDEX")?;
        let code = section(&self.sections, 9, "CODE")?;
        let index_bytes = index.read_uncompressed(memory_limit)?;
        let code_bytes = code.read_uncompressed(memory_limit)?;
        let imports = self.import_counts(memory_limit)?;
        let count = usize::try_from(self.header.function_count)
            .map_err(|_| malformed(20, "function count does not fit in memory"))?;
        let mut headers = Vec::new();
        headers
            .try_reserve_exact(count)
            .map_err(|_| malformed(20, "function body header allocation failed"))?;
        for ordinal in 0..count {
            let index_offset = ordinal * 16;
            let offset = read_u64(&index_bytes, index_offset)?;
            let size = read_u32(&index_bytes, index_offset + 8)?;
            let end = offset
                .checked_add(u64::from(size))
                .ok_or_else(|| malformed(index_offset, "function body range overflows"))?;
            let start = usize::try_from(offset)
                .map_err(|_| malformed(index_offset, "function body offset does not fit"))?;
            let end_usize = usize::try_from(end)
                .map_err(|_| malformed(index_offset, "function body end does not fit"))?;
            let body = code_bytes
                .get(start..end_usize)
                .ok_or_else(|| malformed(index_offset, "function body extends beyond CODE"))?;
            let mut cursor = 0;
            let function_index = read_body_index(body, &mut cursor, start)?;
            let expected_function = imports
                .functions
                .checked_add(ordinal as u64)
                .ok_or_else(|| malformed(index_offset, "function index overflows"))?;
            if function_index != expected_function {
                return Err(malformed(
                    start,
                    "CODE body function index disagrees with FUNCINDEX order",
                ));
            }
            let block_count = read_body_index(body, &mut cursor, start)?;
            if block_count == 0 {
                return Err(malformed(start + cursor, "function body has no blocks"));
            }
            if block_count >= MAX_BLOCKS_PER_FUNCTION {
                return Err(malformed(
                    start + cursor,
                    "function block count exceeds the TVISA limit",
                ));
            }
            let value_count = read_body_index(body, &mut cursor, start)?;
            if value_count >= MAX_VALUES_PER_FUNCTION {
                return Err(malformed(
                    start + cursor,
                    "function value count exceeds the TVISA limit",
                ));
            }
            let remaining = body.len().saturating_sub(cursor);
            if block_count > (remaining / 3) as u64 {
                return Err(malformed(
                    start + cursor,
                    "function block count exceeds the body size",
                ));
            }
            if value_count > remaining as u64 {
                return Err(malformed(
                    start + cursor,
                    "function value count exceeds the body size",
                ));
            }
            headers.push(FunctionBodyHeader {
                function_index,
                offset,
                size,
                block_count,
                value_count,
            });
        }
        Ok(headers)
    }
}

fn section<'a>(sections: &'a [Section], kind: u32, name: &str) -> Result<&'a Section, FormatError> {
    sections
        .iter()
        .find(|section| section.kind == kind)
        .ok_or_else(|| FormatError::malformed(None, format!("{name} is missing")))
}

fn read_body_index(bytes: &[u8], cursor: &mut usize, base: usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(base + offset, format!("invalid CODE ULEB128: {error:?}")))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FormatError> {
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(offset, "truncated FUNCINDEX u32"))?;
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..end]);
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, FormatError> {
    let end = offset
        .checked_add(8)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(offset, "truncated FUNCINDEX u64"))?;
    let mut value = [0; 8];
    value.copy_from_slice(&bytes[offset..end]);
    Ok(u64::from_le_bytes(value))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
