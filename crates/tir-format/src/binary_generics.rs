use std::collections::HashSet;

use crate::leb::{LebError, read_uleb};
use crate::types::{BinaryGenericParameter, BinarySignatureTable, BinaryType, BinaryTypeTable};
use crate::{BinaryModule, FormatError, StringTable};

mod encode;

const MAX_RECORDS: u64 = 1 << 32;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BinaryGenericTable {
    pub traits: Vec<BinaryTraitDefinition>,
    pub conformances: Vec<BinaryConformance>,
    pub vtables: Vec<BinaryVTable>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryTraitDefinition {
    pub name: String,
    pub generic_parameters: Vec<BinaryGenericParameter>,
    pub parents: Vec<String>,
    pub methods: Vec<BinaryTraitMethod>,
    pub associated_types: Vec<BinaryAssociatedTypeRequirement>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryTraitMethod {
    pub name: String,
    pub function_type: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryAssociatedTypeRequirement {
    pub name: String,
    pub bounds: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryConformance {
    pub type_index: u64,
    pub trait_path: String,
    pub associated_types: Vec<BinaryAssociatedTypeBinding>,
    pub witnesses: Vec<BinaryWitness>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryAssociatedTypeBinding {
    pub name: String,
    pub type_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryWitness {
    pub method: String,
    pub function_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryVTable {
    pub name: String,
    pub slots: Vec<BinaryVTableSlot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryVTableSlot {
    pub slot: u64,
    pub function_index: u64,
}

impl BinaryModule {
    /// Decodes GENERICS and checks all references to TYPES and SIGNATURES.
    pub fn generics_table(
        &self,
        memory_limit: u64,
        types: &BinaryTypeTable,
        signatures: &BinarySignatureTable,
    ) -> Result<BinaryGenericTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 11) else {
            return Ok(BinaryGenericTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        decode(&bytes, &strings, types, signatures)
    }
}

fn decode(
    bytes: &[u8],
    strings: &StringTable,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
) -> Result<BinaryGenericTable, FormatError> {
    let mut cursor = 0;
    let trait_count = read_count(bytes, &mut cursor, 1, "trait")?;
    let mut traits = reserve(trait_count, bytes, cursor, "trait")?;
    let mut trait_names = HashSet::new();
    for _ in 0..trait_count {
        let name_offset = cursor;
        let name = read_string(bytes, &mut cursor, strings, "trait name")?;
        if !trait_names.insert(name.clone()) {
            return Err(malformed(name_offset, "duplicate trait name"));
        }
        let generic_parameters = read_generic_parameters(bytes, &mut cursor, strings)?;
        let parent_count = read_count(bytes, &mut cursor, 1, "parent trait")?;
        let mut parents = reserve(parent_count, bytes, cursor, "parent trait")?;
        let mut unique_parents = HashSet::new();
        for _ in 0..parent_count {
            let offset = cursor;
            let parent = read_string(bytes, &mut cursor, strings, "parent trait")?;
            if !unique_parents.insert(parent.clone()) {
                return Err(malformed(offset, "duplicate parent trait"));
            }
            parents.push(parent);
        }
        let method_count = read_count(bytes, &mut cursor, 2, "trait method")?;
        let mut methods = reserve(method_count, bytes, cursor, "trait method")?;
        let mut method_names = HashSet::new();
        for _ in 0..method_count {
            let offset = cursor;
            let method = read_string(bytes, &mut cursor, strings, "trait method name")?;
            if !method_names.insert(method.clone()) {
                return Err(malformed(offset, "duplicate trait method name"));
            }
            let type_offset = cursor;
            let function_type = read_type_index(bytes, &mut cursor, types)?;
            if !matches!(
                usize::try_from(function_type)
                    .ok()
                    .and_then(|index| types.entries.get(index)),
                Some(BinaryType::Function(_))
            ) {
                return Err(malformed(
                    type_offset,
                    "trait method requirement is not a function type",
                ));
            }
            methods.push(BinaryTraitMethod {
                name: method,
                function_type,
            });
        }
        let associated_count = read_count(bytes, &mut cursor, 2, "associated type")?;
        let mut associated_types = reserve(associated_count, bytes, cursor, "associated type")?;
        let mut associated_names = HashSet::new();
        for _ in 0..associated_count {
            let offset = cursor;
            let associated = read_string(bytes, &mut cursor, strings, "associated type name")?;
            if !associated_names.insert(associated.clone()) {
                return Err(malformed(offset, "duplicate associated type name"));
            }
            let bound_count = read_count(bytes, &mut cursor, 1, "associated type bound")?;
            let mut bounds = reserve(bound_count, bytes, cursor, "associated type bound")?;
            let mut unique_bounds = HashSet::new();
            for _ in 0..bound_count {
                let bound_offset = cursor;
                let bound = read_string(bytes, &mut cursor, strings, "associated type bound")?;
                if !unique_bounds.insert(bound.clone()) {
                    return Err(malformed(bound_offset, "duplicate associated type bound"));
                }
                bounds.push(bound);
            }
            associated_types.push(BinaryAssociatedTypeRequirement {
                name: associated,
                bounds,
            });
        }
        traits.push(BinaryTraitDefinition {
            name,
            generic_parameters,
            parents,
            methods,
            associated_types,
        });
    }

    let conformance_count = read_count(bytes, &mut cursor, 3, "conformance")?;
    let mut conformances = reserve(conformance_count, bytes, cursor, "conformance")?;
    let mut conformance_keys = HashSet::new();
    for _ in 0..conformance_count {
        let type_offset = cursor;
        let type_index = read_type_index(bytes, &mut cursor, types)?;
        let trait_path = read_string(bytes, &mut cursor, strings, "conformance trait path")?;
        if !conformance_keys.insert((type_index, trait_path.clone())) {
            return Err(malformed(
                type_offset,
                "duplicate (type, trait) conformance",
            ));
        }

        let associated_count = read_count(bytes, &mut cursor, 2, "associated type binding")?;
        let mut associated_types =
            reserve(associated_count, bytes, cursor, "associated type binding")?;
        let mut associated_names = HashSet::new();
        for _ in 0..associated_count {
            let offset = cursor;
            let name = read_string(bytes, &mut cursor, strings, "associated type binding name")?;
            if !associated_names.insert(name.clone()) {
                return Err(malformed(offset, "duplicate associated type binding"));
            }
            let type_index = read_type_index(bytes, &mut cursor, types)?;
            associated_types.push(BinaryAssociatedTypeBinding { name, type_index });
        }

        let witness_count = read_count(bytes, &mut cursor, 2, "witness")?;
        let mut witnesses = reserve(witness_count, bytes, cursor, "witness")?;
        let mut method_names = HashSet::new();
        for _ in 0..witness_count {
            let offset = cursor;
            let method = read_string(bytes, &mut cursor, strings, "witness method name")?;
            if !method_names.insert(method.clone()) {
                return Err(malformed(offset, "duplicate witness method name"));
            }
            let function_offset = cursor;
            let function_index = read_index(bytes, &mut cursor)?;
            if usize::try_from(function_index)
                .ok()
                .and_then(|index| signatures.records.get(index))
                .is_none()
            {
                return Err(malformed(
                    function_offset,
                    "witness function index is invalid",
                ));
            }
            witnesses.push(BinaryWitness {
                method,
                function_index,
            });
        }
        conformances.push(BinaryConformance {
            type_index,
            trait_path,
            associated_types,
            witnesses,
        });
    }

    validate_conformance_members(&traits, &conformances, cursor)?;

    let vtable_count = read_count(bytes, &mut cursor, 2, "vtable")?;
    let mut vtables = reserve(vtable_count, bytes, cursor, "vtable")?;
    let mut vtable_names = HashSet::new();
    for _ in 0..vtable_count {
        let name_offset = cursor;
        let name = read_string(bytes, &mut cursor, strings, "vtable name")?;
        if !vtable_names.insert(name.clone()) {
            return Err(malformed(name_offset, "duplicate vtable name"));
        }
        let slot_count = read_count(bytes, &mut cursor, 2, "vtable slot")?;
        let mut slots = reserve(slot_count, bytes, cursor, "vtable slot")?;
        let mut previous_slot = None;
        for _ in 0..slot_count {
            let slot_offset = cursor;
            let slot = read_index(bytes, &mut cursor)?;
            if previous_slot.is_some_and(|previous| slot <= previous) {
                return Err(malformed(
                    slot_offset,
                    "vtable slots are not strictly increasing",
                ));
            }
            previous_slot = Some(slot);
            let function_offset = cursor;
            let function_index = read_index(bytes, &mut cursor)?;
            if usize::try_from(function_index)
                .ok()
                .and_then(|index| signatures.records.get(index))
                .is_none()
            {
                return Err(malformed(
                    function_offset,
                    "vtable function index is invalid",
                ));
            }
            slots.push(BinaryVTableSlot {
                slot,
                function_index,
            });
        }
        vtables.push(BinaryVTable { name, slots });
    }
    if cursor != bytes.len() {
        return Err(malformed(cursor, "trailing bytes in GENERICS section"));
    }
    Ok(BinaryGenericTable {
        traits,
        conformances,
        vtables,
    })
}

fn validate_conformance_members(
    traits: &[BinaryTraitDefinition],
    conformances: &[BinaryConformance],
    offset: usize,
) -> Result<(), FormatError> {
    for conformance in conformances {
        let Some(trait_definition) = traits
            .iter()
            .find(|definition| definition.name == conformance.trait_path)
        else {
            continue;
        };
        let required_associated = trait_definition
            .associated_types
            .iter()
            .map(|associated| associated.name.as_str())
            .collect::<HashSet<_>>();
        let actual_associated = conformance
            .associated_types
            .iter()
            .map(|associated| associated.name.as_str())
            .collect::<HashSet<_>>();
        if required_associated != actual_associated {
            return Err(malformed(
                offset,
                "conformance associated-type bindings do not match its trait",
            ));
        }
        let required_methods = trait_definition
            .methods
            .iter()
            .map(|method| method.name.as_str())
            .collect::<HashSet<_>>();
        let actual_methods = conformance
            .witnesses
            .iter()
            .map(|witness| witness.method.as_str())
            .collect::<HashSet<_>>();
        if required_methods != actual_methods {
            return Err(malformed(
                offset,
                "conformance witnesses do not match its trait methods",
            ));
        }
    }
    Ok(())
}

fn read_generic_parameters(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
) -> Result<Vec<BinaryGenericParameter>, FormatError> {
    let count = read_count(bytes, cursor, 2, "generic parameter")?;
    let mut parameters = reserve(count, bytes, *cursor, "generic parameter")?;
    let mut names = HashSet::new();
    for _ in 0..count {
        let name_offset = *cursor;
        let name = read_string(bytes, cursor, strings, "generic parameter name")?;
        if !names.insert(name.clone()) {
            return Err(malformed(name_offset, "duplicate generic parameter name"));
        }
        let bound_count = read_count(bytes, cursor, 1, "generic bound")?;
        let mut bounds = reserve(bound_count, bytes, *cursor, "generic bound")?;
        let mut unique_bounds = HashSet::new();
        for _ in 0..bound_count {
            let bound_offset = *cursor;
            let bound = read_string(bytes, cursor, strings, "generic bound")?;
            if !unique_bounds.insert(bound.clone()) {
                return Err(malformed(bound_offset, "duplicate generic bound"));
            }
            bounds.push(bound);
        }
        parameters.push(BinaryGenericParameter { name, bounds });
    }
    Ok(parameters)
}

fn read_type_index(
    bytes: &[u8],
    cursor: &mut usize,
    types: &BinaryTypeTable,
) -> Result<u64, FormatError> {
    let offset = *cursor;
    let index = read_index(bytes, cursor)?;
    if usize::try_from(index)
        .ok()
        .and_then(|index| types.entries.get(index))
        .is_none()
    {
        return Err(malformed(offset, "type index is out of range"));
    }
    Ok(index)
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    strings: &StringTable,
    label: &str,
) -> Result<String, FormatError> {
    let offset = *cursor;
    let index = usize::try_from(read_index(bytes, cursor)?)
        .map_err(|_| malformed(offset, format!("{label} index overflows memory")))?;
    let value = strings
        .get(index)
        .ok_or_else(|| malformed(offset, format!("{label} index is out of range")))?;
    if value.is_empty() {
        return Err(malformed(offset, format!("{label} is empty")));
    }
    Ok(value.to_owned())
}

fn read_count(
    bytes: &[u8],
    cursor: &mut usize,
    minimum_record_bytes: usize,
    label: &str,
) -> Result<usize, FormatError> {
    let offset = *cursor;
    let count = read_index(bytes, cursor)?;
    if count >= MAX_RECORDS {
        return Err(malformed(
            offset,
            format!("{label} count must be less than 2^32"),
        ));
    }
    let count = usize::try_from(count)
        .map_err(|_| malformed(offset, format!("{label} count overflows memory")))?;
    if minimum_record_bytes != 0
        && count > bytes.len().saturating_sub(*cursor) / minimum_record_bytes
    {
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
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| malformed(offset, format!("{label} allocation failed")))?;
    if count > bytes.len() {
        return Err(malformed(
            offset,
            format!("{label} count exceeds section size"),
        ));
    }
    Ok(values)
}

fn read_index(bytes: &[u8], cursor: &mut usize) -> Result<u64, FormatError> {
    read_uleb(bytes, cursor).map_err(|error| map_leb_error(error, *cursor))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}

fn map_leb_error(error: LebError, offset: usize) -> FormatError {
    malformed(offset, format!("invalid GENERICS ULEB: {error:?}"))
}
