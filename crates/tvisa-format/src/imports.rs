use std::collections::HashSet;

use tir_format::StringTable;
use tir_format::leb::{read_uleb, write_uleb};

use crate::binary::FormatError;
use crate::functions::forbidden_type_tag;
use crate::text::{TextImport, TextImportPayload};

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ImportModuleKind {
    Ridl,
    Package,
    Platform,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportPayload {
    Function { signature_index: u64, type_tag: u32 },
    Data { size: u64, alignment: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportRecord {
    pub module_kind: ImportModuleKind,
    pub module: String,
    pub abi_uuid: [u8; 16],
    pub weak: bool,
    pub symbol: String,
    pub payload: ImportPayload,
}

pub fn encode_import_records(
    records: &[ImportRecord],
    strings: &StringTable,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    write_count(records.len(), &mut bytes)?;
    let mut keys = HashSet::new();
    for record in records {
        if record.module.is_empty() || record.symbol.is_empty() {
            return Err(FormatError::malformed(
                None,
                "IMPORTS module path and symbol must be nonempty",
            ));
        }
        let module_kind = match record.module_kind {
            ImportModuleKind::Ridl => 0,
            ImportModuleKind::Package => 1,
            ImportModuleKind::Platform => 2,
        };
        let (kind, payload) = match &record.payload {
            ImportPayload::Function {
                signature_index,
                type_tag,
            } => {
                if forbidden_type_tag(*type_tag) {
                    return Err(FormatError::malformed(
                        None,
                        "imported function type tag is forbidden",
                    ));
                }
                (
                    0,
                    ImportEncodedPayload::Function(*signature_index, *type_tag),
                )
            }
            ImportPayload::Data { size, alignment } => {
                if *alignment == 0 || !alignment.is_power_of_two() {
                    return Err(FormatError::malformed(
                        None,
                        "imported data alignment must be a positive power of two",
                    ));
                }
                (1, ImportEncodedPayload::Data(*size, *alignment))
            }
        };
        let uuid_is_zero = record.abi_uuid.iter().all(|byte| *byte == 0);
        if (record.module_kind == ImportModuleKind::Ridl) == uuid_is_zero {
            return Err(FormatError::malformed(
                None,
                "IMPORTS ABI UUID does not match module kind",
            ));
        }
        if record.weak && record.module_kind != ImportModuleKind::Platform {
            return Err(FormatError::malformed(
                None,
                "only platform imports may be weak",
            ));
        }
        if !keys.insert((
            kind,
            module_kind,
            record.module.as_str(),
            record.abi_uuid,
            record.symbol.as_str(),
        )) {
            return Err(FormatError::malformed(None, "duplicate IMPORTS record"));
        }
        bytes.push(kind);
        bytes.push(module_kind);
        write_string(&record.module, strings, &mut bytes)?;
        bytes.extend_from_slice(&record.abi_uuid);
        bytes.push(u8::from(record.weak));
        write_string(&record.symbol, strings, &mut bytes)?;
        match payload {
            ImportEncodedPayload::Function(signature_index, type_tag) => {
                write_index(signature_index, &mut bytes)?;
                bytes.extend_from_slice(&type_tag.to_le_bytes());
            }
            ImportEncodedPayload::Data(size, alignment) => {
                write_index(size, &mut bytes)?;
                write_index(alignment, &mut bytes)?;
            }
        }
    }
    Ok(bytes)
}

/// Encodes TVISA text imports after resolving their signature references.
pub fn encode_text_import_records(
    imports: &[TextImport],
    strings: &StringTable,
    mut resolve_signature: impl FnMut(&str) -> Option<u64>,
) -> Result<Vec<u8>, FormatError> {
    let mut records = Vec::new();
    records
        .try_reserve_exact(imports.len())
        .map_err(|_| FormatError::malformed(None, "IMPORTS allocation failed"))?;
    for import in imports {
        let payload = match &import.payload {
            TextImportPayload::Function { signature, tag, .. } => ImportPayload::Function {
                signature_index: resolve_signature(signature).ok_or_else(|| {
                    FormatError::malformed(None, "IMPORTS signature is unresolved")
                })?,
                type_tag: *tag,
            },
            TextImportPayload::Data { size, alignment } => ImportPayload::Data {
                size: *size,
                alignment: *alignment,
            },
        };
        records.push(ImportRecord {
            module_kind: import.module.kind,
            module: import.module.name.clone(),
            abi_uuid: import.module.abi_uuid,
            weak: import.module.weak,
            symbol: import.symbol.clone(),
            payload,
        });
    }
    encode_import_records(&records, strings)
}

enum ImportEncodedPayload {
    Function(u64, u32),
    Data(u64, u64),
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

fn write_string(
    value: &str,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| FormatError::malformed(None, "IMPORTS string is absent from STRINGS"))?;
    let index = u64::try_from(index)
        .map_err(|_| FormatError::malformed(None, "IMPORTS string index overflows"))?;
    write_index(index, bytes)
}

fn write_count(count: usize, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    let count = u64::try_from(count)
        .map_err(|_| FormatError::malformed(None, "IMPORTS count overflows"))?;
    write_index(count, bytes)
}

fn write_index(index: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(index, bytes)
        .map_err(|error| FormatError::malformed(None, format!("IMPORTS encoding failed: {error}")))
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
