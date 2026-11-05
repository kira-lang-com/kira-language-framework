use std::collections::HashSet;

use tir_format::StringTable;
use tir_format::leb::read_uleb;

use crate::binary::FormatError;

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
