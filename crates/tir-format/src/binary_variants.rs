use std::collections::HashSet;

use crate::leb::{read_uleb, write_uleb};
use crate::types::{
    BinaryEngine, BinarySignatureTable, BinaryTarget, BinaryVariantMember, BinaryVariantSet,
};
use crate::{BinaryModule, FormatError, StringTable, StringTableBuilder};

/// Adds VARIANTS names and features to STRINGS in record order.
pub fn intern_variant_strings(
    sets: &[BinaryVariantSet],
    strings: &mut StringTableBuilder,
) -> Result<(), FormatError> {
    for set in sets {
        intern_nonempty(strings, &set.name, "variant-set name")?;
        for member in &set.members {
            for feature in &member.features {
                intern_nonempty(strings, feature, "variant feature")?;
            }
        }
    }
    Ok(())
}

/// Encodes VARIANTS and validates member/function correspondence.
pub fn encode_variant_sets(
    sets: &[BinaryVariantSet],
    strings: &StringTable,
    signatures: &BinarySignatureTable,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    write_count(&mut bytes, sets.len())?;
    let mut set_names = HashSet::new();
    let mut functions = HashSet::new();
    for set in sets {
        if set.name.is_empty() || !set_names.insert(&set.name) {
            return Err(malformed(0, "variant-set name is empty or duplicated"));
        }
        if set.members.is_empty() {
            return Err(malformed(0, "variant set has no members"));
        }
        write_string_index(&mut bytes, strings, &set.name)?;
        write_count(&mut bytes, set.members.len())?;
        let mut keys = HashSet::new();
        for member in &set.members {
            if member.architecture > 4
                || (member.architecture == 0 && !member.features.is_empty())
                || member.features.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(malformed(
                    0,
                    "invalid architecture or variant feature order",
                ));
            }
            if !keys.insert((member.architecture, &member.features)) {
                return Err(malformed(0, "duplicate architecture and feature variant"));
            }
            if !functions.insert(member.function_index) {
                return Err(malformed(
                    0,
                    "variant function belongs to more than one set",
                ));
            }
            let signature = signatures
                .records
                .get(
                    usize::try_from(member.function_index)
                        .map_err(|_| malformed(0, "variant function index overflows memory"))?,
                )
                .ok_or_else(|| malformed(0, "variant function index is invalid"))?;
            if signature.engine != Some(BinaryEngine::Native)
                || signature.target != BinaryTarget::Cpu
            {
                return Err(malformed(0, "variant member must be a native CPU function"));
            }
            let expected = if member.architecture == 0 {
                None
            } else {
                Some((member.architecture, member.features.as_slice()))
            };
            let actual = signature
                .variant
                .as_ref()
                .map(|variant| (variant.architecture, variant.features.as_slice()));
            if expected != actual {
                return Err(malformed(
                    0,
                    "variant member does not match its SIGNATURES variant attribute",
                ));
            }
            bytes.push(member.architecture);
            write_count(&mut bytes, member.features.len())?;
            for feature in &member.features {
                write_string_index(&mut bytes, strings, feature)?;
            }
            write_uleb(member.function_index, &mut bytes).map_err(io_error)?;
        }
    }
    Ok(bytes)
}

fn intern_nonempty(
    strings: &mut StringTableBuilder,
    value: &str,
    label: &str,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(malformed(0, format!("{label} is empty")));
    }
    strings
        .intern(value)
        .map(|_| ())
        .map_err(|error| malformed(0, error.message))
}

fn write_string_index(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    value: &str,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| malformed(0, "VARIANTS string is missing from STRINGS"))?;
    write_uleb(index, bytes).map_err(io_error)
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    let count =
        u64::try_from(count).map_err(|_| malformed(0, "VARIANTS count overflows ULEB128"))?;
    if count >= 1 << 32 {
        return Err(malformed(0, "VARIANTS count must be less than 2^32"));
    }
    write_uleb(count, bytes).map_err(io_error)
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(0, format!("VARIANTS encoding failed: {error}"))
}

