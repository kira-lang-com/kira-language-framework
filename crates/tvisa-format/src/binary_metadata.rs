use tir_format::{BinaryMetadata, BinaryMetadataTable, MetadataEncodeError, StringTable};

use crate::{FormatError, TvisaImage};

pub fn encode_metadata_table(
    table: &BinaryMetadataTable,
    strings: &StringTable,
) -> Result<Vec<u8>, MetadataEncodeError> {
    if table.attachments.iter().any(|attachment| {
        matches!(
            &attachment.metadata,
            BinaryMetadata::Tbaa(_) | BinaryMetadata::ProfileId(_)
        )
    }) {
        return Err(MetadataEncodeError {
            message: "TVISA METADATA cannot contain TBAA or profile-id attachments".to_owned(),
        });
    }
    table.encode(strings)
}

impl TvisaImage {
    pub fn metadata_table(&self, memory_limit: u64) -> Result<BinaryMetadataTable, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 13) else {
            return Ok(BinaryMetadataTable::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let table = BinaryMetadataTable::decode(&bytes, &strings)
            .map_err(|error| FormatError::malformed(Some(error.offset), error.message))?;
        if table.attachments.iter().any(|attachment| {
            matches!(
                &attachment.metadata,
                BinaryMetadata::Tbaa(_) | BinaryMetadata::ProfileId(_)
            )
        }) {
            return Err(FormatError::malformed(
                None,
                "TVISA METADATA cannot contain TBAA or profile-id attachments",
            ));
        }
        Ok(table)
    }
}
