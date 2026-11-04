use std::io;

use crate::binary::FormatError;
use crate::leb::{write_integer_immediate, write_uleb};
use crate::strings::StringTable;

use super::{
    BinaryDebugTable, DebugCompileUnit, DebugDataRecord, DebugEnumerator, DebugFileRecord,
    DebugLabel, DebugLocation, DebugScope, DebugScopeKind, DebugSourceLocation, DebugSourceType,
    DebugTypeMember, DebugVariable,
};

pub fn encode_binary_debug_table(
    table: &BinaryDebugTable,
    strings: &StringTable,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    write_records(&table.files, &mut bytes, |record, bytes| {
        encode_file(record, strings, bytes)
    })?;
    write_records(&table.compile_units, &mut bytes, |record, bytes| {
        encode_compile_unit(record, strings, bytes)
    })?;
    write_records(&table.source_types, &mut bytes, |record, bytes| {
        encode_source_type(record, strings, bytes)
    })?;
    write_records(&table.scopes, &mut bytes, |record, bytes| {
        encode_scope(record, bytes)
    })?;
    write_locations(&table.locations, &mut bytes)?;
    write_records(&table.variables, &mut bytes, |record, bytes| {
        encode_variable(record, strings, bytes)
    })?;
    write_records(&table.labels, &mut bytes, |record, bytes| {
        encode_label(record, strings, bytes)
    })?;
    write_records(&table.data, &mut bytes, |record, bytes| {
        encode_data_record(record, bytes)
    })?;

    let decoded = BinaryDebugTable::decode(&bytes, strings)?;
    if decoded != *table {
        return Err(malformed(
            0,
            "DEBUG records are inconsistent with their encoding",
        ));
    }
    Ok(bytes)
}

fn encode_file(
    record: &DebugFileRecord,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_string(&record.path, strings, bytes)?;
    let mut flags = 0;
    if record.content_hash.is_some() {
        flags |= 1;
    }
    if record.generated_text.is_some() {
        flags |= 2;
    }
    bytes.push(flags);
    if let Some(hash) = record.content_hash {
        bytes.extend_from_slice(&hash);
    }
    if let Some(text) = &record.generated_text {
        write_index(
            u64::try_from(text.len())
                .map_err(|_| malformed(bytes.len(), "DEBUG generated text length overflows"))?,
            bytes,
        )?;
        bytes.extend_from_slice(text.as_bytes());
    }
    Ok(())
}

fn encode_compile_unit(
    record: &DebugCompileUnit,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_string(&record.language, strings, bytes)?;
    write_string(&record.producer, strings, bytes)
}

fn encode_source_type(
    ty: &DebugSourceType,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    match ty {
        DebugSourceType::Base {
            name,
            encoding,
            bit_width,
        } => {
            bytes.push(0);
            write_string(name, strings, bytes)?;
            bytes.push(*encoding);
            write_index(*bit_width, bytes)?;
        }
        DebugSourceType::Composite {
            name,
            kind,
            size,
            alignment,
            members,
        } => {
            bytes.push(1);
            write_string(name, strings, bytes)?;
            bytes.push(*kind);
            write_index(*size, bytes)?;
            write_index(*alignment, bytes)?;
            write_records(members, bytes, |member, bytes| {
                encode_member(member, strings, bytes)
            })?;
        }
        DebugSourceType::Enum {
            name,
            underlying_type,
            variants,
        } => {
            bytes.push(2);
            write_string(name, strings, bytes)?;
            write_index(*underlying_type, bytes)?;
            write_records(variants, bytes, |variant, bytes| {
                encode_enumerator(variant, strings, bytes)
            })?;
        }
        DebugSourceType::Pointer { pointee_type } => {
            bytes.push(3);
            write_index(*pointee_type, bytes)?;
        }
        DebugSourceType::Reference {
            pointee_type,
            mutable,
        } => {
            bytes.push(4);
            write_index(*pointee_type, bytes)?;
            bytes.push(u8::from(*mutable));
        }
        DebugSourceType::Typedef { name, type_index } => {
            bytes.push(5);
            write_string(name, strings, bytes)?;
            write_index(*type_index, bytes)?;
        }
        DebugSourceType::TemplateParameter {
            name,
            kind,
            value_type,
        } => {
            bytes.push(6);
            write_string(name, strings, bytes)?;
            bytes.push(*kind);
            if let Some(value_type) = value_type {
                write_index(*value_type, bytes)?;
            }
        }
        DebugSourceType::Array {
            element_type,
            count,
        } => {
            bytes.push(7);
            write_index(*element_type, bytes)?;
            match count {
                Some(count) => {
                    bytes.push(1);
                    write_index(*count, bytes)?;
                }
                None => bytes.push(0),
            }
        }
        DebugSourceType::Qualified {
            type_index,
            qualifiers,
        } => {
            bytes.push(8);
            write_index(*type_index, bytes)?;
            bytes.push(*qualifiers);
        }
    }
    Ok(())
}

