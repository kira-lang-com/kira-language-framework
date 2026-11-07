use crate::leb::{read_integer_immediate, read_uleb};
use crate::{FormatError, StringTable};

use super::{
    BinaryDebugTable, DebugCompileUnit, DebugDataRecord, DebugEnumerator, DebugFileRecord,
    DebugLabel, DebugLocation, DebugScope, DebugScopeKind, DebugSourceLocation, DebugSourceType,
    DebugTypeMember, DebugVariable,
};

struct Reader<'a> {
    bytes: &'a [u8],
    strings: &'a StringTable,
    at: usize,
}

impl Reader<'_> {
    fn index(&mut self) -> Result<u64, FormatError> {
        read_uleb(self.bytes, &mut self.at)
            .map_err(|e| self.error(format!("invalid DEBUG ULEB: {e:?}")))
    }

    fn number(&mut self) -> Result<crate::leb::IntegerImmediate, FormatError> {
        read_integer_immediate(self.bytes, &mut self.at)
            .map_err(|e| self.error(format!("invalid DEBUG enum immediate: {e:?}")))
    }

    fn byte(&mut self) -> Result<u8, FormatError> {
        let byte = self
            .bytes
            .get(self.at)
            .copied()
            .ok_or_else(|| self.error("truncated DEBUG byte"))?;
        self.at += 1;
        Ok(byte)
    }

    fn string(&mut self) -> Result<String, FormatError> {
        let index = usize::try_from(self.index()?)
            .map_err(|_| self.error("DEBUG string index overflows"))?;
        let value = self
            .strings
            .get(index)
            .ok_or_else(|| self.error("DEBUG string index is invalid"))?;
        if value.is_empty() {
            return Err(self.error("DEBUG string must be nonempty"));
        }
        Ok(value.to_owned())
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], FormatError> {
        let end = self
            .at
            .checked_add(N)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| self.error("truncated DEBUG bytes"))?;
        let mut value = [0u8; N];
        value.copy_from_slice(&self.bytes[self.at..end]);
        self.at = end;
        Ok(value)
    }

    fn utf8(&mut self) -> Result<String, FormatError> {
        let length = usize::try_from(self.index()?)
            .map_err(|_| self.error("DEBUG text length overflows"))?;
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| self.error("truncated DEBUG generated source"))?;
        let value = std::str::from_utf8(&self.bytes[self.at..end])
            .map_err(|_| self.error("DEBUG generated source is invalid UTF-8"))?
            .to_owned();
        self.at = end;
        Ok(value)
    }

    fn count(&mut self) -> Result<usize, FormatError> {
        let number = usize::try_from(self.index()?)
            .map_err(|_| self.error("DEBUG table count overflows"))?;
        if number > self.bytes.len().saturating_sub(self.at) {
            return Err(self.error("DEBUG count exceeds remaining section bytes"));
        }
        Ok(number)
    }

    fn records<T>(
        &mut self,
        mut parse: impl FnMut(&mut Self) -> Result<T, FormatError>,
    ) -> Result<Vec<T>, FormatError> {
        let count = self.count()?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| self.error("DEBUG allocation failed"))?;
        for _ in 0..count {
            values.push(parse(self)?);
        }
        Ok(values)
    }

    fn flag(&mut self) -> Result<bool, FormatError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(self.error("invalid DEBUG boolean flag")),
        }
    }

    fn source(&mut self) -> Result<DebugSourceLocation, FormatError> {
        Ok(DebugSourceLocation {
            file_index: self.index()?,
            line: self.index()?,
            column: self.index()?,
        })
    }

    fn error(&self, message: impl Into<String>) -> FormatError {
        FormatError::malformed(Some(self.at), message)
    }
}

