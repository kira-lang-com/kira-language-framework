use crate::leb::{read_uleb, write_uleb};
use crate::{BinaryModule, FormatError, StringTable};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryImportKind {
    Ridl,
    Package,
    Platform,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryImportEntityKind {
    Function,
    Global,
    Type,
    Witness,
    Descriptor,
    PlatformData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryImportedEntity {
    pub kind: BinaryImportEntityKind,
    pub name: String,
    pub platform_symbol: Option<String>,
    pub type_index: u64,
    pub required_entitlements: Vec<String>,
    pub implied_entitlements: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryImport {
    pub kind: BinaryImportKind,
    pub name: String,
    pub abi_uuid: [u8; 16],
    pub weak: bool,
    pub entities: Vec<BinaryImportedEntity>,
}

pub fn encode_imports(
    imports: &[BinaryImport],
    strings: &StringTable,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    write_count(imports.len(), &mut bytes)?;
    for import in imports {
        if import.name.is_empty() {
            return Err(FormatError::malformed(None, "IMPORTS module name is empty"));
        }
        let kind = match import.kind {
            BinaryImportKind::Ridl => 0,
            BinaryImportKind::Package => 1,
            BinaryImportKind::Platform => 2,
        };
        let uuid_is_zero = import.abi_uuid.iter().all(|byte| *byte == 0);
        if (import.kind == BinaryImportKind::Ridl && uuid_is_zero)
            || (import.kind != BinaryImportKind::Ridl && !uuid_is_zero)
        {
            return Err(FormatError::malformed(
                None,
                "IMPORTS ABI UUID does not match its module kind",
            ));
        }
        if import.weak && import.kind != BinaryImportKind::Platform {
            return Err(FormatError::malformed(
                None,
                "only platform imports may be weak",
            ));
        }
        bytes.push(kind);
        write_string(&import.name, strings, &mut bytes)?;
        bytes.extend_from_slice(&import.abi_uuid);
        bytes.push(u8::from(import.weak));
        write_count(import.entities.len(), &mut bytes)?;
        for entity in &import.entities {
            let kind = match entity.kind {
                BinaryImportEntityKind::Function => 0,
                BinaryImportEntityKind::Global if import.kind != BinaryImportKind::Platform => 1,
                BinaryImportEntityKind::Type if import.kind != BinaryImportKind::Platform => 2,
                BinaryImportEntityKind::Witness if import.kind != BinaryImportKind::Platform => 3,
                BinaryImportEntityKind::Descriptor if import.kind != BinaryImportKind::Platform => {
                    4
                }
                BinaryImportEntityKind::PlatformData
                    if import.kind == BinaryImportKind::Platform =>
                {
                    5
                }
                _ => {
                    return Err(FormatError::malformed(
                        None,
                        "invalid IMPORTS entity kind for module kind",
                    ));
                }
            };
            if entity.name.is_empty() {
                return Err(FormatError::malformed(None, "IMPORTS entity name is empty"));
            }
            bytes.push(kind);
            write_string(&entity.name, strings, &mut bytes)?;
            match (import.kind, entity.platform_symbol.as_deref()) {
                (BinaryImportKind::Platform, Some(symbol)) if !symbol.is_empty() => {
                    write_string(symbol, strings, &mut bytes)?;
                }
                (BinaryImportKind::Platform, _) => {
                    return Err(FormatError::malformed(
                        None,
                        "platform import symbol is missing or empty",
                    ));
                }
                (_, None) => {}
                (_, Some(_)) => {
                    return Err(FormatError::malformed(
                        None,
                        "non-platform import has a platform symbol",
                    ));
                }
            }
            write_index(entity.type_index, &mut bytes)?;
            if (import.kind != BinaryImportKind::Ridl
                || entity.kind != BinaryImportEntityKind::Function)
                && (!entity.required_entitlements.is_empty()
                    || !entity.implied_entitlements.is_empty())
            {
                return Err(FormatError::malformed(
                    None,
                    "only RIDL function imports may declare entitlement metadata",
                ));
            }
            write_names(
                &entity.required_entitlements,
                strings,
                "@Requires",
                &mut bytes,
            )?;
            write_names(
                &entity.implied_entitlements,
                strings,
                "LibTessera",
                &mut bytes,
            )?;
        }
    }
    Ok(bytes)
}

impl BinaryModule {
    pub fn imports(&self, memory_limit: u64) -> Result<Vec<BinaryImport>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 3) else {
            return Ok(Vec::new());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let count = read_count(&bytes, &mut cursor, 20)?;
        let mut imports = reserve(count, &bytes, cursor)?;

        for _ in 0..count {
            let kind = match read_byte(&bytes, &mut cursor)? {
                0 => BinaryImportKind::Ridl,
                1 => BinaryImportKind::Package,
                2 => BinaryImportKind::Platform,
                _ => {
                    return Err(FormatError::malformed(
                        Some(cursor.saturating_sub(1)),
                        "invalid IMPORTS module kind",
                    ));
                }
            };
            let name = read_string(&bytes, &mut cursor, &strings)?;
            if name.is_empty() {
                return Err(FormatError::malformed(
                    Some(cursor),
                    "IMPORTS module name is empty",
                ));
            }
            let mut abi_uuid = [0; 16];
            let uuid_end = cursor
                .checked_add(abi_uuid.len())
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| {
                    FormatError::malformed(Some(cursor), "truncated IMPORTS ABI UUID")
                })?;
            abi_uuid.copy_from_slice(&bytes[cursor..uuid_end]);
            cursor = uuid_end;
            let flags = read_byte(&bytes, &mut cursor)?;
            if flags & !1 != 0 || (kind != BinaryImportKind::Platform && flags != 0) {
                return Err(FormatError::malformed(
                    Some(cursor.saturating_sub(1)),
                    "invalid IMPORTS flags",
                ));
            }
            let uuid_is_zero = abi_uuid.iter().all(|byte| *byte == 0);
            if (kind == BinaryImportKind::Ridl && uuid_is_zero)
                || (kind != BinaryImportKind::Ridl && !uuid_is_zero)
            {
                return Err(FormatError::malformed(
                    Some(uuid_end - abi_uuid.len()),
                    "IMPORTS ABI UUID does not match its module kind",
                ));
            }

            let minimum_entity_bytes = if kind == BinaryImportKind::Platform {
                6
            } else {
                5
            };
            let entity_count = read_count(&bytes, &mut cursor, minimum_entity_bytes)?;
            let mut entities = reserve(entity_count, &bytes, cursor)?;
            for _ in 0..entity_count {
                let entity_kind = match read_byte(&bytes, &mut cursor)? {
                    0 => BinaryImportEntityKind::Function,
                    1 if kind != BinaryImportKind::Platform => BinaryImportEntityKind::Global,
                    2 if kind != BinaryImportKind::Platform => BinaryImportEntityKind::Type,
                    3 if kind != BinaryImportKind::Platform => BinaryImportEntityKind::Witness,
                    4 if kind != BinaryImportKind::Platform => BinaryImportEntityKind::Descriptor,
                    5 if kind == BinaryImportKind::Platform => BinaryImportEntityKind::PlatformData,
                    _ => {
                        return Err(FormatError::malformed(
                            Some(cursor.saturating_sub(1)),
                            "invalid IMPORTS entity kind for module kind",
                        ));
                    }
                };
                let name = read_string(&bytes, &mut cursor, &strings)?;
                if name.is_empty() {
                    return Err(FormatError::malformed(
                        Some(cursor),
                        "IMPORTS entity name is empty",
                    ));
                }
                let platform_symbol = if kind == BinaryImportKind::Platform {
                    let symbol = read_string(&bytes, &mut cursor, &strings)?;
                    if symbol.is_empty() {
                        return Err(FormatError::malformed(
                            Some(cursor),
                            "IMPORTS platform symbol is empty",
                        ));
                    }
                    Some(symbol)
                } else {
                    None
                };
                let type_index = read_index(&bytes, &mut cursor)?;
                let required_entitlements = read_names(&bytes, &mut cursor, &strings, "@Requires")?;
                let implied_entitlements = read_names(&bytes, &mut cursor, &strings, "LibTessera")?;
                if (kind != BinaryImportKind::Ridl
                    || entity_kind != BinaryImportEntityKind::Function)
                    && (!required_entitlements.is_empty() || !implied_entitlements.is_empty())
                {
                    return Err(FormatError::malformed(
                        Some(cursor),
                        "entitlement metadata requires a RIDL function import",
                    ));
                }
                entities.push(BinaryImportedEntity {
                    kind: entity_kind,
                    name,
                    platform_symbol,
                    type_index,
                    required_entitlements,
                    implied_entitlements,
                });
            }
            imports.push(BinaryImport {
                kind,
                name,
                abi_uuid,
                weak: flags & 1 != 0,
                entities,
            });
        }

        if cursor != bytes.len() {
            return Err(FormatError::malformed(
                Some(cursor),
                "trailing bytes in IMPORTS section",
            ));
        }
        Ok(imports)
    }
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<String, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| FormatError::malformed(Some(offset), "IMPORTS string index overflow"))?;
    strings
        .get(index)
        .map(str::to_owned)
        .ok_or_else(|| FormatError::malformed(Some(offset), "IMPORTS string index is out of range"))
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

fn write_names(
    values: &[String],
    strings: &StringTable,
    label: &str,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(values.len(), bytes)?;
    let mut previous: Option<&str> = None;
    for value in values {
        if value.is_empty() || previous.is_some_and(|prior| prior.as_bytes() >= value.as_bytes()) {
            return Err(FormatError::malformed(
                None,
                format!("IMPORTS {label} entitlements must be nonempty and ordered"),
            ));
        }
        previous = Some(value);
        write_string(value, strings, bytes)?;
    }
    Ok(())
}

fn read_names(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    label: &str,
) -> Result<Vec<String>, FormatError> {
    let count = read_count(bytes, cursor, 1)?;
    let mut values = reserve(count, bytes, *cursor)?;
    for _ in 0..count {
        let name = read_string(bytes, cursor, strings)?;
        if name.is_empty()
            || values
                .last()
                .is_some_and(|previous: &String| previous.as_bytes() >= name.as_bytes())
        {
            return Err(FormatError::malformed(
                Some(*cursor),
                format!("IMPORTS {label} entitlements are empty or unordered"),
            ));
        }
        values.push(name);
    }
    Ok(values)
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
    read_uleb(bytes, cursor).map_err(|error| {
        FormatError::malformed(Some(offset), format!("invalid IMPORTS ULEB: {error:?}"))
    })
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_bytes: usize,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| FormatError::malformed(Some(offset), "IMPORTS count overflow"))?;
    if count > bytes.len().saturating_sub(*cursor) / minimum_bytes {
        return Err(FormatError::malformed(
            Some(offset),
            "IMPORTS count exceeds remaining section data",
        ));
    }
    Ok(count)
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let byte = *bytes
        .get(*cursor)
        .ok_or_else(|| FormatError::malformed(Some(*cursor), "truncated IMPORTS section"))?;
    *cursor += 1;
    Ok(byte)
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    if count > bytes.len() {
        return Err(FormatError::malformed(
            Some(offset),
            "IMPORTS count exceeds section size",
        ));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| FormatError::malformed(Some(offset), "IMPORTS allocation failed"))?;
    Ok(values)
}
