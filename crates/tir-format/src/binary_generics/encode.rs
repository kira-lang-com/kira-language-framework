use std::collections::HashSet;

use super::BinaryGenericTable;
use crate::leb::write_uleb;
use crate::types::{BinarySignatureTable, BinaryType, BinaryTypeTable};
use crate::{FormatError, StringTable, StringTableBuilder};

impl BinaryGenericTable {
    /// Adds GENERICS strings in their first-reference order.
    pub fn intern_strings(&self, strings: &mut StringTableBuilder) -> Result<(), FormatError> {
        for trait_definition in &self.traits {
            intern_nonempty(strings, &trait_definition.name, "trait name")?;
            for parameter in &trait_definition.generic_parameters {
                intern_nonempty(strings, &parameter.name, "generic parameter")?;
                for bound in &parameter.bounds {
                    intern_nonempty(strings, bound, "generic bound")?;
                }
            }
            for parent in &trait_definition.parents {
                intern_nonempty(strings, parent, "parent trait")?;
            }
            for method in &trait_definition.methods {
                intern_nonempty(strings, &method.name, "trait method")?;
            }
            for associated in &trait_definition.associated_types {
                intern_nonempty(strings, &associated.name, "associated type")?;
                for bound in &associated.bounds {
                    intern_nonempty(strings, bound, "associated type bound")?;
                }
            }
        }
        for conformance in &self.conformances {
            intern_nonempty(strings, &conformance.trait_path, "conformance trait path")?;
            for associated in &conformance.associated_types {
                intern_nonempty(strings, &associated.name, "associated type binding")?;
            }
            for witness in &conformance.witnesses {
                intern_nonempty(strings, &witness.method, "witness method")?;
            }
        }
        for vtable in &self.vtables {
            intern_nonempty(strings, &vtable.name, "vtable name")?;
        }
        Ok(())
    }

    /// Encodes GENERICS and validates references into TYPES and SIGNATURES.
    pub fn encode(
        &self,
        strings: &StringTable,
        types: &BinaryTypeTable,
        signatures: &BinarySignatureTable,
    ) -> Result<Vec<u8>, FormatError> {
        let mut bytes = Vec::new();
        let mut trait_names = HashSet::new();
        write_count(&mut bytes, self.traits.len())?;
        for trait_definition in &self.traits {
            if trait_definition.name.is_empty() || !trait_names.insert(&trait_definition.name) {
                return Err(malformed("trait name is empty or duplicated"));
            }
            write_string_index(&mut bytes, strings, &trait_definition.name)?;
            validate_generic_parameters(&trait_definition.generic_parameters)?;
            write_count(&mut bytes, trait_definition.generic_parameters.len())?;
            for parameter in &trait_definition.generic_parameters {
                write_string_index(&mut bytes, strings, &parameter.name)?;
                write_unique_strings(&mut bytes, strings, &parameter.bounds, "generic bound")?;
            }
            write_unique_strings(
                &mut bytes,
                strings,
                &trait_definition.parents,
                "parent trait",
            )?;

            let mut method_names = HashSet::new();
            write_count(&mut bytes, trait_definition.methods.len())?;
            for method in &trait_definition.methods {
                if method.name.is_empty() || !method_names.insert(&method.name) {
                    return Err(malformed("trait method name is empty or duplicated"));
                }
                if !matches!(
                    type_entry(types, method.function_type)?,
                    BinaryType::Function(_)
                ) {
                    return Err(malformed("trait method requirement is not a function type"));
                }
                write_string_index(&mut bytes, strings, &method.name)?;
                write_uleb(method.function_type, &mut bytes).map_err(io_error)?;
            }

            let mut associated_names = HashSet::new();
            write_count(&mut bytes, trait_definition.associated_types.len())?;
            for associated in &trait_definition.associated_types {
                if associated.name.is_empty() || !associated_names.insert(&associated.name) {
                    return Err(malformed("associated type name is empty or duplicated"));
                }
                write_string_index(&mut bytes, strings, &associated.name)?;
                write_unique_strings(
                    &mut bytes,
                    strings,
                    &associated.bounds,
                    "associated type bound",
                )?;
            }
        }

        let mut conformance_keys = HashSet::new();
        write_count(&mut bytes, self.conformances.len())?;
        for conformance in &self.conformances {
            type_entry(types, conformance.type_index)?;
            if !conformance_keys.insert((conformance.type_index, &conformance.trait_path)) {
                return Err(malformed("duplicate (type, trait) conformance"));
            }
            write_uleb(conformance.type_index, &mut bytes).map_err(io_error)?;
            write_string_index(&mut bytes, strings, &conformance.trait_path)?;
            let mut associated_names = HashSet::new();
            write_count(&mut bytes, conformance.associated_types.len())?;
            for associated in &conformance.associated_types {
                if !associated_names.insert(&associated.name) {
                    return Err(malformed("duplicate associated type binding"));
                }
                type_entry(types, associated.type_index)?;
                write_string_index(&mut bytes, strings, &associated.name)?;
                write_uleb(associated.type_index, &mut bytes).map_err(io_error)?;
            }
            let mut methods = HashSet::new();
            write_count(&mut bytes, conformance.witnesses.len())?;
            for witness in &conformance.witnesses {
                if !methods.insert(&witness.method) {
                    return Err(malformed("duplicate witness method name"));
                }
                signature(signatures, witness.function_index)?;
                write_string_index(&mut bytes, strings, &witness.method)?;
                write_uleb(witness.function_index, &mut bytes).map_err(io_error)?;
            }
        }
        super::validate_conformance_members(&self.traits, &self.conformances, 0)?;

        let mut vtable_names = HashSet::new();
        write_count(&mut bytes, self.vtables.len())?;
        for vtable in &self.vtables {
            if vtable.name.is_empty() || !vtable_names.insert(&vtable.name) {
                return Err(malformed("vtable name is empty or duplicated"));
            }
            write_string_index(&mut bytes, strings, &vtable.name)?;
            write_count(&mut bytes, vtable.slots.len())?;
            let mut previous_slot = None;
            for slot in &vtable.slots {
                if previous_slot.is_some_and(|previous| slot.slot <= previous) {
                    return Err(malformed("vtable slots are not strictly increasing"));
                }
                previous_slot = Some(slot.slot);
                signature(signatures, slot.function_index)?;
                write_uleb(slot.slot, &mut bytes).map_err(io_error)?;
                write_uleb(slot.function_index, &mut bytes).map_err(io_error)?;
            }
        }
        Ok(bytes)
    }
}

