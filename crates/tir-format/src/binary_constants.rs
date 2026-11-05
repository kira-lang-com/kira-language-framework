use crate::types::{BinaryConstant, BinaryConstantTable, BinaryTypeTable};
use crate::{BinaryModule, FormatError};

mod encode;
mod values;

impl BinaryModule {
    /// Decodes CONSTANTS using the already decoded type table.
    pub fn constant_table(
        &self,
        memory_limit: u64,
        types: &BinaryTypeTable,
    ) -> Result<BinaryConstantTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 5) else {
            return Ok(BinaryConstantTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let mut decoder = ConstantDecoder::new(&bytes, &strings, types);
        let count_offset = decoder.cursor;
        let count = read_count(&bytes, &mut decoder.cursor, 2, "constant")?;
        if count as u64 >= 1 << 32 {
            return Err(malformed(
                count_offset,
                "CONSTANTS count must be less than 2^32",
            ));
        }
        let mut records = reserve(count, &bytes, decoder.cursor)?;
        for _ in 0..count {
            let offset = decoder.cursor;
            let ty = read_index(&bytes, &mut decoder.cursor)?;
            if usize::try_from(ty).map_or(true, |index| index >= types.entries.len()) {
                return Err(malformed(offset, "constant type index is out of range"));
            }
            let value = decoder.read_value(ty, 0)?;
            records.push(BinaryConstant { ty, value });
        }
        if decoder.cursor != bytes.len() {
            return Err(malformed(
                decoder.cursor,
                "trailing bytes in CONSTANTS section",
            ));
        }
        Ok(BinaryConstantTable { records })
    }
}

struct ConstantDecoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
    strings: &'a crate::StringTable,
    types: &'a BinaryTypeTable,
}

impl<'a> ConstantDecoder<'a> {
    fn new(bytes: &'a [u8], strings: &'a crate::StringTable, types: &'a BinaryTypeTable) -> Self {
        Self {
            bytes,
            cursor: 0,
            strings,
            types,
        }
    }
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_record_bytes: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, format!("{label} count overflows memory")))?;
    if count > bytes.len().saturating_sub(*cursor) / minimum_record_bytes {
        return Err(malformed(
            offset,
            format!("{label} count exceeds remaining CONSTANTS data"),
        ));
    }
    Ok(count)
}

fn reserve<T>(count: usize, bytes: &[u8], offset: usize) -> Result<Vec<T>, FormatError> {
    if count > bytes.len() {
        return Err(malformed(offset, "CONSTANTS count exceeds section size"));
    }
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, "CONSTANTS allocation failed"))?;
    Ok(values)
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    let offset = *cursor;
    crate::leb::read_uleb(bytes, cursor)
        .map_err(|error| malformed(offset, format!("invalid CONSTANTS ULEB: {error:?}")))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
