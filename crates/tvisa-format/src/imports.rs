use std::collections::HashSet;

use tir_format::StringTable;
use tir_format::leb::read_uleb;

use crate::binary::FormatError;
use crate::functions::forbidden_type_tag;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportCounts {
    pub functions: u64,
    pub data: u64,
    pub function_tags: Vec<u32>,
    pub function_signatures: Vec<u64>,
    pub function_import_record_indices: Vec<u64>,
    pub function_module_kinds: Vec<u8>,
    pub data_module_kinds: Vec<u8>,
    pub function_weak: Vec<bool>,
    pub data_weak: Vec<bool>,
}

pub(crate) fn decode_counts(
    bytes: &[u8],
    strings: &StringTable,
) -> Result<ImportCounts, FormatError> {
    let mut cursor = 0;
    let count = read_index(bytes, &mut cursor)?;
    let count = usize::try_from(count)
        .map_err(|_| malformed(cursor, "IMPORTS count does not fit in memory"))?;
    if count > bytes.len() {
        return Err(malformed(cursor, "IMPORTS count exceeds the section size"));
    }
    let mut imports = ImportCounts::default();
    let mut keys = HashSet::new();
    for record_index in 0..count {
        let start = cursor;
        let kind = read_byte(bytes, &mut cursor)?;
        if kind > 1 {
            return Err(malformed(start, "invalid IMPORTS kind"));
        }
        let module_kind_offset = cursor;
        let module_kind = read_byte(bytes, &mut cursor)?;
        if module_kind > 2 {
            return Err(malformed(module_kind_offset, "invalid IMPORTS module kind"));
        }
        let module_path_index = read_index(bytes, &mut cursor)?;
        let module_path = string(strings, module_path_index, start, "module path")?;
        let uuid_end = cursor
            .checked_add(16)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| malformed(cursor, "truncated IMPORTS ABI UUID"))?;
        let mut uuid = [0; 16];
        uuid.copy_from_slice(&bytes[cursor..uuid_end]);
        cursor = uuid_end;
        if (module_kind != 0) != uuid.iter().all(|byte| *byte == 0) {
            return Err(malformed(
                start,
                "IMPORTS ABI UUID does not match module kind",
            ));
        }
        let flags_offset = cursor;
        let flags = read_byte(bytes, &mut cursor)?;
        if flags & !1 != 0 || (flags != 0 && module_kind != 2) {
            return Err(malformed(flags_offset, "invalid IMPORTS flags"));
        }
        let symbol_index = read_index(bytes, &mut cursor)?;
        let symbol = string(strings, symbol_index, start, "symbol")?;
        if !keys.insert((kind, module_kind, module_path, uuid, symbol)) {
            return Err(malformed(start, "duplicate IMPORTS record"));
        }
        if kind == 0 {
            let signature_index = read_index(bytes, &mut cursor)?;
            let tag = read_u32(bytes, &mut cursor)?;
            if forbidden_type_tag(tag) {
                return Err(malformed(
                    cursor - 4,
                    "imported function type tag is forbidden",
                ));
            }
            imports.function_tags.push(tag);
            imports.function_signatures.push(signature_index);
            imports.function_module_kinds.push(module_kind);
            imports.function_weak.push(flags & 1 != 0);
            imports
                .function_import_record_indices
                .push(record_index as u64);
            imports.functions += 1;
        } else {
            let _size = read_index(bytes, &mut cursor)?;
            let alignment_offset = cursor;
            let alignment = read_index(bytes, &mut cursor)?;
            if alignment == 0 || !alignment.is_power_of_two() {
                return Err(malformed(
                    alignment_offset,
                    "imported data alignment must be a positive power of two",
                ));
            }
            imports.data += 1;
            imports.data_module_kinds.push(module_kind);
            imports.data_weak.push(flags & 1 != 0);
        }
    }
    if cursor != bytes.len() {
        return Err(malformed(cursor, "trailing bytes in IMPORTS section"));
    }
    Ok(imports)
}

fn string<'a>(
    strings: &'a StringTable,
    index: u64,
    offset: usize,
    field: &str,
) -> Result<&'a str, FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| strings.get(index))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| malformed(offset, format!("IMPORTS {field} is not a nonempty string")))
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid IMPORTS ULEB128: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, "truncated IMPORTS record"))?;
    *cursor += 1;
    Ok(value)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, FormatError> {
    let offset = *cursor;
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(offset, "truncated IMPORTS type tag"))?;
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..end]);
    *cursor = end;
    Ok(u32::from_le_bytes(value))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
