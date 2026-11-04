use std::collections::HashSet;

use tir_format::StringTable;
use tir_format::leb::{read_uleb, write_uleb};

use crate::binary::FormatError;
use crate::text::{TextVariantArchitecture, TextVariantSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VariantIndex {
    /// `true` at each index that denotes a load-time dispatch set.
    pub dispatch: Vec<bool>,
    pub sets: Vec<VariantSetRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantSetRecord {
    pub name: String,
    /// 0 is a variant set; 1 is a dispatch set.
    pub kind: u8,
    pub members: Vec<VariantSetMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantSetMember {
    /// Architecture code: 0 portable, 1 aarch64, 2 x64, 3 rv64, 4 wasm.
    pub architecture: u8,
    pub features: Vec<String>,
    pub function_index: u64,
}

impl VariantIndex {
    /// Builds the binary VARIANTS index from parsed TVISA text declarations.
    pub fn from_text(
        sets: &[TextVariantSet],
        mut resolve_function: impl FnMut(&str) -> Option<u64>,
    ) -> Result<Self, FormatError> {
        let mut index = Self::default();
        index
            .sets
            .try_reserve_exact(sets.len())
            .map_err(|_| FormatError::malformed(None, "VARIANTS allocation failed"))?;
        index
            .dispatch
            .try_reserve_exact(sets.len())
            .map_err(|_| FormatError::malformed(None, "VARIANTS index allocation failed"))?;
        for set in sets {
            let kind = u8::from(set.dispatch);
            let mut members = Vec::new();
            members
                .try_reserve_exact(set.members.len())
                .map_err(|_| FormatError::malformed(None, "VARIANTS member allocation failed"))?;
            for member in &set.members {
                let architecture = match member.key.architecture {
                    TextVariantArchitecture::Portable => 0,
                    TextVariantArchitecture::Aarch64 => 1,
                    TextVariantArchitecture::X64 => 2,
                    TextVariantArchitecture::Rv64 => 3,
                    TextVariantArchitecture::Wasm => 4,
                };
                let function_index = resolve_function(&member.function).ok_or_else(|| {
                    FormatError::malformed(None, "VARIANTS member function is unresolved")
                })?;
                members.push(VariantSetMember {
                    architecture,
                    features: member.key.features.clone(),
                    function_index,
                });
            }
            index.dispatch.push(set.dispatch);
            index.sets.push(VariantSetRecord {
                name: set.name.clone(),
                kind,
                members,
            });
        }
        Ok(index)
    }

    pub fn encode(&self, strings: &StringTable) -> Result<Vec<u8>, FormatError> {
        if self.dispatch.len() != self.sets.len() {
            return Err(FormatError::malformed(
                None,
                "VARIANTS dispatch index does not match its records",
            ));
        }
        let count = u64::try_from(self.sets.len())
            .map_err(|_| FormatError::malformed(None, "VARIANTS count overflows"))?;
        let mut bytes = Vec::new();
        write_index(count, &mut bytes)?;
        let mut names = HashSet::new();
        let mut functions = HashSet::new();
        for (index, set) in self.sets.iter().enumerate() {
            if set.kind > 1 || self.dispatch[index] != (set.kind == 1) {
                return Err(FormatError::malformed(
                    None,
                    "invalid VARIANTS record kind or dispatch index",
                ));
            }
            if set.name.is_empty() || strings.index_of(&set.name).is_none() {
                return Err(FormatError::malformed(
                    None,
                    "VARIANTS name is empty or absent from STRINGS",
                ));
            }
            if !names.insert(set.name.as_str()) {
                return Err(FormatError::malformed(None, "duplicate VARIANTS name"));
            }
            if set.members.is_empty() {
                return Err(FormatError::malformed(
                    None,
                    "VARIANTS record has no members",
                ));
            }
            write_string(&set.name, strings, &mut bytes)?;
            bytes.push(set.kind);
            write_index(
                u64::try_from(set.members.len())
                    .map_err(|_| FormatError::malformed(None, "VARIANTS member count overflows"))?,
                &mut bytes,
            )?;
            let mut variants = HashSet::new();
            for member in &set.members {
                if member.architecture > 4 {
                    return Err(FormatError::malformed(None, "invalid variant architecture"));
                }
                if member.architecture == 0 && !member.features.is_empty() {
                    return Err(FormatError::malformed(
                        None,
                        "portable variant cannot name architecture features",
                    ));
                }
                if member.features.iter().any(String::is_empty)
                    || member.features.windows(2).any(|pair| pair[0] >= pair[1])
                {
                    return Err(FormatError::malformed(
                        None,
                        "variant features must be nonempty and strictly ordered",
                    ));
                }
                if !variants.insert((member.architecture, member.features.clone())) {
                    return Err(FormatError::malformed(
                        None,
                        "duplicate architecture and feature variant",
                    ));
                }
                if !functions.insert(member.function_index) {
                    return Err(FormatError::malformed(
                        None,
                        "variant function belongs to more than one set",
                    ));
                }
                bytes.push(member.architecture);
                write_index(
                    u64::try_from(member.features.len()).map_err(|_| {
                        FormatError::malformed(None, "VARIANTS feature count overflows")
                    })?,
                    &mut bytes,
                )?;
                for feature in &member.features {
                    write_string(feature, strings, &mut bytes)?;
                }
                write_index(member.function_index, &mut bytes)?;
            }
        }
        Ok(bytes)
    }
}

pub(crate) fn decode(bytes: &[u8], strings: &StringTable) -> Result<VariantIndex, FormatError> {
    let mut cursor = 0;
    let count = read_index(bytes, &mut cursor)?;
    let count = usize::try_from(count)
        .map_err(|_| malformed(cursor, "VARIANTS count does not fit in memory"))?;
    if count > bytes.len() {
        return Err(malformed(cursor, "VARIANTS count exceeds the section size"));
    }
    let mut index = VariantIndex::default();
    index
        .dispatch
        .try_reserve_exact(count)
        .map_err(|_| malformed(cursor, "VARIANTS index allocation failed"))?;
    index
        .sets
        .try_reserve_exact(count)
        .map_err(|_| malformed(cursor, "VARIANTS record allocation failed"))?;
    let mut names = HashSet::new();
    for _ in 0..count {
        let start = cursor;
        let name_index = read_index(bytes, &mut cursor)?;
        let name = string(strings, name_index, start)?;
        if !names.insert(name) {
            return Err(malformed(start, "duplicate VARIANTS name"));
        }
        let kind_offset = cursor;
        let kind = read_byte(bytes, &mut cursor)?;
        if kind > 1 {
            return Err(malformed(kind_offset, "invalid VARIANTS record kind"));
        }
        let member_count = read_index(bytes, &mut cursor)?;
        if member_count == 0 {
            return Err(malformed(cursor, "VARIANTS record has no members"));
        }
        if member_count > (bytes.len().saturating_sub(cursor) / 3) as u64 {
            return Err(malformed(
                cursor,
                "VARIANTS member count exceeds the section size",
            ));
        }
        let member_count = usize::try_from(member_count)
            .map_err(|_| malformed(cursor, "VARIANTS member count does not fit in memory"))?;
        let mut members = Vec::new();
        members
            .try_reserve_exact(member_count)
            .map_err(|_| malformed(cursor, "VARIANTS member allocation failed"))?;
        for _ in 0..member_count {
            let architecture_offset = cursor;
            let architecture = read_byte(bytes, &mut cursor)?;
            if architecture > 4 {
                return Err(malformed(
                    architecture_offset,
                    "invalid variant architecture",
                ));
            }
            let feature_count = read_index(bytes, &mut cursor)?;
            if feature_count > bytes.len() as u64 {
                return Err(malformed(
                    cursor,
                    "variant feature count exceeds the section size",
                ));
            }
            let feature_capacity = usize::try_from(feature_count)
                .map_err(|_| malformed(cursor, "variant feature count does not fit in memory"))?;
            let mut features = Vec::new();
            features
                .try_reserve_exact(feature_capacity)
                .map_err(|_| malformed(cursor, "variant feature allocation failed"))?;
            let mut seen_features = HashSet::new();
            for _ in 0..feature_count {
                let feature_index = read_index(bytes, &mut cursor)?;
                let feature = string(strings, feature_index, cursor)?;
                if !seen_features.insert(feature) {
                    return Err(malformed(cursor, "duplicate feature in variant member"));
                }
                features.push(feature.to_owned());
            }
            let function_index = read_index(bytes, &mut cursor)?;
            members.push(VariantSetMember {
                architecture,
                features,
                function_index,
            });
        }
        index.dispatch.push(kind == 1);
        index.sets.push(VariantSetRecord {
            name: name.to_owned(),
            kind,
            members,
        });
    }
    if cursor != bytes.len() {
        return Err(malformed(cursor, "trailing bytes in VARIANTS section"));
    }
    Ok(index)
}

fn string(strings: &StringTable, index: u64, offset: usize) -> Result<&str, FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| strings.get(index))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| malformed(offset, "VARIANTS string index is invalid"))
}

fn write_string(
    value: &str,
    strings: &StringTable,
    bytes: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| FormatError::malformed(None, "VARIANTS string is absent from STRINGS"))?;
    let index = u64::try_from(index)
        .map_err(|_| FormatError::malformed(None, "VARIANTS string index overflows"))?;
    write_index(index, bytes)
}

fn write_index(index: u64, bytes: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(index, bytes)
        .map_err(|error| FormatError::malformed(None, format!("VARIANTS encoding failed: {error}")))
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid VARIANTS ULEB128: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, "truncated VARIANTS record"))?;
    *cursor += 1;
    Ok(value)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
