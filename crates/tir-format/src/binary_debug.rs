use std::collections::HashSet;

use crate::binary::{BinaryModule, FormatError};
use crate::leb::{IntegerImmediate, read_integer_immediate, read_uleb};
use crate::strings::StringTable;

mod encode;
pub use encode::encode_binary_debug_table;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryDebugTable {
    pub files: Vec<DebugFileRecord>,
    pub compile_units: Vec<DebugCompileUnit>,
    pub source_types: Vec<DebugSourceType>,
    pub scopes: Vec<DebugScope>,
    pub locations: Vec<DebugLocation>,
    pub variables: Vec<DebugVariable>,
    pub labels: Vec<DebugLabel>,
    pub data: Vec<DebugDataRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugFileRecord {
    pub path: String,
    pub content_hash: Option<[u8; 32]>,
    pub generated_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugCompileUnit {
    pub language: String,
    pub producer: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugSourceType {
    Base {
        name: String,
        encoding: u8,
        bit_width: u64,
    },
    Composite {
        name: String,
        kind: u8,
        size: u64,
        alignment: u64,
        members: Vec<DebugTypeMember>,
    },
    Enum {
        name: String,
        underlying_type: u64,
        variants: Vec<DebugEnumerator>,
    },
    Pointer {
        pointee_type: u64,
    },
    Reference {
        pointee_type: u64,
        mutable: bool,
    },
    Typedef {
        name: String,
        type_index: u64,
    },
    TemplateParameter {
        name: String,
        kind: u8,
        value_type: Option<u64>,
    },
    Array {
        element_type: u64,
        count: Option<u64>,
    },
    Qualified {
        type_index: u64,
        qualifiers: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugTypeMember {
    pub name: String,
    pub type_index: u64,
    pub byte_offset: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugEnumerator {
    pub name: String,
    pub value: IntegerImmediate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugScope {
    pub function_index: u64,
    pub compile_unit: u64,
    pub parent: Option<u64>,
    pub kind: DebugScopeKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugScopeKind {
    Function,
    Lexical,
    Inlined { call_site: DebugSourceLocation },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DebugSourceLocation {
    pub file_index: u64,
    pub line: u64,
    pub column: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DebugLocation {
    pub function_index: u64,
    pub instruction_ordinal: u64,
    pub source: DebugSourceLocation,
    pub scope: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugVariable {
    pub name: String,
    pub source_type: u64,
    pub scope: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugLabel {
    pub name: String,
    pub scope: u64,
    pub source: DebugSourceLocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugDataRecord {
    /// Zero-based index in DATA, ordered by the corresponding data object.
    pub data_index: u64,
    pub source_type: u64,
    pub source: DebugSourceLocation,
}

impl BinaryDebugTable {
    pub fn decode(bytes: &[u8], strings: &StringTable) -> Result<Self, FormatError> {
        let mut cursor = 0;
        let files = read_records(bytes, &mut cursor, "DEBUG file", |bytes, cursor, _| {
            decode_file(bytes, cursor, strings)
        })?;
        let compile_units = read_records(
            bytes,
            &mut cursor,
            "DEBUG compile unit",
            |bytes, cursor, _| decode_compile_unit(bytes, cursor, strings),
        )?;
        let source_types = read_records(
            bytes,
            &mut cursor,
            "DEBUG source type",
            |bytes, cursor, _| decode_source_type(bytes, cursor, strings),
        )?;
        let scopes = read_records(bytes, &mut cursor, "DEBUG scope", |bytes, cursor, index| {
            decode_scope(
                bytes,
                cursor,
                index,
                files.len() as u64,
                compile_units.len() as u64,
            )
        })?;
        let locations = decode_locations(bytes, &mut cursor, files.len() as u64, &scopes)?;
        let variables = read_records(bytes, &mut cursor, "DEBUG variable", |bytes, cursor, _| {
            decode_variable(bytes, cursor, strings)
        })?;
        let labels = read_records(bytes, &mut cursor, "DEBUG label", |bytes, cursor, _| {
            decode_label(bytes, cursor, strings, files.len() as u64, &scopes)
        })?;
        let data = decode_data_records(bytes, &mut cursor, files.len() as u64)?;
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in DEBUG section"));
        }
        validate_type_references(&source_types)?;
        validate_scopes(&scopes)?;
        validate_location_references(&locations, &scopes)?;
        validate_variable_references(&variables, &source_types, &scopes)?;
        validate_data_references(&data, &source_types)?;
        validate_label_uniqueness(&labels)?;
        Ok(Self {
            files,
            compile_units,
            source_types,
            scopes,
            locations,
            variables,
            labels,
            data,
        })
    }
}

impl BinaryModule {
    /// Decode the optional shared TIR/TVISA DEBUG table in this TIR module.
    pub fn debug_table(&self, memory_limit: u64) -> Result<Option<BinaryDebugTable>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 17) else {
            return Ok(None);
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        BinaryDebugTable::decode(&bytes, &strings).map(Some)
    }
}

fn decode_file(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<DebugFileRecord, FormatError> {
    let start = *cursor;
    let path = read_string(bytes, cursor, strings, "DEBUG file path")?;
    let flags = read_byte(bytes, cursor, "DEBUG file flags")?;
    if flags & !3 != 0 {
        return Err(malformed(*cursor - 1, "reserved DEBUG file flags are set"));
    }
    let content_hash = if flags & 1 != 0 {
        Some(read_fixed::<32>(bytes, cursor, "DEBUG file hash")?)
    } else {
        None
    };
    let generated_text = if flags & 2 != 0 {
        let length = read_index(bytes, cursor, "DEBUG generated text length")?;
        let length = usize::try_from(length)
            .map_err(|_| malformed(start, "DEBUG generated text does not fit in memory"))?;
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| malformed(*cursor, "truncated DEBUG generated text"))?;
        let text = std::str::from_utf8(&bytes[*cursor..end])
            .map_err(|_| malformed(*cursor, "DEBUG generated text is not UTF-8"))?
            .to_owned();
        *cursor = end;
        Some(text)
    } else {
        None
    };
    Ok(DebugFileRecord {
        path,
        content_hash,
        generated_text,
    })
}

fn decode_compile_unit(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<DebugCompileUnit, FormatError> {
    Ok(DebugCompileUnit {
        language: read_string(bytes, cursor, strings, "DEBUG source language")?,
        producer: read_string(bytes, cursor, strings, "DEBUG producer")?,
    })
}

fn decode_source_type(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<DebugSourceType, FormatError> {
    let start = *cursor;
    match read_byte(bytes, cursor, "DEBUG source type tag")? {
        0 => {
            let name = read_string(bytes, cursor, strings, "DEBUG base type name")?;
            let encoding = read_byte(bytes, cursor, "DEBUG base encoding")?;
            if encoding == 0 {
                return Err(malformed(*cursor - 1, "invalid DEBUG base encoding"));
            }
            Ok(DebugSourceType::Base {
                name,
                encoding,
                bit_width: read_index(bytes, cursor, "DEBUG base bit width")?,
            })
        }
        1 => {
            let name = read_string(bytes, cursor, strings, "DEBUG composite type name")?;
            let kind = read_byte(bytes, cursor, "DEBUG composite kind")?;
            if kind > 1 {
                return Err(malformed(*cursor - 1, "invalid DEBUG composite kind"));
            }
            let size = read_index(bytes, cursor, "DEBUG composite size")?;
            let alignment = read_index(bytes, cursor, "DEBUG composite alignment")?;
            if alignment == 0 || !alignment.is_power_of_two() {
                return Err(malformed(*cursor, "DEBUG composite alignment is invalid"));
            }
            let members = read_records(
                bytes,
                cursor,
                "DEBUG composite member",
                |bytes, cursor, _| {
                    Ok(DebugTypeMember {
                        name: read_string(bytes, cursor, strings, "DEBUG member name")?,
                        type_index: read_index(bytes, cursor, "DEBUG member type")?,
                        byte_offset: read_index(bytes, cursor, "DEBUG member offset")?,
                    })
                },
            )?;
            if members.iter().any(|member| member.byte_offset > size) {
                return Err(malformed(
                    start,
                    "DEBUG member offset exceeds its type size",
                ));
            }
            Ok(DebugSourceType::Composite {
                name,
                kind,
                size,
                alignment,
                members,
            })
        }
        2 => {
            let name = read_string(bytes, cursor, strings, "DEBUG enum name")?;
            let underlying_type = read_index(bytes, cursor, "DEBUG enum underlying type")?;
            let variants = read_records(bytes, cursor, "DEBUG enumerator", |bytes, cursor, _| {
                Ok(DebugEnumerator {
                    name: read_string(bytes, cursor, strings, "DEBUG enumerator name")?,
                    value: read_integer_immediate(bytes, cursor).map_err(|error| {
                        malformed(
                            *cursor,
                            format!("invalid DEBUG enumerator value: {error:?}"),
                        )
                    })?,
                })
            })?;
            Ok(DebugSourceType::Enum {
                name,
                underlying_type,
                variants,
            })
        }
        3 => Ok(DebugSourceType::Pointer {
            pointee_type: read_index(bytes, cursor, "DEBUG pointer target")?,
        }),
        4 => {
            let pointee_type = read_index(bytes, cursor, "DEBUG reference target")?;
            let mutable = read_bool(bytes, cursor, "DEBUG reference mutability")?;
            Ok(DebugSourceType::Reference {
                pointee_type,
                mutable,
            })
        }
        5 => Ok(DebugSourceType::Typedef {
            name: read_string(bytes, cursor, strings, "DEBUG typedef name")?,
            type_index: read_index(bytes, cursor, "DEBUG typedef target")?,
        }),
        6 => {
            let name = read_string(bytes, cursor, strings, "DEBUG template parameter name")?;
            let kind = read_byte(bytes, cursor, "DEBUG template parameter kind")?;
            if kind > 1 {
                return Err(malformed(
                    *cursor - 1,
                    "invalid DEBUG template parameter kind",
                ));
            }
            let value_type = (kind == 1)
                .then(|| read_index(bytes, cursor, "DEBUG value parameter type"))
                .transpose()?;
            Ok(DebugSourceType::TemplateParameter {
                name,
                kind,
                value_type,
            })
        }
        7 => {
            let element_type = read_index(bytes, cursor, "DEBUG array element type")?;
            let count = match read_byte(bytes, cursor, "DEBUG array count flag")? {
                0 => None,
                1 => Some(read_index(bytes, cursor, "DEBUG array element count")?),
                _ => return Err(malformed(*cursor - 1, "invalid DEBUG array count flag")),
            };
            Ok(DebugSourceType::Array {
                element_type,
                count,
            })
        }
        8 => {
            let type_index = read_index(bytes, cursor, "DEBUG qualified type")?;
            let qualifiers = read_byte(bytes, cursor, "DEBUG qualifiers")?;
            if qualifiers == 0 || qualifiers & !7 != 0 {
                return Err(malformed(*cursor - 1, "invalid DEBUG qualifiers"));
            }
            Ok(DebugSourceType::Qualified {
                type_index,
                qualifiers,
            })
        }
        _ => Err(malformed(start, "unknown DEBUG source type tag")),
    }
}

fn decode_scope(
    bytes: &[u8],
    cursor: &mut usize,
    scope_index: u64,
    file_count: u64,
    compile_unit_count: u64,
) -> Result<DebugScope, FormatError> {
    let start = *cursor;
    let kind = read_byte(bytes, cursor, "DEBUG scope kind")?;
    let function_index = read_index(bytes, cursor, "DEBUG scope function")?;
    let compile_unit = read_index(bytes, cursor, "DEBUG scope compile unit")?;
    if compile_unit >= compile_unit_count {
        return Err(malformed(start, "DEBUG scope compile unit is out of range"));
    }
    let parent_encoded = read_index(bytes, cursor, "DEBUG scope parent")?;
    let parent = parent_encoded.checked_sub(1);
    match kind {
        0 if parent.is_none() => Ok(DebugScope {
            function_index,
            compile_unit,
            parent,
            kind: DebugScopeKind::Function,
        }),
        1 | 2 => {
            let parent =
                parent.ok_or_else(|| malformed(start, "nested DEBUG scope has no parent"))?;
            if parent >= scope_index {
                return Err(malformed(
                    start,
                    "DEBUG scope parent must precede its child",
                ));
            }
            if kind == 1 {
                Ok(DebugScope {
                    function_index,
                    compile_unit,
                    parent: Some(parent),
                    kind: DebugScopeKind::Lexical,
                })
            } else {
                let call_site = read_source_location(bytes, cursor, file_count)?;
                Ok(DebugScope {
                    function_index,
                    compile_unit,
                    parent: Some(parent),
                    kind: DebugScopeKind::Inlined { call_site },
                })
            }
        }
        0 => Err(malformed(
            start,
            "function DEBUG scope cannot have a parent",
        )),
        _ => Err(malformed(start, "unknown DEBUG scope kind")),
    }
}

fn decode_locations(
    bytes: &[u8],
    cursor: &mut usize,
    file_count: u64,
    scopes: &[DebugScope],
) -> Result<Vec<DebugLocation>, FormatError> {
    let mut previous_function = None;
    let mut previous_ordinal = 0u64;
    read_records(bytes, cursor, "DEBUG location", |bytes, cursor, _| {
        let start = *cursor;
        let function_index = read_index(bytes, cursor, "DEBUG location function")?;
        let ordinal_delta = read_index(bytes, cursor, "DEBUG instruction ordinal delta")?;
        let instruction_ordinal = if previous_function == Some(function_index) {
            if ordinal_delta == 0 {
                return Err(malformed(start, "DEBUG instruction ordinal delta is zero"));
            }
            previous_ordinal
                .checked_add(ordinal_delta)
                .ok_or_else(|| malformed(start, "DEBUG instruction ordinal overflows"))?
        } else {
            ordinal_delta
        };
        if previous_function.is_some_and(|previous| function_index < previous) {
            return Err(malformed(start, "DEBUG locations are not ordered"));
        }
        let file_index = read_index(bytes, cursor, "DEBUG location file")?;
        if file_index >= file_count {
            return Err(malformed(start, "DEBUG location file is out of range"));
        }
        let line = read_index(bytes, cursor, "DEBUG location line")?;
        let column = read_index(bytes, cursor, "DEBUG location column")?;
        validate_line_column(line, column, start)?;
        let scope = read_index(bytes, cursor, "DEBUG location scope")?.checked_sub(1);
        if scope.is_some_and(|scope| scope >= scopes.len() as u64) {
            return Err(malformed(start, "DEBUG location scope is out of range"));
        }
        let location = DebugLocation {
            function_index,
            instruction_ordinal,
            source: DebugSourceLocation {
                file_index,
                line,
                column,
            },
            scope,
        };
        previous_function = Some(function_index);
        previous_ordinal = instruction_ordinal;
        Ok(location)
    })
    .and_then(|locations| {
        if locations.windows(2).any(|pair| {
            (pair[0].function_index, pair[0].instruction_ordinal)
                >= (pair[1].function_index, pair[1].instruction_ordinal)
        }) {
            return Err(malformed(
                *cursor,
                "DEBUG locations are not strictly ordered",
            ));
        }
        Ok(locations)
    })
}

fn decode_variable(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<DebugVariable, FormatError> {
    Ok(DebugVariable {
        name: read_string(bytes, cursor, strings, "DEBUG variable name")?,
        source_type: read_index(bytes, cursor, "DEBUG variable source type")?,
        scope: read_index(bytes, cursor, "DEBUG variable scope")?,
    })
}

fn decode_label(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    file_count: u64,
    scopes: &[DebugScope],
) -> Result<DebugLabel, FormatError> {
    let name = read_string(bytes, cursor, strings, "DEBUG label name")?;
    let scope = read_index(bytes, cursor, "DEBUG label scope")?;
    if scope >= scopes.len() as u64 {
        return Err(malformed(*cursor, "DEBUG label scope is out of range"));
    }
    let source = read_source_location(bytes, cursor, file_count)?;
    Ok(DebugLabel {
        name,
        scope,
        source,
    })
}

fn decode_data_records(
    bytes: &[u8],
    cursor: &mut usize,
    file_count: u64,
) -> Result<Vec<DebugDataRecord>, FormatError> {
    read_records(bytes, cursor, "DEBUG data record", |bytes, cursor, _| {
        let data_index = read_index(bytes, cursor, "DEBUG DATA index")?;
        let source_type = read_index(bytes, cursor, "DEBUG DATA source type")?;
        let source = read_source_location(bytes, cursor, file_count)?;
        Ok(DebugDataRecord {
            data_index,
            source_type,
            source,
        })
    })
    .and_then(|records| {
        if records
            .windows(2)
            .any(|pair| pair[0].data_index >= pair[1].data_index)
        {
            return Err(malformed(*cursor, "DEBUG DATA records are not ordered"));
        }
        Ok(records)
    })
}

fn read_source_location(
    bytes: &[u8],
    cursor: &mut usize,
    file_count: u64,
) -> Result<DebugSourceLocation, FormatError> {
    let start = *cursor;
    let file_index = read_index(bytes, cursor, "DEBUG source file")?;
    if file_index >= file_count {
        return Err(malformed(start, "DEBUG source file is out of range"));
    }
    let line = read_index(bytes, cursor, "DEBUG source line")?;
    let column = read_index(bytes, cursor, "DEBUG source column")?;
    validate_line_column(line, column, start)?;
    Ok(DebugSourceLocation {
        file_index,
        line,
        column,
    })
}

fn validate_type_references(types: &[DebugSourceType]) -> Result<(), FormatError> {
    let count = types.len() as u64;
    for (index, ty) in types.iter().enumerate() {
        let references: Vec<u64> = match ty {
            DebugSourceType::Base { bit_width, .. } if *bit_width == 0 => {
                return Err(malformed(index, "DEBUG base type width must be nonzero"));
            }
            DebugSourceType::Composite { members, .. } => {
                members.iter().map(|member| member.type_index).collect()
            }
            DebugSourceType::Enum {
                underlying_type, ..
            } => vec![*underlying_type],
            DebugSourceType::Pointer { pointee_type }
            | DebugSourceType::Reference { pointee_type, .. } => {
                vec![*pointee_type]
            }
            DebugSourceType::Typedef { type_index, .. }
            | DebugSourceType::Qualified { type_index, .. } => vec![*type_index],
            DebugSourceType::TemplateParameter { value_type, .. } => {
                value_type.iter().copied().collect()
            }
            DebugSourceType::Array { element_type, .. } => vec![*element_type],
            DebugSourceType::Base { .. } => Vec::new(),
        };
        if references.iter().any(|reference| *reference >= count) {
            return Err(malformed(
                index,
                "DEBUG source type reference is out of range",
            ));
        }
    }
    Ok(())
}

fn validate_variable_references(
    variables: &[DebugVariable],
    types: &[DebugSourceType],
    scopes: &[DebugScope],
) -> Result<(), FormatError> {
    for variable in variables {
        if variable.source_type >= types.len() as u64 || variable.scope >= scopes.len() as u64 {
            return Err(malformed(0, "DEBUG variable type or scope is out of range"));
        }
    }
    Ok(())
}

fn validate_data_references(
    records: &[DebugDataRecord],
    types: &[DebugSourceType],
) -> Result<(), FormatError> {
    if records
        .iter()
        .any(|record| record.source_type >= types.len() as u64)
    {
        return Err(malformed(0, "DEBUG DATA source type is out of range"));
    }
    Ok(())
}

fn validate_scopes(scopes: &[DebugScope]) -> Result<(), FormatError> {
    for scope in scopes {
        if matches!(
            scope.kind,
            DebugScopeKind::Lexical | DebugScopeKind::Inlined { .. }
        ) {
            let Some(parent) = scope.parent.and_then(|index| scopes.get(index as usize)) else {
                return Err(malformed(0, "nested DEBUG scope has no valid parent"));
            };
            if parent.function_index != scope.function_index {
                return Err(malformed(
                    0,
                    "nested DEBUG scope belongs to a different function than its parent",
                ));
            }
        }
    }
    Ok(())
}

fn validate_location_references(
    locations: &[DebugLocation],
    scopes: &[DebugScope],
) -> Result<(), FormatError> {
    for location in locations {
        if location.scope.is_some_and(|scope_index| {
            scopes
                .get(scope_index as usize)
                .is_none_or(|scope| scope.function_index != location.function_index)
        }) {
            return Err(malformed(
                0,
                "DEBUG location scope belongs to a different function",
            ));
        }
    }
    Ok(())
}

fn validate_label_uniqueness(labels: &[DebugLabel]) -> Result<(), FormatError> {
    let mut seen = HashSet::new();
    for label in labels {
        if !seen.insert((label.scope, label.name.as_str())) {
            return Err(malformed(0, "duplicate DEBUG label in a scope"));
        }
    }
    Ok(())
}

fn read_records<T>(
    bytes: &[u8],
    cursor: &mut usize,
    label: &str,
    mut decode: impl FnMut(&[u8], &mut usize, u64) -> Result<T, FormatError>,
) -> Result<Vec<T>, FormatError> {
    let start = *cursor;
    let count = read_index(bytes, cursor, &format!("{label} count"))?;
    if count > bytes.len().saturating_sub(*cursor) as u64 {
        return Err(malformed(
            start,
            format!("{label} count exceeds the section size"),
        ));
    }
    let capacity = usize::try_from(count)
        .map_err(|_| malformed(start, format!("{label} count does not fit in memory")))?;
    let mut records = Vec::new();
    records
        .try_reserve_exact(capacity)
        .map_err(|_| malformed(start, format!("{label} allocation failed")))?;
    for index in 0..count {
        records.push(decode(bytes, cursor, index)?);
    }
    Ok(records)
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    field: &str,
) -> Result<String, FormatError> {
    let start = *cursor;
    let index = read_index(bytes, cursor, field)?;
    let value = usize::try_from(index)
        .ok()
        .and_then(|index| strings.get(index))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| malformed(start, format!("{field} is not a nonempty STRINGS entry")))?;
    Ok(value.to_owned())
}

fn read_fixed<const N: usize>(
    bytes: &[u8],
    cursor: &mut usize,
    field: &str,
) -> Result<[u8; N], FormatError> {
    let start = *cursor;
    let end = cursor
        .checked_add(N)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(start, format!("truncated {field}")))?;
    let value = bytes[start..end]
        .try_into()
        .map_err(|_| malformed(start, format!("truncated {field}")))?;
    *cursor = end;
    Ok(value)
}

fn read_bool(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<bool, FormatError> {
    match read_byte(bytes, cursor, field)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(malformed(*cursor - 1, format!("invalid {field}"))),
    }
}

fn read_byte(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u8, FormatError> {
    let value = bytes
        .get(*cursor)
        .copied()
        .ok_or_else(|| malformed(*cursor, format!("truncated {field}")))?;
    *cursor += 1;
    Ok(value)
}

fn read_index(bytes: &[u8], cursor: &mut usize, field: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid {field} ULEB128: {error:?}")))
}

fn validate_line_column(line: u64, column: u64, offset: usize) -> Result<(), FormatError> {
    if line == 0 || column == 0 {
        return Err(malformed(offset, "DEBUG line and column are one-based"));
    }
    Ok(())
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
