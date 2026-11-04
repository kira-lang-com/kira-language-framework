use tir_format::StringTable;
use tir_format::leb::write_uleb;

use crate::binary::FormatError;

use super::{
    ADDRESS_TAKEN, ATTRIBUTE_MASK, FrameSlotSummary, FunctionRecordSummary, FunctionVariant,
    decode_summaries, forbidden_type_tag,
};

/// Encode the fixed-count SIGNATURES function records and validate the result.
pub fn encode_function_records(
    records: &[FunctionRecordSummary],
    strings: &StringTable,
) -> Result<Vec<u8>, FormatError> {
    let expected_count = u32::try_from(records.len())
        .map_err(|_| malformed(0, "SIGNATURES function count overflows"))?;
    let mut bytes = Vec::new();
    for record in records {
        encode_function(record, strings, &mut bytes)?;
    }

    let decoded = decode_summaries(&bytes, expected_count, strings)?;
    if decoded != records {
        return Err(malformed(
            0,
            "SIGNATURES records are inconsistent with their binary encoding",
        ));
    }
    Ok(bytes)
}

fn encode_function(
    record: &FunctionRecordSummary,
    strings: &StringTable,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    if record.name.is_empty() || record.attributes & !ATTRIBUTE_MASK != 0 {
        return Err(malformed(
            output.len(),
            "invalid SIGNATURES name or attributes",
        ));
    }
    let name_index = strings
        .index_of(&record.name)
        .ok_or_else(|| malformed(output.len(), "SIGNATURES name is absent from STRINGS"))?;
    write_index(
        u64::try_from(name_index)
            .map_err(|_| malformed(output.len(), "SIGNATURES name index overflows"))?,
        output,
    )?;
    output.push(record.linkage);
    write_index(record.signature_index, output)?;
    output.push(record.target);
    write_index(record.attributes, output)?;

    match (record.attributes & ADDRESS_TAKEN != 0, record.type_tag) {
        (true, Some(type_tag)) if !forbidden_type_tag(type_tag) => {
            output.extend_from_slice(&type_tag.to_le_bytes());
        }
        (false, None) => {}
        (true, Some(_)) => {
            return Err(malformed(
                output.len(),
                "SIGNATURES address-taken function has a forbidden type tag",
            ));
        }
        _ => {
            return Err(malformed(
                output.len(),
                "SIGNATURES type tag does not match address_taken",
            ));
        }
    }

    encode_variant(record.variant.as_ref(), strings, output)?;
    output.push(record.trust_class);
    output.push(record.semantics);
    output.push(record.personality);
    output.push(record.alignment_log2);
    output.push(record.placement);
    write_index(
        u64::try_from(record.slots.len())
            .map_err(|_| malformed(output.len(), "SIGNATURES frame slot count overflows"))?,
        output,
    )?;
    for slot in &record.slots {
        encode_slot(slot, output)?;
    }
    write_index(record.debug_scope, output)
}

fn encode_variant(
    variant: Option<&FunctionVariant>,
    strings: &StringTable,
    output: &mut Vec<u8>,
) -> Result<(), FormatError> {
    let Some(variant) = variant else {
        output.push(u8::MAX);
        return write_index(0, output);
    };

    output.push(variant.architecture);
    write_index(
        u64::try_from(variant.features.len())
            .map_err(|_| malformed(output.len(), "SIGNATURES feature count overflows"))?,
        output,
    )?;
    for feature in &variant.features {
        if feature.is_empty() {
            return Err(malformed(output.len(), "SIGNATURES feature name is empty"));
        }
        let index = strings
            .index_of(feature)
            .ok_or_else(|| malformed(output.len(), "SIGNATURES feature is absent from STRINGS"))?;
        write_index(
            u64::try_from(index)
                .map_err(|_| malformed(output.len(), "SIGNATURES feature index overflows"))?,
            output,
        )?;
    }
    Ok(())
}

fn encode_slot(slot: &FrameSlotSummary, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_index(slot.size, output)?;
    output.push(slot.alignment_log2);
    output.push(slot.address_space);
    output.push(slot.flags);
    write_index(slot.debug_variable, output)
}

fn write_index(value: u64, output: &mut Vec<u8>) -> Result<(), FormatError> {
    write_uleb(value, output).map_err(|error| malformed(output.len(), error.to_string()))
}

fn malformed(offset: usize, message: impl Into<String>) -> FormatError {
    FormatError::malformed(Some(offset), message)
}
