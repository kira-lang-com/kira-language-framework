use crate::leb::{LebError, read_uleb, write_uleb};
use crate::{
    BinaryImport, BinaryImportEntityKind, BinaryImportKind, BinaryModule, FormatError, StringTable,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BinaryImportReference {
    pub import_index: u64,
    pub entity_index: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryCapabilities {
    pub reached_libtessera_calls: Vec<BinaryImportReference>,
    pub reached_requires_functions: Vec<BinaryImportReference>,
    pub reached_platform_imports: Vec<BinaryImportReference>,
    pub implied_entitlements: Vec<String>,
    pub requested_entitlements: Vec<String>,
}

impl BinaryCapabilities {
    pub fn encode(
        &self,
        strings: &StringTable,
        imports: &[BinaryImport],
    ) -> Result<Vec<u8>, FormatError> {
        let mut output = Vec::new();
        write_references(
            &self.reached_libtessera_calls,
            imports,
            ReferenceKind::RidlFunction,
            "LibTessera call",
            &mut output,
        )?;
        write_references(
            &self.reached_requires_functions,
            imports,
            ReferenceKind::RidlFunction,
            "@Requires function",
            &mut output,
        )?;
        write_references(
            &self.reached_platform_imports,
            imports,
            ReferenceKind::Platform,
            "platform import",
            &mut output,
        )?;
        write_entitlements(&self.implied_entitlements, strings, "implied", &mut output)?;
        write_entitlements(
            &self.requested_entitlements,
            strings,
            "requested",
            &mut output,
        )?;
        Ok(output)
    }
}

impl BinaryModule {
    pub fn capabilities(&self, memory_limit: u64) -> Result<BinaryCapabilities, FormatError> {
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == 13)
            .ok_or_else(|| FormatError::malformed(None, "CAPS section is missing"))?;
        let imports = self.imports(memory_limit)?;
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let reached_libtessera_calls = read_references(
            &bytes,
            &mut cursor,
            &imports,
            ReferenceKind::RidlFunction,
            "LibTessera call",
        )?;
        let reached_requires_functions = read_references(
            &bytes,
            &mut cursor,
            &imports,
            ReferenceKind::RidlFunction,
            "@Requires function",
        )?;
        let reached_platform_imports = read_references(
            &bytes,
            &mut cursor,
            &imports,
            ReferenceKind::Platform,
            "platform import",
        )?;
        let implied_entitlements = read_entitlements(&bytes, &mut cursor, &strings, "implied")?;
        let requested_entitlements = read_entitlements(&bytes, &mut cursor, &strings, "requested")?;
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in CAPS section"));
        }
        Ok(BinaryCapabilities {
            reached_libtessera_calls,
            reached_requires_functions,
            reached_platform_imports,
            implied_entitlements,
            requested_entitlements,
        })
    }
}

#[derive(Clone, Copy)]
enum ReferenceKind {
    RidlFunction,
    Platform,
}

fn write_references(
    references: &[BinaryImportReference],
    imports: &[BinaryImport],
    kind: ReferenceKind,
    label: &str,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(references.len(), label, output)?;
    let mut previous = None;
    for reference in references {
        if previous.is_some_and(|value| reference <= &value) {
            return Err(FormatError::malformed(
                None,
                format!("CAPS {label} references are not strictly ordered"),
            ));
        }
        previous = Some(*reference);
        validate_reference(*reference, imports, kind, label)?;
        write_index(reference.import_index, label, output)?;
        write_index(reference.entity_index, label, output)?;
    }
    Ok(())
}

fn read_references(
    bytes: &[u8],
    cursor: &mut usize,
    imports: &[BinaryImport],
    kind: ReferenceKind,
    label: &str,
) -> Result<Vec<BinaryImportReference>, FormatError> {
    let count = read_count(bytes, cursor, 2, label)?;
    let mut references = reserve(count, bytes, *cursor, label)?;
    let mut previous = None;
    for _ in 0..count {
        let offset = *cursor;
        let reference = BinaryImportReference {
            import_index: read_index(bytes, cursor, label)?,
            entity_index: read_index(bytes, cursor, label)?,
        };
        if previous.is_some_and(|value| reference <= value) {
            return Err(malformed(
                offset,
                format!("CAPS {label} references are not strictly ordered"),
            ));
        }
        previous = Some(reference);
        validate_reference(reference, imports, kind, label)
            .map_err(|error| with_offset(error, offset))?;
        references.push(reference);
    }
    Ok(references)
}

