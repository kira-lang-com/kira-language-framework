use crate::leb::{LebError, read_uleb, write_uleb};
use crate::{BinaryModule, FormatError, SemanticsMode, StringTable, StringTableBuilder};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryProducer {
    pub name: String,
    pub version: String,
    pub languages: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryInitializer {
    pub function_index: u64,
    pub priority: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryModuleRecord {
    pub name: String,
    pub producer: Option<BinaryProducer>,
    pub semantics: SemanticsMode,
    pub attributes: Vec<(String, String)>,
    pub initializers: Vec<BinaryInitializer>,
    pub finalizers: Vec<BinaryInitializer>,
    pub source_hash: Option<[u8; 32]>,
}

impl BinaryModuleRecord {
    /// Adds MODULE strings in their section traversal order to the canonical table.
    pub fn intern_strings(&self, strings: &mut StringTableBuilder) -> Result<(), FormatError> {
        intern_string(strings, &self.name)?;
        if let Some(producer) = &self.producer {
            intern_string(strings, &producer.name)?;
            intern_string(strings, &producer.version)?;
            for language in &producer.languages {
                intern_string(strings, language)?;
            }
        }
        for (key, value) in &self.attributes {
            intern_string(strings, key)?;
            intern_string(strings, value)?;
        }
        Ok(())
    }

    /// Encodes the MODULE payload using indices from the canonical STRINGS table.
    pub fn encode(
        &self,
        strings: &StringTable,
        profile: tir_target::Profile,
    ) -> Result<Vec<u8>, FormatError> {
        if self.name.is_empty() {
            return Err(FormatError::malformed(None, "module name is empty"));
        }
        match (profile, self.source_hash.is_some()) {
            (tir_target::Profile::Driver, true) => {}
            (tir_target::Profile::Driver, false) => {
                return Err(FormatError::malformed(
                    None,
                    "driver MODULE record is missing its source hash",
                ));
            }
            (_, true) => {
                return Err(FormatError::malformed(
                    None,
                    "non-driver MODULE record cannot contain a source hash",
                ));
            }
            (_, false) => {}
        }

        let mut bytes = Vec::new();
        write_string_index(&mut bytes, strings, &self.name)?;
        if let Some(producer) = &self.producer {
            bytes.push(1);
            write_string_index(&mut bytes, strings, &producer.name)?;
            write_string_index(&mut bytes, strings, &producer.version)?;
            write_count(&mut bytes, producer.languages.len())?;
            for language in &producer.languages {
                write_string_index(&mut bytes, strings, language)?;
            }
        } else {
            bytes.push(0);
        }
        bytes.push(match self.semantics {
            SemanticsMode::Defined => 0,
            SemanticsMode::Permissive => 1,
        });
        write_count(&mut bytes, self.attributes.len())?;
        for (key, value) in &self.attributes {
            write_string_index(&mut bytes, strings, key)?;
            write_string_index(&mut bytes, strings, value)?;
        }
        write_initializers(&mut bytes, &self.initializers)?;
        write_initializers(&mut bytes, &self.finalizers)?;
        if let Some(source_hash) = self.source_hash {
            bytes.extend_from_slice(&source_hash);
        }
        Ok(bytes)
    }
}

fn intern_string(strings: &mut StringTableBuilder, value: &str) -> Result<(), FormatError> {
    strings
        .intern(value)
        .map(|_| ())
        .map_err(|error| FormatError::malformed(None, error.message))
}

fn write_string_index(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    value: &str,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| FormatError::malformed(None, "MODULE string is missing from STRINGS"))?;
    write_uleb(index, bytes).map_err(|error| FormatError::malformed(None, error.to_string()))
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    let count = u64::try_from(count)
        .map_err(|_| FormatError::malformed(None, "MODULE count does not fit in ULEB128"))?;
    write_uleb(count, bytes).map_err(|error| FormatError::malformed(None, error.to_string()))
}

fn write_initializers(
    bytes: &mut Vec<u8>,
    initializers: &[BinaryInitializer],
) -> Result<(), FormatError> {
    write_count(bytes, initializers.len())?;
    for initializer in initializers {
        write_uleb(initializer.function_index, bytes)
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
        bytes.extend_from_slice(&initializer.priority.to_le_bytes());
    }
    Ok(())
}

impl BinaryModule {
    pub fn module_record(&self, memory_limit: u64) -> Result<BinaryModuleRecord, FormatError> {
        let strings = self.string_table(memory_limit)?;
        let section = self
            .sections
            .iter()
            .find(|section| section.kind == 2)
            .ok_or_else(|| FormatError::malformed(None, "MODULE section is missing"))?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;

        let name = read_string(&bytes, &mut cursor, &strings)?;
        if name.is_empty() {
            return Err(FormatError::malformed(Some(cursor), "module name is empty"));
        }

        let producer = match read_byte(&bytes, &mut cursor)? {
            0 => None,
            1 => {
                let name = read_string(&bytes, &mut cursor, &strings)?;
                let version = read_string(&bytes, &mut cursor, &strings)?;
                let count = read_count(&bytes, &mut cursor, 1)?;
                let mut languages = reserve(count, &bytes, cursor)?;
                for _ in 0..count {
                    languages.push(read_string(&bytes, &mut cursor, &strings)?);
                }
                Some(BinaryProducer {
                    name,
                    version,
                    languages,
                })
            }
            _ => {
                return Err(FormatError::malformed(
                    Some(cursor.saturating_sub(1)),
                    "invalid producer-present flag",
                ));
            }
        };

        let semantics = match read_byte(&bytes, &mut cursor)? {
            0 => SemanticsMode::Defined,
            1 => SemanticsMode::Permissive,
            _ => {
                return Err(FormatError::malformed(
                    Some(cursor.saturating_sub(1)),
                    "invalid default-semantics code",
                ));
            }
        };

        let count = read_count(&bytes, &mut cursor, 2)?;
        let mut attributes = reserve(count, &bytes, cursor)?;
        for _ in 0..count {
            let key = read_string(&bytes, &mut cursor, &strings)?;
            let value = read_string(&bytes, &mut cursor, &strings)?;
            attributes.push((key, value));
        }

        let initializers = read_initializers(&bytes, &mut cursor)?;
        let finalizers = read_initializers(&bytes, &mut cursor)?;
        let source_hash = if self.header.profile == tir_target::Profile::Driver {
            let end = cursor
                .checked_add(32)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| FormatError::malformed(Some(cursor), "truncated source hash"))?;
            let mut hash = [0; 32];
            hash.copy_from_slice(&bytes[cursor..end]);
            cursor = end;
            Some(hash)
        } else {
            None
        };

        if cursor != bytes.len() {
            return Err(FormatError::malformed(
                Some(cursor),
                "trailing bytes in MODULE section",
            ));
        }

        Ok(BinaryModuleRecord {
            name,
            producer,
            semantics,
            attributes,
            initializers,
            finalizers,
            source_hash,
        })
    }
}

