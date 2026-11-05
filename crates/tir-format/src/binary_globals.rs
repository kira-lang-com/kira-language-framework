use std::collections::HashSet;

use crate::leb::{LebError, read_uleb};
use crate::types::{
    BinaryAddressSpace, BinaryConstantTable, BinaryGlobal, BinaryGlobalTable, BinaryLinkage,
    BinaryTypeTable,
};
use crate::{BinaryModule, FormatError};

mod encode;

const MAX_GLOBAL_COUNT: u64 = 1 << 32;

impl BinaryModule {
    /// Decodes GLOBALS and checks references into TYPES and CONSTANTS.
    pub fn global_table(
        &self,
        memory_limit: u64,
        types: &BinaryTypeTable,
        constants: &BinaryConstantTable,
    ) -> Result<BinaryGlobalTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 6) else {
            return Ok(BinaryGlobalTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut cursor = 0;
        let count_offset = cursor;
        let count = read_count(&bytes, &mut cursor)?;
        if count as u64 >= MAX_GLOBAL_COUNT {
            return Err(malformed(
                count_offset,
                "GLOBALS count must be less than 2^32",
            ));
        }
        let mut records = reserve(count, &bytes, cursor)?;
        let mut names = HashSet::new();
        names
            .try_reserve(count)
            .map_err(|_| malformed(cursor, "GLOBALS name allocation failed"))?;
        for _ in 0..count {
            let name_offset = cursor;
            let name = read_string(&bytes, &mut cursor, &strings)?;
            if name.is_empty() {
                return Err(malformed(name_offset, "global name is empty"));
            }
            if !names.insert(name.clone()) {
                return Err(malformed(name_offset, "duplicate global name"));
            }
            let linkage = match read_byte(&bytes, &mut cursor)? {
                0 => BinaryLinkage::Private,
                1 => BinaryLinkage::Module,
                2 => BinaryLinkage::Package,
                3 => BinaryLinkage::Exported,
                _ => return Err(malformed(cursor - 1, "invalid global linkage")),
            };
            let type_offset = cursor;
            let ty = read_index(&bytes, &mut cursor)?;
            if usize::try_from(ty).map_or(true, |index| index >= types.entries.len()) {
                return Err(malformed(type_offset, "global type index is out of range"));
            }
            let address_space = match read_byte(&bytes, &mut cursor)? {
                0 => BinaryAddressSpace::Generic,
                1 => BinaryAddressSpace::Device,
                2 => BinaryAddressSpace::Constant,
                3 => BinaryAddressSpace::Threadgroup,
                4 => BinaryAddressSpace::Thread,
                5 => BinaryAddressSpace::ObjectPayload,
                6 => BinaryAddressSpace::RayPayload,
                7 => BinaryAddressSpace::HitAttribute,
                8 => BinaryAddressSpace::Tile,
                9 => BinaryAddressSpace::NodeRecord,
                _ => return Err(malformed(cursor - 1, "invalid global address space")),
            };
            let flags = read_byte(&bytes, &mut cursor)?;
            if flags & !0x07 != 0 {
                return Err(malformed(cursor - 1, "invalid global flags"));
            }
            let alignment_offset = cursor;
            let alignment = read_index(&bytes, &mut cursor)?;
            let alignment = if alignment == 0 {
                None
            } else if alignment.is_power_of_two() {
                Some(alignment)
            } else {
                return Err(malformed(
                    alignment_offset,
                    "global alignment must be a power of two",
                ));
            };
            let initializer_offset = cursor;
            let initializer = read_index(&bytes, &mut cursor)?;
            let initializer = if initializer == 0 {
                None
            } else {
                let index = initializer - 1;
                let constant = constants
                    .records
                    .get(usize::try_from(index).map_err(|_| {
                        malformed(
                            initializer_offset,
                            "global initializer index overflows memory",
                        )
                    })?)
                    .ok_or_else(|| {
                        malformed(
                            initializer_offset,
                            "global initializer index is out of range",
                        )
                    })?;
                if constant.ty != ty {
                    return Err(malformed(
                        initializer_offset,
                        "global initializer type does not match its declared type",
                    ));
                }
                Some(index)
            };
            records.push(BinaryGlobal {
                name,
                linkage,
                ty,
                address_space,
                thread_local: flags & 1 != 0,
                constant: flags & 2 != 0,
                per_cpu: flags & 4 != 0,
                alignment,
                initializer,
            });
        }
        if cursor != bytes.len() {
            return Err(malformed(cursor, "trailing bytes in GLOBALS section"));
        }
        Ok(BinaryGlobalTable { records })
    }
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &crate::StringTable,
) -> Result<String, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, "global name index overflows memory"))?;
    strings
        .get(index)
        .map(str::to_owned)
        .ok_or_else(|| malformed(offset, "global name index is out of range"))
}

fn read_count(bytes: &[u8], cursor: &mut usize) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, "GLOBALS count overflows memory"))?;
    if count > bytes.len().saturating_sub(*cursor) / 7 {
        return Err(malformed(*cursor, "GLOBALS count exceeds section size"));
    }
    Ok(count)
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, "GLOBALS allocation failed"))?;
    if count > bytes.len() {
        return Err(malformed(offset, "GLOBALS count exceeds section size"));
    }
    Ok(values)
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    read_uleb(bytes, cursor).map_err(|error| map_leb_error(error, *cursor))
}

fn read_byte(bytes: &[u8], cursor: &mut usize) -> Result<u8, FormatError> {
    let byte = *bytes
        .get(*cursor)
        .ok_or_else(|| malformed(*cursor, "truncated GLOBALS section"))?;
    *cursor += 1;
    Ok(byte)
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}

fn map_leb_error(error: LebError, offset: usize) -> FormatError {
    malformed(offset, format!("invalid GLOBALS ULEB: {error:?}"))
}