fn encode_member(
    member: &DebugTypeMember,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_string(&member.name, strings, bytes)?;
    write_index(member.type_index, bytes)?;
    write_index(member.byte_offset, bytes)
}

fn encode_enumerator(
    enumerator: &DebugEnumerator,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_string(&enumerator.name, strings, bytes)?;
    write_integer_immediate(enumerator.value.negative, enumerator.value.magnitude, bytes)
        .map_err(|error| malformed(bytes.len(), error.to_string()))
}

fn encode_scope(scope: &DebugScope, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    let (kind, call_site) = match &scope.kind {
        DebugScopeKind::Function => (0, None),
        DebugScopeKind::Lexical => (1, None),
        DebugScopeKind::Inlined { call_site } => (2, Some(*call_site)),
    };
    bytes.push(kind);
    write_index(scope.function_index, bytes)?;
    write_index(scope.compile_unit, bytes)?;
    let parent = scope
        .parent
        .map(|parent| {
            parent
                .checked_add(1)
                .ok_or_else(|| malformed(bytes.len(), "DEBUG scope parent overflows"))
        })
        .transpose()?
        .unwrap_or(0);
    write_index(parent, bytes)?;
    if let Some(call_site) = call_site {
        encode_source_location(call_site, bytes)?;
    }
    Ok(())
}

fn write_locations(locations: &[DebugLocation], bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_count(locations.len(), bytes)?;
    let mut previous_function = None;
    let mut previous_ordinal = 0u64;
    for location in locations {
        if previous_function.is_some_and(|function| location.function_index < function) {
            return Err(malformed(bytes.len(), "DEBUG locations are not ordered"));
        }
        let ordinal_delta = if previous_function == Some(location.function_index) {
            location
                .instruction_ordinal
                .checked_sub(previous_ordinal)
                .filter(|delta| *delta != 0)
                .ok_or_else(|| malformed(bytes.len(), "DEBUG instruction ordinals repeat"))?
        } else {
            location.instruction_ordinal
        };
        write_index(location.function_index, bytes)?;
        write_index(ordinal_delta, bytes)?;
        encode_source_location(location.source, bytes)?;
        let scope = location
            .scope
            .map(|scope| {
                scope
                    .checked_add(1)
                    .ok_or_else(|| malformed(bytes.len(), "DEBUG location scope overflows"))
            })
            .transpose()?
            .unwrap_or(0);
        write_index(scope, bytes)?;
        previous_function = Some(location.function_index);
        previous_ordinal = location.instruction_ordinal;
    }
    Ok(())
}

fn encode_variable(
    variable: &DebugVariable,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_string(&variable.name, strings, bytes)?;
    write_index(variable.source_type, bytes)?;
    write_index(variable.scope, bytes)
}

fn encode_label(
    label: &DebugLabel,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_string(&label.name, strings, bytes)?;
    write_index(label.scope, bytes)?;
    encode_source_location(label.source, bytes)
}

fn encode_data_record(record: &DebugDataRecord, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_index(record.data_index, bytes)?;
    write_index(record.source_type, bytes)?;
    encode_source_location(record.source, bytes)
}

fn encode_source_location(
    source: DebugSourceLocation,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_index(source.file_index, bytes)?;
    write_index(source.line, bytes)?;
    write_index(source.column, bytes)
}

fn write_records<T>(
    records: &[T],
    bytes: &mut Vec<u8>,
    mut encode: impl FnMut(&T, &mut Vec<u8>) -> Result<(), FormatError>,
) -> Result<(), FormatError> {
    write_count(records.len(), bytes)?;
    for record in records {
        encode(record, bytes)?;
    }
    Ok(())
}

fn write_string(
    value: &str,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(malformed(bytes.len(), "DEBUG string cannot be empty"));
    }
    let index = strings
        .index_of(value)
        .ok_or_else(|| malformed(bytes.len(), "DEBUG string is absent from STRINGS"))?;
    write_index(
        u64::try_from(index).map_err(|_| malformed(bytes.len(), "DEBUG string index overflows"))?,
        bytes,
    )
}

fn write_count(count: usize, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_index(
        u64::try_from(count).map_err(|_| malformed(bytes.len(), "DEBUG count overflows"))?,
        bytes,
    )
}

fn write_index(value: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, bytes).map_err(io_error(bytes.len()))
}

fn io_error(offset: usize) -> impl FnOnce(io::Error) -> FormatError {
    move |error| malformed(offset, error.to_string())
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