pub(super) fn decode(bytes: &[u8], strings: &StringTable) -> Result<BinaryDebugTable, FormatError> {
    let mut r = Reader {
        bytes,
        strings,
        at: 0,
    };
    let files = r.records(|r| {
        let path = r.string()?;
        let flags = r.byte()?;
        if flags & !3 != 0 {
            return Err(r.error("invalid DEBUG file flags"));
        }
        Ok(DebugFileRecord {
            path,
            content_hash: if flags & 1 != 0 {
                Some(r.bytes()?)
            } else {
                None
            },
            generated_text: if flags & 2 != 0 {
                Some(r.utf8()?)
            } else {
                None
            },
        })
    })?;
    let compile_units = r.records(|r| {
        Ok(DebugCompileUnit {
            language: r.string()?,
            producer: r.string()?,
        })
    })?;
    let source_types = r.records(|r| {
        Ok(match r.byte()? {
            0 => DebugSourceType::Base {
                name: r.string()?,
                encoding: r.byte()?,
                bit_width: r.index()?,
            },
            1 => DebugSourceType::Composite {
                name: r.string()?,
                kind: r.byte()?,
                size: r.index()?,
                alignment: r.index()?,
                members: r.records(|r| {
                    Ok(DebugTypeMember {
                        name: r.string()?,
                        type_index: r.index()?,
                        byte_offset: r.index()?,
                    })
                })?,
            },
            2 => DebugSourceType::Enum {
                name: r.string()?,
                underlying_type: r.index()?,
                variants: r.records(|r| {
                    Ok(DebugEnumerator {
                        name: r.string()?,
                        value: r.number()?,
                    })
                })?,
            },
            3 => DebugSourceType::Pointer {
                pointee_type: r.index()?,
            },
            4 => DebugSourceType::Reference {
                pointee_type: r.index()?,
                mutable: r.flag()?,
            },
            5 => DebugSourceType::Typedef {
                name: r.string()?,
                type_index: r.index()?,
            },
            6 => {
                let name = r.string()?;
                let kind = r.byte()?;
                DebugSourceType::TemplateParameter {
                    name,
                    kind,
                    value_type: if kind == 1 { Some(r.index()?) } else { None },
                }
            }
            7 => DebugSourceType::Array {
                element_type: r.index()?,
                count: if r.flag()? { Some(r.index()?) } else { None },
            },
            8 => DebugSourceType::Qualified {
                type_index: r.index()?,
                qualifiers: r.byte()?,
            },
            _ => return Err(r.error("unknown DEBUG source type tag")),
        })
    })?;
    let scopes = r.records(|r| {
        let kind = r.byte()?;
        let function_index = r.index()?;
        let compile_unit = r.index()?;
        let parent = r.index()?.checked_sub(1);
        Ok(DebugScope {
            kind: match kind {
                0 => DebugScopeKind::Function,
                1 => DebugScopeKind::Lexical,
                2 => DebugScopeKind::Inlined {
                    call_site: r.source()?,
                },
                _ => return Err(r.error("unknown DEBUG scope kind")),
            },
            function_index,
            compile_unit,
            parent,
        })
    })?;
    let mut previous_function = None;
    let mut previous_ordinal = 0u64;
    let locations = r.records(|r| {
        let function_index = r.index()?;
        let delta = r.index()?;
        let instruction_ordinal = if previous_function == Some(function_index) {
            previous_ordinal
                .checked_add(delta)
                .ok_or_else(|| r.error("DEBUG location ordinal overflows"))?
        } else {
            delta
        };
        let source = r.source()?;
        let scope = r.index()?.checked_sub(1);
        previous_function = Some(function_index);
        previous_ordinal = instruction_ordinal;
        Ok(DebugLocation {
            function_index,
            instruction_ordinal,
            source,
            scope,
        })
    })?;
    let variables = r.records(|r| {
        Ok(DebugVariable {
            name: r.string()?,
            source_type: r.index()?,
            scope: r.index()?,
        })
    })?;
    let labels = r.records(|r| {
        Ok(DebugLabel {
            name: r.string()?,
            scope: r.index()?,
            source: r.source()?,
        })
    })?;
    let data = r.records(|r| {
        Ok(DebugDataRecord {
            data_index: r.index()?,
            source_type: r.index()?,
            source: r.source()?,
        })
    })?;
    if r.at != bytes.len() {
        return Err(r.error("trailing bytes after DEBUG tables"));
    }
    Ok(BinaryDebugTable {
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