fn read_initializers(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<Vec<BinaryInitializer>, FormatError> {
    let count = read_count(bytes, cursor, 3)?;
    let mut initializers = reserve(count, bytes, *cursor)?;
    for _ in 0..count {
        let function_index = read_index(bytes, cursor)?;
        let end = cursor
            .checked_add(2)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| {
                FormatError::malformed(Some(*cursor), "truncated initializer priority")
            })?;
        let priority = u16::from_le_bytes([bytes[*cursor], bytes[*cursor + 1]]);
        *cursor = end;
        initializers.push(BinaryInitializer {
            function_index,
            priority,
        });
    }
    Ok(initializers)
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &crate::StringTable,
) -> Result<String, FormatError> {
    let index = usize::try_from(read_index(bytes, cursor)?).map_err(|_| {
        FormatError::malformed(Some(*cursor), "string index does not fit in memory")
    })?;
    strings
        .get(index)
        .map(str::to_owned)
        .ok_or_else(|| FormatError::malformed(Some(*cursor), "string index is out of range"))
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    read_uleb(bytes, cursor).map_err(|error| map_leb_error(error, *cursor))
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_bytes: usize,
) -> Result<usize, FormatError> {
    let count = usize::try_from(read_index(bytes, cursor)?).map_err(|_| {
        FormatError::malformed(Some(*cursor), "record count does not fit in memory")
    })?;
    let remaining = bytes.len().saturating_sub(*cursor);
    if minimum_bytes == 0 || count > remaining / minimum_bytes {
        return Err(FormatError::malformed(
            Some(*cursor),
            "record count exceeds the remaining section data",
        ));
    }
    Ok(count)
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let byte = *bytes
        .get(*cursor)
        .ok_or_else(|| FormatError::malformed(Some(*cursor), "truncated MODULE section"))?;
    *cursor += 1;
    Ok(byte)
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| FormatError::malformed(Some(offset), "MODULE record allocation failed"))?;
    if count > bytes.len() {
        return Err(FormatError::malformed(
            Some(offset),
            "MODULE record count exceeds the section size",
        ));
    }
    Ok(values)
}

fn map_leb_error(error: LebError, offset: usize) -> FormatError {
    FormatError::malformed(Some(offset), format!("invalid MODULE ULEB: {error:?}"))
}
