use crate::binary::{FormatError, TvisaImage};
use crate::constants::{self, ConstantRecord};
use crate::data::DataSection;
use crate::functions::{self, FunctionRecordSummary};
use crate::imports::{self, ImportCounts};
use crate::types::TypeSection;
use crate::variant_index::{self, VariantIndex};

impl TvisaImage {
    /// Decodes the container and validates bounded section records used by
    /// binary verifier passes.
    pub fn decode_and_validate_sections(
        bytes: &[u8],
        section_memory_limit: u64,
    ) -> Result<Self, FormatError> {
        let image = Self::decode(bytes)?;
        image.data_section(section_memory_limit)?;
        image.import_counts(section_memory_limit)?;
        image.image_record(section_memory_limit)?;
        image.function_records(section_memory_limit)?;
        image.variant_index(section_memory_limit)?;
        image.validate_signature_indices(section_memory_limit)?;
        image.validate_header_feature_flags(section_memory_limit)?;
        image.function_body_headers(section_memory_limit)?;
        image.constants(section_memory_limit)?;
        image.unwind_section(section_memory_limit)?;
        image.assembly_records(section_memory_limit)?;
        image.analysis_summary(section_memory_limit)?;
        image.metadata_table(section_memory_limit)?;
        image.debug_table(section_memory_limit)?;
        Ok(image)
    }

    /// Decodes and checks DATA records, resolving each object's name through STRINGS.
    pub fn data_section(&self, memory_limit: u64) -> Result<DataSection, FormatError> {
        let data = if let Some(section) = self.sections.iter().find(|section| section.kind == 6) {
            let strings = self.string_table(memory_limit)?;
            let bytes = section.read_uncompressed(memory_limit)?;
            DataSection::decode(&bytes, &strings)?
        } else {
            DataSection {
                objects: Vec::new(),
            }
        };
        let imports = self.import_counts(memory_limit)?;
        let total = imports
            .data
            .checked_add(data.objects.len() as u64)
            .ok_or_else(|| FormatError::malformed(None, "data object count overflows"))?;
        if total >= 1 << 28 {
            return Err(FormatError::malformed(
                None,
                "data object count exceeds the TVISA image limit",
            ));
        }
        Ok(data)
    }

    pub fn import_counts(&self, memory_limit: u64) -> Result<ImportCounts, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 3) else {
            return Ok(ImportCounts::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        let imports = imports::decode_counts(&bytes, &strings)?;
        let total_functions = imports
            .functions
            .checked_add(u64::from(self.header.function_count))
            .ok_or_else(|| FormatError::malformed(None, "function count overflows"))?;
        if total_functions >= 1 << 28 {
            return Err(FormatError::malformed(
                None,
                "function count exceeds the TVISA image limit",
            ));
        }
        Ok(imports)
    }

    pub fn function_records(
        &self,
        memory_limit: u64,
    ) -> Result<Vec<FunctionRecordSummary>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 7) else {
            if self.header.function_count == 0 {
                return Ok(Vec::new());
            }
            return Err(FormatError::malformed(
                None,
                "SIGNATURES section is missing for defined functions",
            ));
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        functions::decode_summaries(&bytes, self.header.function_count, &strings)
    }

    pub fn variant_index(&self, memory_limit: u64) -> Result<VariantIndex, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 10) else {
            return Ok(VariantIndex::default());
        };
        let strings = self.string_table(memory_limit)?;
        let bytes = section.read_uncompressed(memory_limit)?;
        variant_index::decode(&bytes, &strings)
    }

    pub fn type_section(&self, memory_limit: u64) -> Result<TypeSection, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 4) else {
            return Ok(TypeSection::default());
        };
        TypeSection::decode(&section.read_uncompressed(memory_limit)?)
    }

    pub fn constants(&self, memory_limit: u64) -> Result<Vec<ConstantRecord>, FormatError> {
        let Some(section) = self.sections.iter().find(|section| section.kind == 5) else {
            return Ok(Vec::new());
        };
        let bytes = section.read_uncompressed(memory_limit)?;
        let records = constants::decode(&bytes, &self.type_section(memory_limit)?)?;
        if records.is_empty() {
            return Err(FormatError::malformed(
                None,
                "CONSTANTS section must be absent when it has no records",
            ));
        }
        Ok(records)
    }

    fn validate_header_feature_flags(&self, memory_limit: u64) -> Result<(), FormatError> {
        let functions = self.function_records(memory_limit)?;
        let imports = self.import_counts(memory_limit)?;
        let variants = self.variant_index(memory_limit)?;
        let assembly = self.assembly_records(memory_limit)?;
        let types = self.type_section(memory_limit)?;

        let mut expected = 0u32;
        if functions.iter().any(|function| function.target != 0) {
            expected |= 1 << 0;
        }
        if !assembly.is_empty() {
            expected |= 1 << 1;
        }
        if variants.sets.iter().any(|set| set.kind == 0) {
            expected |= 1 << 2;
        }
        if variants.sets.iter().any(|set| set.kind == 1) {
            expected |= 1 << 3;
        }
        if functions.iter().any(|function| function.trust_class == 2) {
            expected |= 1 << 4;
        }
        if functions.iter().any(|function| function.trust_class == 1) {
            expected |= 1 << 5;
        }

        let signature_indices = imports
            .function_signatures
            .iter()
            .copied()
            .chain(functions.iter().map(|function| function.signature_index));
        for signature_index in signature_indices {
            let signature_index = usize::try_from(signature_index).map_err(|_| {
                FormatError::malformed(None, "function signature index does not fit in memory")
            })?;
            let signature = types.signatures.get(signature_index).ok_or_else(|| {
                FormatError::malformed(
                    None,
                    "function record references an invalid signature index",
                )
            })?;
            if signature.convention == 1 {
                expected |= 1 << 7;
                break;
            }
        }

        let summary_mask = (1 << 6) - 1 | 1 << 7;
        if self.header.flags & summary_mask != expected {
            return Err(FormatError::malformed(
                Some(8),
                "header feature flags do not match the image contents",
            ));
        }
        Ok(())
    }

    pub fn validate_signature_indices(&self, memory_limit: u64) -> Result<(), FormatError> {
        let types = self.type_section(memory_limit)?;
        let imports = self.import_counts(memory_limit)?;
        let functions = self.function_records(memory_limit)?;
        for index in imports
            .function_signatures
            .iter()
            .copied()
            .chain(functions.iter().map(|function| function.signature_index))
        {
            if index >= types.signatures.len() as u64 {
                return Err(FormatError::malformed(
                    None,
                    "function record references an invalid signature index",
                ));
            }
        }
        Ok(())
    }
}
