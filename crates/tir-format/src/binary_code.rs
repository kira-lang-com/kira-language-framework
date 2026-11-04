use crate::binary_code_blocks::decode_function_body;
use crate::leb::{LebError, read_uleb};
use crate::types::{BinaryCodeTable, BinarySignatureTable};
use crate::{BinaryModule, FormatError};

mod encode;

impl BinaryCodeTable {
    pub fn encode(
        &self,
        signatures: &BinarySignatureTable,
        types: &crate::types::BinaryTypeTable,
        constants: &crate::types::BinaryConstantTable,
        strings: &crate::strings::StringTable,
        assembly_count: u64,
    ) -> Result<Vec<u8>, FormatError> {
        encode::encode_code_table(self, signatures, types, constants, strings, assembly_count)
    }
}

const MAX_FUNCTION_COUNT: u64 = 1 << 32;

impl BinaryModule {
    /// Decodes CODE function records, blocks, and currently supported opcodes.
    /// Blocks containing an unsupported opcode retain their raw instruction bytes.
    pub fn code_table(
        &self,
        memory_limit: u64,
        signatures: &BinarySignatureTable,
    ) -> Result<BinaryCodeTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 8) else {
            return Ok(BinaryCodeTable::default());
        };
        let bytes = section.read_uncompressed(memory_limit)?;
        let types = self.type_table(memory_limit)?;
        let constants = self.constant_table(memory_limit, &types)?;
        let strings = self.string_table(memory_limit)?;
        let assembly_count = u64::try_from(self.assembly_table(memory_limit)?.len())
            .map_err(|_| FormatError::malformed(None, "ASM record count overflows"))?;
        let mut cursor = 0;
        let count_offset = cursor;
        let count = read_count(&bytes, &mut cursor, 3, "CODE function")?;
        if count as u64 >= MAX_FUNCTION_COUNT {
            return Err(malformed(
                count_offset,
                "CODE function count must be less than 2^32",
            ));
        }
        let mut functions = reserve(count, &bytes, cursor)?;
        let mut previous_function = None;
        for _ in 0..count {
            let function_offset = cursor;
            let function_index = read_index(&bytes, &mut cursor)?;
            if usize::try_from(function_index)
                .map_or(true, |index| index >= signatures.records.len())
            {
                return Err(malformed(
                    function_offset,
                    "CODE function index is out of range",
                ));
            }
            if previous_function.is_some_and(|previous| function_index <= previous) {
                return Err(malformed(
                    function_offset,
                    "CODE functions are not in SIGNATURES declaration order",
                ));
            }
            previous_function = Some(function_index);

            let size_offset = cursor;
            let body_byte_size = read_index(&bytes, &mut cursor)?;
            let body_size = usize::try_from(body_byte_size)
                .map_err(|_| malformed(size_offset, "CODE body size overflows memory"))?;
            let body_end = cursor
                .checked_add(body_size)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| malformed(size_offset, "CODE body exceeds the section"))?;
            let body = decode_function_body(
                &bytes[cursor..body_end],
                cursor,
                function_index,
                body_byte_size,
                signatures,
                &types,
                &constants,
                &strings,
                assembly_count,
            )?;
            cursor = body_end;
            functions.push(body);
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in CODE section"));
        }
        Ok(BinaryCodeTable { functions })
    }
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_record_bytes: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, format!("{label} count overflows memory")))?;
    if count as u64 >= MAX_FUNCTION_COUNT {
        return Err(malformed(
            offset,
            format!("{label} count must be less than 2^32"),
        ));
    }
    if count > bytes.len().saturating_sub(*cursor) / minimum_record_bytes {
        return Err(malformed(
            *cursor,
            format!("{label} count exceeds section size"),
        ));
    }
    Ok(count)
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, "CODE allocation failed"))?;
    if count > bytes.len() {
        return Err(malformed(offset, "CODE count exceeds section size"));
    }
    Ok(values)
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    read_uleb(bytes, cursor).map_err(|error| map_leb_error(error, *cursor))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}

fn map_leb_error(error: LebError, offset: usize) -> FormatError {
    malformed(offset, format!("invalid CODE ULEB: {error:?}"))
}
