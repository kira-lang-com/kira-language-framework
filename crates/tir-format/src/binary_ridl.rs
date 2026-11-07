use std::collections::HashSet;

use crate::binary_generics::BinaryGenericTable;
use crate::leb::{read_uleb, write_uleb};
use crate::types::BinaryCodeTable;
use crate::types::{BinaryGlobalTable, BinarySignatureTable, BinaryTypeTable};
use crate::{BinaryModule, FormatError, StringTable};
use tir_core::ContentHash;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryRidlExportKind {
    Function,
    Method,
    Descriptor,
    Builder,
    DescriptorSize,
    Witness,
    Instance,
    Global,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryRidlExport {
    pub symbol: String,
    pub kind: BinaryRidlExportKind,
    pub entity_index: u64,
    pub abi_uuid: [u8; 16],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryAbiStamp {
    pub symbol: String,
    pub uuid: [u8; 16],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryInlinableBody {
    pub function_index: u64,
    pub hash: [u8; 32],
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryRidlTable {
    pub exports: Vec<BinaryRidlExport>,
    pub abi_stamps: Vec<BinaryAbiStamp>,
    pub inlinable_bodies: Vec<BinaryInlinableBody>,
    pub canonical_hash: Option<[u8; 32]>,
}

impl BinaryRidlTable {
    pub fn intern_strings(
        &self,
        strings: &mut crate::StringTableBuilder,
    ) -> Result<(), FormatError> {
        for export in &self.exports {
            strings
                .intern(&export.symbol)
                .map_err(|error| malformed(0, error.message))?;
        }
        for stamp in &self.abi_stamps {
            strings
                .intern(&stamp.symbol)
                .map_err(|error| malformed(0, error.message))?;
        }
        Ok(())
    }

    pub fn encode(
        &self,
        strings: &StringTable,
        types: &BinaryTypeTable,
        signatures: &BinarySignatureTable,
        generics: &BinaryGenericTable,
        globals: &BinaryGlobalTable,
        code: &BinaryCodeTable,
    ) -> Result<Option<Vec<u8>>, FormatError> {
        if self.exports.is_empty() && self.abi_stamps.is_empty() && self.inlinable_bodies.is_empty()
        {
            if self.canonical_hash.is_some() {
                return Err(malformed(
                    0,
                    "empty RIDL table cannot carry a canonical hash",
                ));
            }
            if signatures
                .records
                .iter()
                .any(|signature| signature.inlinable)
            {
                return Err(malformed(0, "RIDL inlinable function body is missing"));
            }
            return Ok(None);
        }
        let hash = self
            .canonical_hash
            .ok_or_else(|| malformed(0, "RIDL canonical hash is missing"))?;
        let mut output = Vec::new();
        write_index(&mut output, self.exports.len() as u64)?;
        for export in &self.exports {
            write_string_index(&mut output, strings, &export.symbol)?;
            output.push(match export.kind {
                BinaryRidlExportKind::Function => 0,
                BinaryRidlExportKind::Method => 1,
                BinaryRidlExportKind::Descriptor => 2,
                BinaryRidlExportKind::Builder => 3,
                BinaryRidlExportKind::DescriptorSize => 4,
                BinaryRidlExportKind::Witness => 5,
                BinaryRidlExportKind::Instance => 6,
                BinaryRidlExportKind::Global => 7,
            });
            write_index(&mut output, export.entity_index)?;
            output.extend_from_slice(&export.abi_uuid);
        }
        write_index(&mut output, self.abi_stamps.len() as u64)?;
        for stamp in &self.abi_stamps {
            write_string_index(&mut output, strings, &stamp.symbol)?;
            output.extend_from_slice(&stamp.uuid);
        }
        write_index(&mut output, self.inlinable_bodies.len() as u64)?;
        for body in &self.inlinable_bodies {
            write_index(&mut output, body.function_index)?;
            output.extend_from_slice(&body.hash);
        }
        output.extend_from_slice(&hash);
        let decoded = decode(&output, strings, types, signatures, generics, globals, code)?;
        if decoded != *self {
            return Err(malformed(
                0,
                "RIDL records disagree with canonical encoding",
            ));
        }
        Ok(Some(output))
    }
}

fn write_index(bytes: &mut Vec<u8>, index: u64) -> Result<(), FormatError> {
    write_uleb(index, bytes).map_err(|error| malformed(bytes.len(), error.to_string()))
}

fn write_string_index(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    value: &str,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| malformed(bytes.len(), "RIDL symbol was not interned in STRINGS"))?;
    write_index(bytes, index)
}

impl BinaryModule {
    pub fn ridl_table(&self, memory_limit: u64) -> Result<BinaryRidlTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 12) else {
            return Ok(BinaryRidlTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let types = self.type_table(memory_limit)?;
        let signatures = self.signature_table(memory_limit, &types)?;
        let generics = self.generics_table(memory_limit, &types, &signatures)?;
        let constants = self.constant_table(memory_limit, &types)?;
        let globals = self.global_table(memory_limit, &types, &constants)?;
        let code = self.code_table(memory_limit, &signatures)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        decode(
            &bytes,
            &strings,
            &types,
            &signatures,
            &generics,
            &globals,
            &code,
        )
    }
}

fn decode(
    bytes: &[u8],
    strings: &StringTable,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
    generics: &BinaryGenericTable,
    globals: &BinaryGlobalTable,
    code: &BinaryCodeTable,
) -> Result<BinaryRidlTable, FormatError> {
    let mut cursor = 0;
    let export_count = read_count(bytes, &mut cursor, "export")?;
    let mut exports = Vec::with_capacity(export_count);
    let mut previous_symbol: Option<String> = None;
    let mut symbols = HashSet::new();
    for _ in 0..export_count {
        let start = cursor;
        let symbol = read_string(bytes, &mut cursor, strings, "export symbol")?;
        if symbol.is_empty()
            || previous_symbol
                .as_ref()
                .is_some_and(|previous| previous.as_bytes() >= symbol.as_bytes())
            || !symbols.insert(symbol.clone())
        {
            return Err(malformed(
                start,
                "RIDL exports are not strictly ordered by symbol",
            ));
        }
        previous_symbol = Some(symbol.clone());
        let kind = match read_byte(bytes, &mut cursor, "export kind")? {
            0 => BinaryRidlExportKind::Function,
            1 => BinaryRidlExportKind::Method,
            2 => BinaryRidlExportKind::Descriptor,
            3 => BinaryRidlExportKind::Builder,
            4 => BinaryRidlExportKind::DescriptorSize,
            5 => BinaryRidlExportKind::Witness,
            6 => BinaryRidlExportKind::Instance,
            7 => BinaryRidlExportKind::Global,
            _ => return Err(malformed(cursor - 1, "invalid RIDL export kind")),
        };
        let entity_index = read_index(bytes, &mut cursor, "export entity index")?;
        let limit = match kind {
            BinaryRidlExportKind::Function
            | BinaryRidlExportKind::Method
            | BinaryRidlExportKind::Instance => signatures.records.len(),
            BinaryRidlExportKind::Descriptor
            | BinaryRidlExportKind::Builder
            | BinaryRidlExportKind::DescriptorSize => types.declarations.len(),
            BinaryRidlExportKind::Witness => generics.conformances.len(),
            BinaryRidlExportKind::Global => globals.records.len(),
        };
        if usize::try_from(entity_index).map_or(true, |index| index >= limit) {
            return Err(malformed(start, "RIDL export entity index is out of range"));
        }
        let abi_uuid = read_array::<16>(bytes, &mut cursor, "export ABI UUID")?;
        if abi_uuid.iter().all(|byte| *byte == 0) {
            return Err(malformed(start, "RIDL export ABI UUID is zero"));
        }
        exports.push(BinaryRidlExport {
            symbol,
            kind,
            entity_index,
            abi_uuid,
        });
    }

    let stamp_count = read_count(bytes, &mut cursor, "ABI stamp")?;
    let mut abi_stamps = Vec::with_capacity(stamp_count);
    let mut previous_stamp: Option<String> = None;
    for _ in 0..stamp_count {
        let start = cursor;
        let symbol = read_string(bytes, &mut cursor, strings, "ABI stamp symbol")?;
        if symbol.is_empty()
            || previous_stamp
                .as_ref()
                .is_some_and(|previous| previous.as_bytes() >= symbol.as_bytes())
        {
            return Err(malformed(
                start,
                "RIDL ABI stamps are not strictly ordered by symbol",
            ));
        }
        previous_stamp = Some(symbol.clone());
        let uuid = read_array::<16>(bytes, &mut cursor, "ABI stamp UUID")?;
        if uuid.iter().all(|byte| *byte == 0) {
            return Err(malformed(start, "RIDL ABI stamp UUID is zero"));
        }
        abi_stamps.push(BinaryAbiStamp { symbol, uuid });
    }
    for stamp in &abi_stamps {
        let export = exports
            .binary_search_by(|export| export.symbol.as_bytes().cmp(stamp.symbol.as_bytes()))
            .ok()
            .and_then(|index| exports.get(index))
            .ok_or_else(|| malformed(cursor, "RIDL ABI stamp has no matching export"))?;
        if export.abi_uuid != stamp.uuid {
            return Err(malformed(
                cursor,
                "RIDL ABI stamp disagrees with its export UUID",
            ));
        }
    }

    let body_count = read_count(bytes, &mut cursor, "inlinable body")?;
    let mut inlinable_bodies = Vec::with_capacity(body_count);
    let mut previous_function = None;
    for _ in 0..body_count {
        let start = cursor;
        let function_index = read_index(bytes, &mut cursor, "inlinable function index")?;
        if previous_function.is_some_and(|previous| function_index <= previous) {
            return Err(malformed(
                start,
                "RIDL inlinable bodies are not strictly ordered",
            ));
        }
        previous_function = Some(function_index);
        let Some(signature) = usize::try_from(function_index)
            .ok()
            .and_then(|index| signatures.records.get(index))
        else {
            return Err(malformed(
                start,
                "RIDL inlinable function index is out of range",
            ));
        };
        if !signature.inlinable {
            return Err(malformed(
                start,
                "RIDL body record references a function not marked inlinable",
            ));
        }
        if signature.linkage != crate::types::BinaryLinkage::Exported {
            return Err(malformed(
                start,
                "RIDL inlinable body belongs to a non-exported function",
            ));
        }
        let hash = read_array::<32>(bytes, &mut cursor, "inlinable body hash")?;
        let Some(body) = code
            .functions
            .iter()
            .find(|body| body.function_index == function_index)
        else {
            return Err(malformed(start, "RIDL inlinable body has no CODE function"));
        };
        let canonical = crate::binary_code_encode::encode_function_body(body)?;
        if ContentHash::of(&canonical).as_bytes() != &hash {
            return Err(malformed(
                start,
                "RIDL inlinable body hash does not match canonical CODE bytes",
            ));
        }
        inlinable_bodies.push(BinaryInlinableBody {
            function_index,
            hash,
        });
    }
    let canonical_hash = read_array::<32>(bytes, &mut cursor, "canonical RIDL hash")?;
    if exports.is_empty() && abi_stamps.is_empty() && inlinable_bodies.is_empty() {
        return Err(malformed(0, "empty RIDL section must be omitted"));
    }
    for (index, signature) in signatures.records.iter().enumerate() {
        if signature.inlinable
            && !inlinable_bodies
                .iter()
                .any(|body| body.function_index == index as u64)
        {
            return Err(malformed(
                0,
                "RIDL section omits a declared inlinable function body",
            ));
        }
    }
    if cursor != bytes.len() {
        return Err(malformed(cursor, "trailing bytes in RIDL section"));
    }
    Ok(BinaryRidlTable {
        exports,
        abi_stamps,
        inlinable_bodies,
        canonical_hash: Some(canonical_hash),
    })
}

fn read_count(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<usize, FormatError> {
    let count = usize::try_from(read_index(bytes, cursor, label)?)
        .map_err(|_| malformed(*cursor, format!("RIDL {label} count overflows memory")))?;
    if count > bytes.len().saturating_sub(*cursor) {
        return Err(malformed(
            *cursor,
            format!("RIDL {label} count exceeds section size"),
        ));
    }
    Ok(count)
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    label: &str,
) -> Result<String, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor, label)?)
        .map_err(|_| malformed(offset, format!("RIDL {label} index overflows memory")))?;
    strings
        .get(index)
        .map(str::to_owned)
        .ok_or_else(|| malformed(offset, format!("RIDL {label} index is out of range")))
}

fn read_index(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid RIDL {label} ULEB: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u8, FormatError> {
    let value = *bytes
        .get(*cursor)
        .ok_or_else(|| malformed(*cursor, format!("truncated RIDL {label}")))?;
    *cursor += 1;
    Ok(value)
}

fn read_array<const N: usize>(
    bytes: &[u8],
    cursor: &mut usize,
    label: &str,
) -> Result<[u8; N], FormatError> {
    let start = *cursor;
    let end = start
        .checked_add(N)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(start, format!("truncated RIDL {label}")))?;
    let mut value = [0; N];
    value.copy_from_slice(&bytes[start..end]);
    *cursor = end;
    Ok(value)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