impl BinaryModule {
    /// Decodes VARIANTS and checks member references against SIGNATURES.
    pub fn variant_sets(
        &self,
        memory_limit: u64,
        signatures: &BinarySignatureTable,
    ) -> Result<Vec<BinaryVariantSet>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 9) else {
            return Ok(Vec::new());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let count = read_count(&bytes, &mut cursor, 2, "variant set")?;
        let mut sets = reserve(count, &bytes, cursor, "variant set")?;
        let mut names = HashSet::new();
        let mut functions = HashSet::new();
        for _ in 0..count {
            let name_offset = cursor;
            let name = read_string(&bytes, &mut cursor, &strings, "variant-set name")?;
            if name.is_empty() {
                return Err(malformed(name_offset, "variant-set name is empty"));
            }
            if !names.insert(name.clone()) {
                return Err(malformed(name_offset, "duplicate variant-set name"));
            }
            let member_count_offset = cursor;
            let member_count = read_count(&bytes, &mut cursor, 3, "variant member")?;
            if member_count == 0 {
                return Err(malformed(member_count_offset, "variant set has no members"));
            }
            let mut members = reserve(member_count, &bytes, cursor, "variant member")?;
            let mut keys = HashSet::new();
            for _ in 0..member_count {
                let member_offset = cursor;
                let architecture = read_byte(&bytes, &mut cursor, "variant architecture")?;
                if architecture > 4 {
                    return Err(malformed(member_offset, "invalid variant architecture"));
                }
                let feature_count = read_count(&bytes, &mut cursor, 1, "variant feature")?;
                let mut features = reserve(feature_count, &bytes, cursor, "variant feature")?;
                let mut previous_feature: Option<String> = None;
                for _ in 0..feature_count {
                    let feature_offset = cursor;
                    let feature = read_string(&bytes, &mut cursor, &strings, "variant feature")?;
                    if feature.is_empty()
                        || previous_feature
                            .as_ref()
                            .is_some_and(|previous| feature <= *previous)
                    {
                        return Err(malformed(
                            feature_offset,
                            "variant features must be nonempty and strictly ordered",
                        ));
                    }
                    previous_feature = Some(feature.clone());
                    features.push(feature);
                }
                if architecture == 0 && !features.is_empty() {
                    return Err(malformed(
                        member_offset,
                        "portable variant cannot name architecture features",
                    ));
                }
                let function_offset = cursor;
                let function_index = read_index(&bytes, &mut cursor, "variant function index")?;
                let signature = signatures
                    .records
                    .get(usize::try_from(function_index).map_err(|_| {
                        malformed(function_offset, "variant function index overflows memory")
                    })?)
                    .ok_or_else(|| {
                        malformed(function_offset, "variant function index is invalid")
                    })?;
                if !functions.insert(function_index) {
                    return Err(malformed(
                        function_offset,
                        "variant function belongs to more than one set",
                    ));
                }
                if signature.engine != Some(BinaryEngine::Native)
                    || signature.target != BinaryTarget::Cpu
                {
                    return Err(malformed(
                        function_offset,
                        "variant member must be a native CPU function",
                    ));
                }
                let expected_variant = if architecture == 0 {
                    None
                } else {
                    Some((architecture, features.as_slice()))
                };
                let actual_variant = signature
                    .variant
                    .as_ref()
                    .map(|variant| (variant.architecture, variant.features.as_slice()));
                if actual_variant != expected_variant {
                    return Err(malformed(
                        function_offset,
                        "variant member does not match its SIGNATURES variant attribute",
                    ));
                }
                if !keys.insert((architecture, features.clone())) {
                    return Err(malformed(
                        member_offset,
                        "duplicate architecture and feature variant",
                    ));
                }
                members.push(BinaryVariantMember {
                    architecture,
                    features,
                    function_index,
                });
            }
            sets.push(BinaryVariantSet { name, members });
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in VARIANTS section"));
        }
        Ok(sets)
    }
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    label: &str,
) -> Result<String, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor, label)?)
        .map_err(|_| malformed(offset, format!("{label} index overflows memory")))?;
    strings
        .get(index)
        .map(str::to_owned)
        .ok_or_else(|| malformed(offset, format!("{label} index is out of range")))
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_record_size: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor, label)?)
        .map_err(|_| malformed(offset, format!("{label} count overflows memory")))?;
    if count as u64 >= 1 << 32 {
        return Err(malformed(
            offset,
            format!("{label} count must be less than 2^32"),
        ));
    }
    if count > bytes.len().saturating_sub(*cursor) / minimum_record_size {
        return Err(malformed(
            *cursor,
            format!("{label} count exceeds section size"),
        ));
    }
    Ok(count)
}

fn reserve<T>(
    count: usize,
    bytes: &[u8],
    offset: usize,
    label: &str,
) -> Result<Vec<T>, FormatError> {
    if count > bytes.len().saturating_sub(offset) {
        return Err(malformed(
            offset,
            format!("{label} count exceeds section size"),
        ));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, format!("{label} allocation failed")))?;
    Ok(values)
}

fn read_index(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor).map_err(|error| {
        malformed(
            offset,
            format!("invalid VARIANTS {label} ULEB128: {error:?}"),
        )
    })
}

fn read_byte(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, format!("truncated VARIANTS {label}")))?;
    *cursor += 1;
    Ok(value)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
