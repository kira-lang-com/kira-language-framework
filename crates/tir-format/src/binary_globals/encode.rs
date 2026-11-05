use std::collections::HashSet;

use crate::leb::write_uleb;
use crate::types::{BinaryAddressSpace, BinaryGlobalTable, BinaryLinkage, BinaryTypeTable};
use crate::{BinaryConstantTable, FormatError, StringTable, StringTableBuilder};

impl BinaryGlobalTable {
    /// Adds GLOBALS names to STRINGS in record order.
    pub fn intern_strings(&self, strings: &mut StringTableBuilder) -> Result<(), FormatError> {
        for global in &self.records {
            if global.name.is_empty() {
                return Err(malformed("global name is empty"));
            }
            strings
                .intern(&global.name)
                .map_err(|error| malformed(error.message))?;
        }
        Ok(())
    }

    /// Encodes GLOBALS and validates references to TYPES and CONSTANTS.
    pub fn encode(
        &self,
        strings: &StringTable,
        types: &BinaryTypeTable,
        constants: &BinaryConstantTable,
    ) -> Result<Vec<u8>, FormatError> {
        if self.records.len() as u64 >= 1 << 32 {
            return Err(malformed("GLOBALS count must be less than 2^32"));
        }
        let mut names = HashSet::new();
        let mut bytes = Vec::new();
        write_count(&mut bytes, self.records.len())?;
        for global in &self.records {
            if global.name.is_empty() || !names.insert(&global.name) {
                return Err(malformed("global name is empty or duplicated"));
            }
            if types
                .entries
                .get(
                    usize::try_from(global.ty)
                        .map_err(|_| malformed("global type index overflows memory"))?,
                )
                .is_none()
            {
                return Err(malformed("global type index is out of range"));
            }
            if let Some(initializer) = global.initializer {
                let constant = constants
                    .records
                    .get(
                        usize::try_from(initializer)
                            .map_err(|_| malformed("initializer index overflows memory"))?,
                    )
                    .ok_or_else(|| malformed("global initializer index is out of range"))?;
                if constant.ty != global.ty {
                    return Err(malformed(
                        "global initializer type does not match its declared type",
                    ));
                }
            }
            if global
                .alignment
                .is_some_and(|alignment| !alignment.is_power_of_two())
            {
                return Err(malformed("global alignment must be a power of two"));
            }
            let name_index = strings
                .index_of(&global.name)
                .ok_or_else(|| malformed("global name is missing from STRINGS"))?;
            write_uleb(name_index, &mut bytes).map_err(io_error)?;
            bytes.push(match global.linkage {
                BinaryLinkage::Private => 0,
                BinaryLinkage::Module => 1,
                BinaryLinkage::Package => 2,
                BinaryLinkage::Exported => 3,
            });
            write_uleb(global.ty, &mut bytes).map_err(io_error)?;
            bytes.push(address_space(global.address_space));
            bytes.push(
                u8::from(global.thread_local)
                    | (u8::from(global.constant) << 1)
                    | (u8::from(global.per_cpu) << 2),
            );
            write_uleb(global.alignment.unwrap_or(0), &mut bytes).map_err(io_error)?;
            let initializer = global
                .initializer
                .map_or(Some(0), |index| index.checked_add(1))
                .ok_or_else(|| malformed("global initializer index overflows"))?;
            write_uleb(initializer, &mut bytes).map_err(io_error)?;
        }
        Ok(bytes)
    }
}

fn address_space(space: BinaryAddressSpace) -> u8 {
    match space {
        BinaryAddressSpace::Generic => 0,
        BinaryAddressSpace::Device => 1,
        BinaryAddressSpace::Constant => 2,
        BinaryAddressSpace::Threadgroup => 3,
        BinaryAddressSpace::Thread => 4,
        BinaryAddressSpace::ObjectPayload => 5,
        BinaryAddressSpace::RayPayload => 6,
        BinaryAddressSpace::HitAttribute => 7,
        BinaryAddressSpace::Tile => 8,
        BinaryAddressSpace::NodeRecord => 9,
    }
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    write_uleb(
        u64::try_from(count).map_err(|_| malformed("GLOBALS count overflows ULEB128"))?,
        bytes,
    )
    .map_err(io_error)
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(format!("GLOBALS encoding failed: {error}"))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
