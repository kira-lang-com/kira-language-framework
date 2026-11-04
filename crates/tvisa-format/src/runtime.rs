use tir_format::leb::write_uleb;

use crate::binary::FormatError;
use crate::text::TextRuntimeMetadata;

/// Encode the optional RUNTIME section's strictly increasing entry IDs.
/// An empty list is represented by omitting the section.
pub fn encode_runtime_entry_ids(entries: &[u16]) -> Result<Vec<u8>, FormatError> {
    if entries.is_empty() {
        return Err(FormatError::malformed(
            None,
            "RUNTIME must be absent when the image uses no runtime entries",
        ));
    }
    if entries.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(FormatError::malformed(
            None,
            "RUNTIME identifiers must be strictly increasing",
        ));
    }

    let mut bytes = Vec::new();
    write_uleb(
        u64::try_from(entries.len())
            .map_err(|_| FormatError::malformed(None, "RUNTIME count overflows"))?,
        &mut bytes,
    )
    .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    for entry in entries {
        bytes.extend_from_slice(&entry.to_le_bytes());
    }
    Ok(bytes)
}

/// Encodes the optional RUNTIME record parsed from a TVISA text image.
pub fn encode_text_runtime_metadata(
    metadata: &[TextRuntimeMetadata],
) -> Result<Option<Vec<u8>>, FormatError> {
    match metadata {
        [] => Ok(None),
        [record] => encode_runtime_entry_ids(&record.identifiers).map(Some),
        _ => Err(FormatError::malformed(
            None,
            "TVISA text image has multiple runtime metadata records",
        )),
    }
}
