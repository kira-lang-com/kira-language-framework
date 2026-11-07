mod decode;
mod encode;
mod validate;

use crate::leb::IntegerImmediate;
use crate::{FormatError, StringTable, StringTableBuilder};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DebugSourceLocation {
    pub file_index: u64,
    pub line: u64,
    pub column: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugScopeKind {
    Function,
    Lexical,
    Inlined { call_site: DebugSourceLocation },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugScope {
    pub kind: DebugScopeKind,
    pub function_index: u64,
    pub compile_unit: u64,
    pub parent: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub data_index: u64,
    pub source_type: u64,
    pub source: DebugSourceLocation,
}

impl BinaryDebugTable {
    pub fn decode(bytes: &[u8], strings: &StringTable) -> Result<Self, FormatError> {
        let value = decode::decode(bytes, strings)?;
        validate::validate(&value)?;
        Ok(value)
    }

    pub fn encode(&self, strings: &StringTable) -> Result<Vec<u8>, FormatError> {
        validate::validate(self)?;
        encode_binary_debug_table(self, strings)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
            && self.compile_units.is_empty()
            && self.source_types.is_empty()
            && self.scopes.is_empty()
            && self.locations.is_empty()
            && self.variables.is_empty()
            && self.labels.is_empty()
            && self.data.is_empty()
    }

    pub fn intern_strings(&self, strings: &mut StringTableBuilder) -> Result<(), FormatError> {
        let mut intern = |value: &str| -> Result<(), FormatError> {
            if value.is_empty() {
                return Err(FormatError::malformed(
                    None,
                    "DEBUG string must not be empty",
                ));
            }
            strings
                .intern(value)
                .map(|_| ())
                .map_err(|e| FormatError::malformed(None, e.message))
        };
        for file in &self.files {
            intern(&file.path)?;
        }
        for unit in &self.compile_units {
            intern(&unit.language)?;
            intern(&unit.producer)?;
        }
        for ty in &self.source_types {
            match ty {
                DebugSourceType::Base { name, .. }
                | DebugSourceType::Typedef { name, .. }
                | DebugSourceType::TemplateParameter { name, .. } => intern(name)?,
                DebugSourceType::Composite { name, members, .. } => {
                    intern(name)?;
                    for member in members {
                        intern(&member.name)?;
                    }
                }
                DebugSourceType::Enum { name, variants, .. } => {
                    intern(name)?;
                    for variant in variants {
                        intern(&variant.name)?;
                    }
                }
                _ => {}
            }
        }
        for variable in &self.variables {
            intern(&variable.name)?;
        }
        for label in &self.labels {
            intern(&label.name)?;
        }
        Ok(())
    }
}

impl crate::BinaryModule {
    pub fn debug_table(&self, memory_limit: u64) -> Result<Option<BinaryDebugTable>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 17) else {
            return Ok(None);
        };
        let strings = self.string_table(memory_limit)?;
        BinaryDebugTable::decode(&section.read_uncompressed(memory_limit)?, &strings).map(Some)
    }
}
