use std::collections::HashSet;
use std::io::{self, Write};

use tir_format::leb::{read_sleb, read_uleb, write_sleb, write_uleb};

use crate::binary::FormatError;
use tir_format::StringTable;

const MAX_DATA_OBJECTS_PER_IMAGE: u64 = 1 << 28;
const DATA_PERCPU_FLAG: u8 = 1 << 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DataLinkage {
    Local = 0,
    Image = 1,
    Exported = 2,
}

impl DataLinkage {
    fn decode(value: u8, offset: usize) -> Result<Self, FormatError> {
        match value {
            0 => Ok(Self::Local),
            1 => Ok(Self::Image),
            2 => Ok(Self::Exported),
            _ => Err(malformed(offset, "invalid DATA linkage")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DataKind {
    Readonly = 0,
    Relro = 1,
    Writable = 2,
    Zero = 3,
    Tls = 4,
    TlsZero = 5,
    GpuConstant = 6,
}

impl DataKind {
    fn decode(value: u8, offset: usize) -> Result<Self, FormatError> {
        match value {
            0 => Ok(Self::Readonly),
            1 => Ok(Self::Relro),
            2 => Ok(Self::Writable),
            3 => Ok(Self::Zero),
            4 => Ok(Self::Tls),
            5 => Ok(Self::TlsZero),
            6 => Ok(Self::GpuConstant),
            _ => Err(malformed(offset, "invalid DATA kind")),
        }
    }

    const fn has_contents(self) -> bool {
        !matches!(self, Self::Zero | Self::TlsZero)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DataRelocationKind {
    Abs64 = 0,
    Code64 = 1,
    Rel32 = 2,
    GpuEntry64 = 3,
    Tpoff64 = 4,
    Import64 = 5,
    ImportCode64 = 6,
    Dispatch64 = 7,
}

impl DataRelocationKind {
    fn decode(value: u8, offset: usize) -> Result<Self, FormatError> {
        match value {
            0 => Ok(Self::Abs64),
            1 => Ok(Self::Code64),
            2 => Ok(Self::Rel32),
            3 => Ok(Self::GpuEntry64),
            4 => Ok(Self::Tpoff64),
            5 => Ok(Self::Import64),
            6 => Ok(Self::ImportCode64),
            7 => Ok(Self::Dispatch64),
            _ => Err(malformed(offset, "invalid DATA relocation kind")),
        }
    }

    #[must_use]
    pub const fn width(self) -> u64 {
        if matches!(self, Self::Rel32) { 4 } else { 8 }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataRelocation {
    pub offset: u64,
    pub kind: DataRelocationKind,
    /// The symbol operand, with the low two bits encoding its index space.
    pub symbol: u64,
    pub addend: i64,
    pub tag: Option<u32>,
}

impl DataRelocation {
    #[must_use]
    pub const fn symbol_space(&self) -> u8 {
        (self.symbol & 3) as u8
    }

    #[must_use]
    pub const fn symbol_index(&self) -> u64 {
        self.symbol >> 2
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataObject {
    pub name_index: u64,
    pub linkage: DataLinkage,
    pub kind: DataKind,
    pub per_cpu: bool,
    pub alignment: u64,
    pub size: u64,
    /// Zero-initialized kinds have no payload; other kinds carry exactly `size` bytes.
    pub contents: Option<Vec<u8>>,
    pub debug_index: u64,
    pub relocations: Vec<DataRelocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataSection {
    pub objects: Vec<DataObject>,
}

impl DataSection {
    pub fn decode(bytes: &[u8], strings: &StringTable) -> Result<Self, FormatError> {
        let mut cursor = 0;
        let count = read_varint(bytes, &mut cursor)?;
        if count >= MAX_DATA_OBJECTS_PER_IMAGE {
            return Err(malformed(
                cursor,
                "DATA object count exceeds the TVISA limit",
            ));
        }
        let count = usize::try_from(count)
            .map_err(|_| malformed(cursor, "DATA object count does not fit in memory"))?;
        if count > bytes.len() {
            return Err(malformed(
                cursor,
                "DATA object count exceeds the section size",
            ));
        }
        let mut objects = Vec::new();
        let mut names = HashSet::new();
        objects
            .try_reserve_exact(count)
            .map_err(|_| malformed(cursor, "DATA object allocation failed"))?;
        for _ in 0..count {
            let object = decode_object(bytes, &mut cursor, strings)?;
            let name = strings
                .get(usize::try_from(object.name_index).unwrap_or(usize::MAX))
                .expect("decode_object validated the DATA name");
            if !names.insert(name) {
                return Err(malformed(cursor, "duplicate DATA name"));
            }
            objects.push(object);
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in DATA section"));
        }
        Ok(Self { objects })
    }

    pub fn encode(&self, strings: &StringTable) -> Result<Vec<u8>, FormatError> {
        let mut output = Vec::new();
        let mut names = HashSet::new();
        write_varint(self.objects.len() as u64, &mut output)?;
        for object in &self.objects {
            validate_object(object, strings)?;
            let name = strings
                .get(usize::try_from(object.name_index).unwrap_or(usize::MAX))
                .expect("validate_object checked the DATA name");
            if !names.insert(name) {
                return Err(FormatError::malformed(None, "duplicate DATA name"));
            }
            write_varint(object.name_index, &mut output)?;
            output.push(object.linkage as u8);
            output.push(object.kind as u8 | if object.per_cpu { DATA_PERCPU_FLAG } else { 0 });
            write_varint(object.alignment, &mut output)?;
            write_varint(object.size, &mut output)?;
            write_varint(object.debug_index, &mut output)?;
            if let Some(contents) = &object.contents {
                output.extend_from_slice(contents);
            }
            write_varint(object.relocations.len() as u64, &mut output)?;
            for relocation in &object.relocations {
                write_varint(relocation.offset, &mut output)?;
                output.push(relocation.kind as u8);
                write_varint(relocation.symbol, &mut output)?;
                write_signed(relocation.addend, &mut output)?;
                output.push(u8::from(relocation.tag.is_some()));
                if let Some(tag) = relocation.tag {
                    output.extend_from_slice(&tag.to_le_bytes());
                }
            }
        }
        Ok(output)
    }
}

fn decode_object(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<DataObject, FormatError> {
    let record_start = *cursor;
    let name_index = read_varint(bytes, cursor)?;
    let _name = usize::try_from(name_index)
        .ok()
        .and_then(|index| strings.get(index))
        .filter(|name| !name.is_empty())
        .ok_or_else(|| malformed(record_start, "DATA name is not a nonempty string"))?;
    let linkage_offset = *cursor;
    let linkage = DataLinkage::decode(read_byte(bytes, cursor)?, linkage_offset)?;
    let kind_offset = *cursor;
    let encoded_kind = read_byte(bytes, cursor)?;
    if encoded_kind & !0x0f != 0 {
        return Err(malformed(kind_offset, "reserved DATA kind bits are set"));
    }
    let per_cpu = encoded_kind & DATA_PERCPU_FLAG != 0;
    let kind = DataKind::decode(encoded_kind & !DATA_PERCPU_FLAG, kind_offset)?;
    let alignment_offset = *cursor;
    let alignment = read_varint(bytes, cursor)?;
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(malformed(
            alignment_offset,
            "DATA alignment must be a positive power of two",
        ));
    }
    let size = read_varint(bytes, cursor)?;
    let debug_index = read_varint(bytes, cursor)?;
    let contents = if kind.has_contents() {
        let length = usize::try_from(size)
            .map_err(|_| malformed(*cursor, "DATA initializer does not fit in memory"))?;
        let end = (*cursor)
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| malformed(*cursor, "truncated DATA initializer"))?;
        let contents = bytes[*cursor..end].to_vec();
        *cursor = end;
        Some(contents)
    } else {
        None
    };
    let relocation_count = read_varint(bytes, cursor)?;
    let relocation_count = usize::try_from(relocation_count)
        .map_err(|_| malformed(*cursor, "DATA relocation count does not fit in memory"))?;
    if relocation_count > bytes.len().saturating_sub(*cursor) / 4 {
        return Err(malformed(
            *cursor,
            "DATA relocation count exceeds the section size",
        ));
    }
    let mut relocations = Vec::new();
    relocations
        .try_reserve_exact(relocation_count)
        .map_err(|_| malformed(*cursor, "DATA relocation allocation failed"))?;
    for _ in 0..relocation_count {
        let relocation_start = *cursor;
        let offset = read_varint(bytes, cursor)?;
        let kind_offset = *cursor;
        let kind = DataRelocationKind::decode(read_byte(bytes, cursor)?, kind_offset)?;
        let symbol = read_varint(bytes, cursor)?;
        if symbol & 3 == 3 {
            return Err(malformed(
                *cursor,
                "DATA relocation uses the reserved symbol space",
            ));
        }
        let addend = read_signed(bytes, cursor)?;
        let flags_offset = *cursor;
        let flags = read_byte(bytes, cursor)?;
        if flags & !1 != 0 {
            return Err(malformed(
                flags_offset,
                "reserved DATA relocation flags are set",
            ));
        }
        let tag = if flags & 1 == 0 {
            None
        } else {
            Some(read_u32(bytes, cursor)?)
        };
        let end = offset
            .checked_add(kind.width())
            .ok_or_else(|| malformed(relocation_start, "DATA relocation range overflows"))?;
        if end > size {
            return Err(malformed(
                relocation_start,
                "DATA relocation extends beyond its object",
            ));
        }
        if kind != DataRelocationKind::Code64
            && kind != DataRelocationKind::ImportCode64
            && tag.is_some()
        {
            return Err(malformed(
                relocation_start,
                "only code-pointer relocations may carry a tag",
            ));
        }
        relocations.push(DataRelocation {
            offset,
            kind,
            symbol,
            addend,
            tag,
        });
    }
    validate_relocations(kind, &relocations, size, record_start)?;
    Ok(DataObject {
        name_index,
        linkage,
        kind,
        per_cpu,
        alignment,
        size,
        contents,
        debug_index,
        relocations,
    })
}

fn validate_object(object: &DataObject, strings: &StringTable) -> Result<(), FormatError> {
    let offset = usize::try_from(object.name_index).unwrap_or(usize::MAX);
    if strings
        .get(usize::try_from(object.name_index).unwrap_or(usize::MAX))
        .is_none_or(str::is_empty)
    {
        return Err(malformed(offset, "DATA name is not a nonempty string"));
    }
    if object.alignment == 0 || !object.alignment.is_power_of_two() {
        return Err(malformed(
            offset,
            "DATA alignment must be a positive power of two",
        ));
    }
    match (object.kind.has_contents(), object.contents.as_ref()) {
        (true, Some(contents)) if contents.len() as u64 == object.size => {}
        (false, None) => {}
        _ => {
            return Err(malformed(
                offset,
                "DATA initializer does not match its kind and size",
            ));
        }
    }
    validate_relocations(object.kind, &object.relocations, object.size, offset)
}

fn validate_relocations(
    data_kind: DataKind,
    relocations: &[DataRelocation],
    size: u64,
    offset: usize,
) -> Result<(), FormatError> {
    let mut ranges = Vec::new();
    ranges
        .try_reserve_exact(relocations.len())
        .map_err(|_| malformed(offset, "DATA relocation validation allocation failed"))?;
    for relocation in relocations {
        if relocation.symbol & 3 == 3 {
            return Err(malformed(
                offset,
                "DATA relocation uses the reserved symbol space",
            ));
        }
        if relocation.kind != DataRelocationKind::Code64
            && relocation.kind != DataRelocationKind::ImportCode64
            && relocation.tag.is_some()
        {
            return Err(malformed(
                offset,
                "only code-pointer relocations may carry a tag",
            ));
        }
        if data_kind == DataKind::Readonly && relocation.kind != DataRelocationKind::Rel32 {
            return Err(malformed(
                offset,
                "read-only DATA permits only REL32 relocations",
            ));
        }
        let end = relocation
            .offset
            .checked_add(relocation.kind.width())
            .filter(|end| *end <= size)
            .ok_or_else(|| malformed(offset, "DATA relocation extends beyond its object"))?;
        if ranges.iter().any(|(start, previous_end): &(u64, u64)| {
            relocation.offset < *previous_end && *start < end
        }) {
            return Err(malformed(offset, "DATA relocations overlap"));
        }
        ranges.push((relocation.offset, end));
    }
    Ok(())
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let offset = *cursor;
    let byte = *bytes
        .get(offset)
        .ok_or_else(|| malformed(offset, "truncated DATA record"))?;
    *cursor += 1;
    Ok(byte)
}

fn read_varint(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid DATA ULEB128: {error:?}")))
}

fn read_signed(bytes: &[u8], cursor: &mut usize) -> Result<i64, FormatError> {
    let offset = *cursor;
    read_sleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid DATA SLEB128: {error:?}")))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, FormatError> {
    let offset = *cursor;
    let end = offset
        .checked_add(4)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| malformed(offset, "truncated DATA relocation tag"))?;
    let mut value = [0; 4];
    value.copy_from_slice(&bytes[offset..end]);
    *cursor = end;
    Ok(u32::from_le_bytes(value))
}

fn write_varint(value: u64, output: &mut impl Write) -> Result<(), FormatError> {
    write_uleb(value, output).map_err(io_error)
}

fn write_signed(value: i64, output: &mut impl Write) -> Result<(), FormatError> {
    write_sleb(value, output).map_err(io_error)
}

fn io_error(error: io::Error) -> FormatError {
    FormatError::malformed(None, error.to_string())
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
