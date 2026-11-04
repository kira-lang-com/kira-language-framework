use std::collections::HashSet;

use crate::leb::write_uleb;
use crate::types::{BinaryGenericParameter, BinarySignatureTable, BinaryTypeTable};
use crate::{BinaryGenericTable, FormatError, StringTable};

pub(super) fn encode_generics(
    table: &BinaryGenericTable,
    strings: &StringTable,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
) -> Result<Vec<u8>, FormatError> {
    let mut output = Vec::new();
    write_count(table.traits.len(), "trait", &mut output)?;
    for definition in &table.traits {
        write_string(&definition.name, strings, "trait name", &mut output)?;
        write_generic_parameters(&definition.generic_parameters, strings, &mut output)?;
        write_count(definition.parents.len(), "parent trait", &mut output)?;
        for parent in &definition.parents {
            write_string(parent, strings, "parent trait", &mut output)?;
        }
        write_count(definition.methods.len(), "trait method", &mut output)?;
        for method in &definition.methods {
            write_string(&method.name, strings, "trait method name", &mut output)?;
            write_index(method.function_type, "trait method type", &mut output)?;
        }
        write_count(
            definition.associated_types.len(),
            "associated type",
            &mut output,
        )?;
        for associated in &definition.associated_types {
            write_string(
                &associated.name,
                strings,
                "associated type name",
                &mut output,
            )?;
            write_count(
                associated.bounds.len(),
                "associated type bound",
                &mut output,
            )?;
            for bound in &associated.bounds {
                write_string(bound, strings, "associated type bound", &mut output)?;
            }
        }
    }

    write_count(table.conformances.len(), "conformance", &mut output)?;
    for conformance in &table.conformances {
        write_index(conformance.type_index, "conformance type", &mut output)?;
        write_string(
            &conformance.trait_path,
            strings,
            "conformance trait path",
            &mut output,
        )?;
        write_count(
            conformance.associated_types.len(),
            "associated type binding",
            &mut output,
        )?;
        for binding in &conformance.associated_types {
            write_string(
                &binding.name,
                strings,
                "associated type binding name",
                &mut output,
            )?;
            write_index(
                binding.type_index,
                "associated type binding type",
                &mut output,
            )?;
        }
        write_count(conformance.witnesses.len(), "witness", &mut output)?;
        for witness in &conformance.witnesses {
            write_string(&witness.method, strings, "witness method name", &mut output)?;
            write_index(witness.function_index, "witness function", &mut output)?;
        }
    }

    write_count(table.vtables.len(), "vtable", &mut output)?;
    for vtable in &table.vtables {
        write_string(&vtable.name, strings, "vtable name", &mut output)?;
        write_count(vtable.slots.len(), "vtable slot", &mut output)?;
        for slot in &vtable.slots {
            write_index(slot.slot, "vtable slot index", &mut output)?;
            write_index(slot.function_index, "vtable function", &mut output)?;
        }
    }

    let decoded = super::decode(&output, strings, types, signatures)?;
    if decoded != *table {
        return Err(FormatError::malformed(
            None,
            "encoded GENERICS records do not match the typed table",
        ));
    }
    Ok(output)
}

fn write_generic_parameters(
    parameters: &[BinaryGenericParameter],
    strings: &StringTable,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    write_count(parameters.len(), "generic parameter", output)?;
    let mut names = HashSet::new();
    for parameter in parameters {
        if parameter.name.is_empty() || !names.insert(parameter.name.as_str()) {
            return Err(FormatError::malformed(
                None,
                "GENERICS parameter names must be nonempty and unique",
            ));
        }
        write_string(&parameter.name, strings, "generic parameter name", output)?;
        write_count(parameter.bounds.len(), "generic bound", output)?;
        let mut bounds = HashSet::new();
        for bound in &parameter.bounds {
            if bound.is_empty() || !bounds.insert(bound.as_str()) {
                return Err(FormatError::malformed(
                    None,
                    "GENERICS bounds must be nonempty and unique",
                ));
            }
            write_string(bound, strings, "generic bound", output)?;
        }
    }
    Ok(())
}

fn write_count(count: usize, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    let count = u64::try_from(count)
        .map_err(|_| FormatError::malformed(None, format!("GENERICS {label} count overflows")))?;
    if count >= 1 << 32 {
        return Err(FormatError::malformed(
            None,
            format!("GENERICS {label} count exceeds the limit"),
        ));
    }
    write_index(count, label, output)
}

fn write_index(value: u64, label: &str, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output)
        .map_err(|error| FormatError::malformed(None, format!("GENERICS {label}: {error}")))
}

fn write_string(
    value: &str,
    strings: &StringTable,
    label: &str,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if value.is_empty() {
        return Err(FormatError::malformed(
            None,
            format!("GENERICS {label} is empty"),
        ));
    }
    let index = strings.index_of(value).ok_or_else(|| {
        FormatError::malformed(None, format!("GENERICS {label} is absent from STRINGS"))
    })?;
    let index = u64::try_from(index)
        .map_err(|_| FormatError::malformed(None, "GENERICS string index overflows"))?;
    write_index(index, label, output)
}