fn validate_generic_parameters(
    parameters: &[crate::types::BinaryGenericParameter],
) -> Result<(), FormatError> {
    let mut names = HashSet::new();
    for parameter in parameters {
        if parameter.name.is_empty() || !names.insert(&parameter.name) {
            return Err(malformed("generic parameter name is empty or duplicated"));
        }
        let mut bounds = HashSet::new();
        if parameter
            .bounds
            .iter()
            .any(|bound| bound.is_empty() || !bounds.insert(bound))
        {
            return Err(malformed("generic bound is empty or duplicated"));
        }
    }
    Ok(())
}

fn write_unique_strings(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    values: &[String],
    label: &str,
) -> Result<(), FormatError> {
    let mut unique = HashSet::new();
    write_count(bytes, values.len())?;
    for value in values {
        if value.is_empty() || !unique.insert(value) {
            return Err(malformed(format!("{label} is empty or duplicated")));
        }
        write_string_index(bytes, strings, value)?;
    }
    Ok(())
}

fn type_entry(types: &BinaryTypeTable, index: u64) -> Result<&BinaryType, FormatError> {
    types
        .entries
        .get(usize::try_from(index).map_err(|_| malformed("type index overflows memory"))?)
        .ok_or_else(|| malformed("type index is out of range"))
}

fn signature(signatures: &BinarySignatureTable, index: u64) -> Result<(), FormatError> {
    if signatures
        .records
        .get(usize::try_from(index).map_err(|_| malformed("function index overflows memory"))?)
        .is_none()
    {
        return Err(malformed("function index is invalid"));
    }
    Ok(())
}

fn intern_nonempty(
    strings: &mut StringTableBuilder,
    value: &str,
    label: &str,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(malformed(format!("{label} is empty")));
    }
    strings
        .intern(value)
        .map(|_| ())
        .map_err(|error| malformed(error.message))
}

fn write_string_index(
    bytes: &mut Vec<u8>,
    strings: &StringTable,
    value: &str,
) -> Result<(), FormatError> {
    let index = strings
        .index_of(value)
        .ok_or_else(|| malformed("GENERICS string is missing from STRINGS"))?;
    write_uleb(index, bytes).map_err(io_error)
}

fn write_count(bytes: &mut Vec<u8>, count: usize) -> Result<(), FormatError> {
    let count = u64::try_from(count).map_err(|_| malformed("GENERICS count overflows ULEB128"))?;
    if count >= 1 << 32 {
        return Err(malformed("GENERICS count must be less than 2^32"));
    }
    write_uleb(count, bytes).map_err(io_error)
}

fn io_error(error: std::io::Error) -> FormatError {
    malformed(format!("GENERICS encoding failed: {error}"))
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
