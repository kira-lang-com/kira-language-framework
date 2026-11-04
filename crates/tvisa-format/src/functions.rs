use std::collections::HashSet;

use tir_format::StringTable;
use tir_format::leb::read_uleb;

use crate::binary::FormatError;

const ADDRESS_TAKEN: u64 = 1;
const ATTRIBUTE_MASK: u64 = (1 << 14) - 1;
const MAX_FRAME_SLOTS_PER_FUNCTION: u64 = 1 << 20;

mod encode;
pub use encode::encode_function_records;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionRecordSummary {
    pub name: String,
    pub signature_index: u64,
    pub linkage: u8,
    pub target: u8,
    pub attributes: u64,
    pub type_tag: Option<u32>,
    pub trust_class: u8,
    pub semantics: u8,
    pub personality: u8,
    pub alignment_log2: u8,
    pub placement: u8,
    pub slots: Vec<FrameSlotSummary>,
    pub debug_scope: u64,
    /// `None` is architecture code 255, which denotes an ordinary function.
    pub variant: Option<FunctionVariant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionVariant {
    /// Architecture code: 0 portable, 1 aarch64, 2 x64, 3 rv64, 4 wasm.
    pub architecture: u8,
    pub features: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameSlotSummary {
    pub size: u64,
    pub alignment_log2: u8,
    /// 0 CPU stack, 3 GPU threadgroup, 4 GPU thread.
    pub address_space: u8,
    pub flags: u8,
    pub debug_variable: u64,
}

impl FunctionRecordSummary {
    #[must_use]
    pub fn address_taken(&self) -> bool {
        self.attributes & ADDRESS_TAKEN != 0
    }
}

pub(crate) fn decode_summaries(
    bytes: &[u8],
    expected_count: u32,
    strings: &StringTable,
) -> Result<Vec<FunctionRecordSummary>, FormatError> {
    let expected_count = usize::try_from(expected_count)
        .map_err(|_| malformed(0, "function count does not fit in memory"))?;
    if expected_count > bytes.len() / 14 {
        return Err(malformed(
            0,
            "SIGNATURES function count exceeds the section size",
        ));
    }
    let mut cursor = 0;
    let mut records = Vec::new();
    records
        .try_reserve_exact(expected_count)
        .map_err(|_| malformed(0, "SIGNATURES allocation failed"))?;
    let mut names = HashSet::new();
    for _ in 0..expected_count {
        let start = cursor;
        let name_index = read_index(bytes, &mut cursor)?;
        let name = string(strings, name_index, start)?;
        if !names.insert(name) {
            return Err(malformed(start, "duplicate function name in SIGNATURES"));
        }
        let linkage_offset = cursor;
        let linkage = read_byte(bytes, &mut cursor)?;
        if linkage > 2 {
            return Err(malformed(linkage_offset, "invalid function linkage"));
        }
        let signature_index = read_index(bytes, &mut cursor)?;
        let target_offset = cursor;
        let target = read_byte(bytes, &mut cursor)?;
        if target > 2 {
            return Err(malformed(target_offset, "invalid function target kind"));
        }
        let attributes = read_index(bytes, &mut cursor)?;
        if attributes & !ATTRIBUTE_MASK != 0 {
            return Err(malformed(
                cursor,
                "reserved function attribute bits are set",
            ));
        }
        let type_tag = if attributes & ADDRESS_TAKEN != 0 {
            let tag = read_u32(bytes, &mut cursor)?;
            if forbidden_type_tag(tag) {
                return Err(malformed(cursor - 4, "function type tag is forbidden"));
            }
            Some(tag)
        } else {
            None
        };
        let variant = decode_variant(bytes, &mut cursor, strings)?;
        let trust_offset = cursor;
        let trust_class = read_byte(bytes, &mut cursor)?;
        if trust_class > 2 {
            return Err(malformed(trust_offset, "invalid function trust class"));
        }
        let semantics_offset = cursor;
        let semantics = read_byte(bytes, &mut cursor)?;
        if semantics > 1 {
            return Err(malformed(semantics_offset, "invalid function semantics"));
        }
        let personality_offset = cursor;
        let personality = read_byte(bytes, &mut cursor)?;
        if personality > 1 {
            return Err(malformed(
                personality_offset,
                "invalid function personality",
            ));
        }
        let alignment_offset = cursor;
        let alignment_log2 = read_byte(bytes, &mut cursor)?;
        if alignment_log2 < 4 {
            return Err(malformed(
                alignment_offset,
                "function alignment is below 16 bytes",
            ));
        }
        let placement_offset = cursor;
        let placement = read_byte(bytes, &mut cursor)?;
        if placement > 2 {
            return Err(malformed(placement_offset, "invalid function placement"));
        }
        let slots = decode_slots(bytes, &mut cursor)?;
        let debug_scope = read_index(bytes, &mut cursor)?;
        records.push(FunctionRecordSummary {
            name: name.to_owned(),
            signature_index,
            linkage,
            target,
            attributes,
            type_tag,
            trust_class,
            semantics,
            personality,
            alignment_log2,
            placement,
            slots,
            debug_scope,
            variant,
        });
    }
    if cursor != bytes.len() {
        return Err(malformed(cursor, "trailing bytes in SIGNATURES section"));
    }
    Ok(records)
}

fn decode_variant(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<Option<FunctionVariant>, FormatError> {
    let architecture_offset = *cursor;
    let architecture = read_byte(bytes, cursor)?;
    if architecture > 4 && architecture != u8::MAX {
        return Err(malformed(
            architecture_offset,
            "invalid function variant architecture",
        ));
    }
    let feature_count = read_index(bytes, cursor)?;
    if architecture == u8::MAX && feature_count != 0 {
        return Err(malformed(
            *cursor,
            "non-variant function has variant features",
        ));
    }
    if feature_count > bytes.len() as u64 {
        return Err(malformed(
            *cursor,
            "variant feature count exceeds the section size",
        ));
    }
    let feature_capacity = usize::try_from(feature_count).map_err(|_| {
        malformed(
            *cursor,
            "function variant feature count does not fit in memory",
        )
    })?;
    let mut features = Vec::new();
    features
        .try_reserve_exact(feature_capacity)
        .map_err(|_| malformed(*cursor, "function variant feature allocation failed"))?;
    let mut seen = HashSet::new();
    for _ in 0..feature_count {
        let feature_index = read_index(bytes, cursor)?;
        let feature = string(strings, feature_index, *cursor)?;
        if !seen.insert(feature) {
            return Err(malformed(*cursor, "duplicate function variant feature"));
        }
        features.push(feature.to_owned());
    }
    if architecture == u8::MAX {
        Ok(None)
    } else {
        Ok(Some(FunctionVariant {
            architecture,
            features,
        }))
    }
}

fn decode_slots(bytes: &[u8], cursor: &mut usize) -> Result<Vec<FrameSlotSummary>, FormatError> {
    let count = read_index(bytes, cursor)?;
    if count >= MAX_FRAME_SLOTS_PER_FUNCTION {
        return Err(malformed(
            *cursor,
            "frame slot count exceeds the TVISA limit",
        ));
    }
    if count > bytes.len() as u64 {
        return Err(malformed(
            *cursor,
            "frame slot count exceeds the section size",
        ));
    }
    let count = usize::try_from(count)
        .map_err(|_| malformed(*cursor, "frame slot count does not fit in memory"))?;
    if count > bytes.len().saturating_sub(*cursor) / 5 {
        return Err(malformed(
            *cursor,
            "frame slot count exceeds the section size",
        ));
    }
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(count)
        .map_err(|_| malformed(*cursor, "frame slot allocation failed"))?;
    for _ in 0..count {
        let size_offset = *cursor;
        let size = read_index(bytes, cursor)?;
        if size >= 1 << 31 {
            return Err(malformed(size_offset, "frame slot size exceeds its limit"));
        }
        let alignment_offset = *cursor;
        let alignment_log2 = read_byte(bytes, cursor)?;
        if alignment_log2 > 12 {
            return Err(malformed(
                alignment_offset,
                "frame slot alignment exceeds its limit",
            ));
        }
        let space_offset = *cursor;
        let space = read_byte(bytes, cursor)?;
        if !matches!(space, 0 | 3 | 4) {
            return Err(malformed(space_offset, "invalid frame slot address space"));
        }
        let flags_offset = *cursor;
        let flags = read_byte(bytes, cursor)?;
        if flags & !0x1f != 0 {
            return Err(malformed(flags_offset, "reserved frame slot flags are set"));
        }
        let debug_variable = read_index(bytes, cursor)?;
        slots.push(FrameSlotSummary {
            size,
            alignment_log2,
            address_space: space,
            flags,
            debug_variable,
        });
    }
    Ok(slots)
}

pub(crate) fn forbidden_type_tag(tag: u32) -> bool {
    tag & 0x000f_ffff == 0
        || tag & 0x0000_0fff == 0x017
        || tag == 0xfa1e_0ff3
        || tag.wrapping_neg() == 0xfa1e_0ff3
        || matches!(
            tag,
            0xd503_241f | 0xd503_245f | 0xd503_249f | 0xd503_24df | 0xd503_233f | 0xd503_237f
        )
}

pub fn is_valid_type_tag(tag: u32) -> bool {
    !forbidden_type_tag(tag)
}

fn string<'a>(strings: &'a StringTable, index: u64, offset: usize) -> Result<&'a str, FormatError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| strings.get(index))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| malformed(offset, "SIGNATURES string index is invalid"))
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid SIGNATURES ULEB128: {error:?}")))
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let offset = *cursor;
    let value = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, "truncated SIGNATURES record"))?;
    *cursor += 1;
    Ok(value)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, FormatError> {
    let offset = *cursor;
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(offset, "truncated function type tag"))?;
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..end]);
    *cursor = end;
    Ok(u32::from_le_bytes(value))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