fn validate_reference(
    reference: BinaryImportReference,
    imports: &[BinaryImport],
    kind: ReferenceKind,
    label: &str,
) -> Result<(), FormatError> {
    let import_index = usize::try_from(reference.import_index).map_err(|_| {
        FormatError::malformed(None, format!("CAPS {label} import index overflows"))
    })?;
    let entity_index = usize::try_from(reference.entity_index).map_err(|_| {
        FormatError::malformed(None, format!("CAPS {label} entity index overflows"))
    })?;
    let import = imports.get(import_index).ok_or_else(|| {
        FormatError::malformed(None, format!("CAPS {label} import index is out of range"))
    })?;
    let entity = import.entities.get(entity_index).ok_or_else(|| {
        FormatError::malformed(None, format!("CAPS {label} entity index is out of range"))
    })?;
    let valid = match kind {
        ReferenceKind::RidlFunction => {
            import.kind == BinaryImportKind::Ridl && entity.kind == BinaryImportEntityKind::Function
        }
        ReferenceKind::Platform => import.kind == BinaryImportKind::Platform,
    };
    if !valid {
        return Err(FormatError::malformed(
            None,
            format!("CAPS {label} reference has the wrong import kind"),
        ));
    }
    Ok(())
}

fn write_entitlements(
    values: &[String],
    strings: &StringTable,
    label: &str,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(values.len(), label, output)?;
    let mut previous: Option<&str> = None;
    for value in values {
        if value.is_empty() || previous.is_some_and(|prior| prior.as_bytes() >= value.as_bytes()) {
            return Err(FormatError::malformed(
                None,
                format!("CAPS {label} entitlements must be nonempty and UTF-8 ordered"),
            ));
        }
        previous = Some(value);
        let index = strings.index_of(value).ok_or_else(|| {
            FormatError::malformed(None, "CAPS entitlement is absent from STRINGS")
        })?;
        let index = u64::try_from(index)
            .map_err(|_| FormatError::malformed(None, "CAPS string index overflows"))?;
        write_index(index, label, output)?;
    }
    Ok(())
}

fn read_entitlements(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    label: &str,
) -> Result<Vec<String>, FormatError> {
    let count = read_count(bytes, cursor, 1, label)?;
    let mut values = reserve(count, bytes, *cursor, label)?;
    for _ in 0..count {
        let offset = *cursor;
        let index = usize::try_from(read_index(bytes, cursor, label)?)
            .map_err(|_| malformed(offset, "CAPS string index overflows"))?;
        let value = strings
            .get(index)
            .ok_or_else(|| malformed(offset, "CAPS entitlement string index is out of range"))?;
        if value.is_empty()
            || values
                .last()
                .is_some_and(|previous: &String| previous.as_bytes() >= value.as_bytes())
        {
            return Err(malformed(
                offset,
                format!("CAPS {label} entitlements are empty or not UTF-8 ordered"),
            ));
        }
        values.push(value.to_owned());
    }
    Ok(values)
}

fn write_count(count: usize, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let count = u64::try_from(count)
        .map_err(|_| FormatError::malformed(None, format!("CAPS {label} count overflows")))?;
    write_uleb(count, output).map_err(|error| FormatError::malformed(None, error.to_string()))
}

fn write_index(value: u64, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output)
        .map_err(|error| FormatError::malformed(None, format!("CAPS {label} index: {error}")))
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    min_record_bytes: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = read_index(bytes, cursor, label)?;
    let count = usize::try_from(count).map_err(|_| malformed(offset, "CAPS count overflows"))?;
    if count > bytes.len().saturating_sub(*cursor) / min_record_bytes {
        return Err(malformed(
            offset,
            format!("CAPS {label} count exceeds section size"),
        ));
    }
    Ok(count)
}

fn read_index(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, FormatError> {
    read_uleb(bytes, cursor).map_err(|error| {
        malformed(
            *cursor,
            format!("invalid CAPS {label} ULEB: {}", leb_message(error)),
        )
    })
}

fn reserve<T>(
    count: usize,
    bytes: &[u8],
    offset: usize,
    label: &str,
) -> Result<Vec<T>, FormatError> {
    if count > bytes.len() {
        return Err(malformed(
            offset,
            format!("CAPS {label} count exceeds section size"),
        ));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, format!("CAPS {label} allocation failed")))?;
    Ok(values)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}

fn with_offset(mut error: FormatError, offset: usize) -> FormatError {
    if error.offset.is_none() {
        error.offset = Some(offset);
    }
    error
}

fn leb_message(error: LebError) -> &'static str {
    match error {
        LebError::Truncated => "truncated",
        LebError::Overflow => "overflow",
        LebError::NonMinimal => "non-minimal",
        LebError::InvalidSign => "invalid sign",
    }
}
