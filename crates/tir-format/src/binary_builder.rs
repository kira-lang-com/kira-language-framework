use tir_core::ContentHash;

use crate::types::{
    BinaryCodeTable, BinaryConstantTable, BinaryCopyPolicy, BinaryDestroyPolicy, BinaryGlobalTable,
    BinaryGpuTable, BinaryRelocatePolicy, BinarySignatureTable, BinaryType, BinaryTypeTable,
    BinaryVariantSet,
};
use crate::{
    BinaryAnalysisSummary, BinaryGenericTable, BinaryImport, BinaryModule, BinaryModuleRecord,
    FormatError, Header, Section, StringTable, StringTableBuilder,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryAssemblySourceRecord {
    pub template: String,
    pub constraints: String,
}

/// Builds a canonical TIR container from its modeled semantic sections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryModuleBuilder {
    pub header: Header,
    pub module: BinaryModuleRecord,
    pub imports: Vec<BinaryImport>,
    pub types: BinaryTypeTable,
    pub constants: BinaryConstantTable,
    pub globals: BinaryGlobalTable,
    pub signatures: BinarySignatureTable,
    pub code: BinaryCodeTable,
    /// CODE string literals, interned after SIGNATURES and before later sections.
    pub code_strings: Vec<String>,
    pub variants: Vec<BinaryVariantSet>,
    pub gpu: BinaryGpuTable,
    pub generics: BinaryGenericTable,
    pub analysis: BinaryAnalysisSummary,
    pub assemblies: Vec<BinaryAssemblySourceRecord>,
}

impl BinaryModuleBuilder {
    #[must_use]
    pub fn new(header: Header, module: BinaryModuleRecord) -> Self {
        Self {
            header,
            module,
            imports: Vec::new(),
            types: BinaryTypeTable::default(),
            constants: BinaryConstantTable::default(),
            globals: BinaryGlobalTable::default(),
            signatures: BinarySignatureTable::default(),
            code: BinaryCodeTable::default(),
            code_strings: Vec::new(),
            variants: Vec::new(),
            gpu: BinaryGpuTable::default(),
            generics: BinaryGenericTable::default(),
            analysis: BinaryAnalysisSummary::default(),
            assemblies: Vec::new(),
        }
    }

    /// Encodes modeled sections in kind order and canonical STRINGS order.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        validate_import_type_references(&self.imports, &self.types)?;
        validate_type_signature_references(&self.types, &self.signatures)?;
        validate_module_function_references(&self.module, &self.signatures)?;
        let (strings, assembly_records) = self.build_string_table()?;

        let mut sections = vec![
            Section::from_uncompressed(
                1,
                strings.encode().map_err(|error| malformed(error.message))?,
            ),
            Section::from_uncompressed(2, self.module.encode(&strings, self.header.profile)?),
        ];
        if !self.imports.is_empty() {
            sections.push(Section::from_uncompressed(
                3,
                crate::encode_imports(&self.imports, &strings)?,
            ));
        }
        if !self.types.declarations.is_empty()
            || !self.types.entries.is_empty()
            || !self.types.definitions.is_empty()
        {
            sections.push(Section::from_uncompressed(4, self.types.encode(&strings)?));
        }
        if !self.constants.records.is_empty() {
            sections.push(Section::from_uncompressed(
                5,
                self.constants.encode(&strings, &self.types)?,
            ));
        }
        if !self.globals.records.is_empty() {
            sections.push(Section::from_uncompressed(
                6,
                self.globals
                    .encode(&strings, &self.types, &self.constants)?,
            ));
        }
        if !self.signatures.records.is_empty() {
            sections.push(Section::from_uncompressed(
                7,
                self.signatures.encode(&strings, &self.types)?,
            ));
        }
        if !self.code.functions.is_empty() {
            let assembly_count = u64::try_from(assembly_records.len())
                .map_err(|_| malformed("ASM record count overflows"))?;
            sections.push(Section::from_uncompressed(
                8,
                self.code.encode(
                    &self.signatures,
                    &self.types,
                    &self.constants,
                    &strings,
                    assembly_count,
                )?,
            ));
        }
        if !self.variants.is_empty() {
            sections.push(Section::from_uncompressed(
                9,
                crate::encode_variant_sets(&self.variants, &strings, &self.signatures)?,
            ));
        }
        if let Some(gpu) = self
            .gpu
            .encode(&self.signatures, &self.types, &self.constants)?
        {
            sections.push(Section::from_uncompressed(10, gpu));
        }
        if !self.generics.traits.is_empty()
            || !self.generics.conformances.is_empty()
            || !self.generics.vtables.is_empty()
        {
            sections.push(Section::from_uncompressed(
                11,
                self.generics
                    .encode(&strings, &self.types, &self.signatures)?,
            ));
        }
        sections.push(Section::from_uncompressed(
            14,
            self.analysis
                .encode(&self.signatures, &self.globals, &assembly_records)?,
        ));
        if !assembly_records.is_empty() {
            sections.push(Section::from_uncompressed(
                16,
                crate::encode_assembly_table(&assembly_records, &strings)?,
            ));
        }

        let mut header = self.header;
        header.flags |= 1 << 6;
        let module = BinaryModule {
            header,
            sections,
            content_hash: ContentHash::of(&[]),
        };
        let mut bytes = Vec::new();
        module.write_uncompressed_container(&mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn string_index(&self, value: &str) -> Result<Option<u64>, FormatError> {
        let (strings, _) = self.build_string_table()?;
        Ok(strings.index_of(value))
    }

    fn build_string_table(
        &self,
    ) -> Result<(StringTable, Vec<crate::BinaryAssemblyRecord>), FormatError> {
        let mut strings = StringTableBuilder::new();
        self.module.intern_strings(&mut strings)?;
        for import in &self.imports {
            import.intern_strings(&mut strings)?;
        }
        self.types.intern_strings(&mut strings)?;
        self.constants.intern_strings(&mut strings)?;
        self.globals.intern_strings(&mut strings)?;
        self.signatures.intern_strings(&mut strings)?;
        for value in &self.code_strings {
            strings
                .intern(value)
                .map_err(|error| malformed(error.message))?;
        }
        crate::intern_variant_strings(&self.variants, &mut strings)?;
        self.generics.intern_strings(&mut strings)?;
        let mut assembly_records = Vec::with_capacity(self.assemblies.len());
        for assembly in &self.assemblies {
            let template_index = strings
                .intern(&assembly.template)
                .map_err(|error| malformed(error.message))?;
            let constraints_index = strings
                .intern(&assembly.constraints)
                .map_err(|error| malformed(error.message))?;
            assembly_records.push(crate::BinaryAssemblyRecord {
                template_index,
                constraints_index,
            });
        }
        Ok((strings.finish(), assembly_records))
    }
}

fn validate_import_type_references(
    imports: &[BinaryImport],
    types: &BinaryTypeTable,
) -> Result<(), FormatError> {
    for import in imports {
        for entity in &import.entities {
            if usize::try_from(entity.type_index)
                .ok()
                .and_then(|index| types.entries.get(index))
                .is_none()
            {
                return Err(malformed("imported entity type index is out of range"));
            }
        }
    }
    Ok(())
}

fn validate_type_signature_references(
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
) -> Result<(), FormatError> {
    for entry in &types.entries {
        if let BinaryType::Frame { function_index } = entry {
            require_signature(signatures, *function_index)?;
        }
    }
    for definition in &types.definitions {
        match definition.policy.copy {
            BinaryCopyPolicy::Implicit(function) | BinaryCopyPolicy::Explicit(function) => {
                require_signature(signatures, function)?;
            }
            BinaryCopyPolicy::Trivial | BinaryCopyPolicy::None => {}
        }
        if let BinaryDestroyPolicy::Function(function) = definition.policy.destroy {
            require_signature(signatures, function)?;
        }
        if let BinaryRelocatePolicy::Function(function) = definition.policy.relocate {
            require_signature(signatures, function)?;
        }
    }
    Ok(())
}

fn require_signature(signatures: &BinarySignatureTable, index: u64) -> Result<(), FormatError> {
    if usize::try_from(index)
        .ok()
        .and_then(|index| signatures.records.get(index))
        .is_none()
    {
        return Err(malformed("function signature index is out of range"));
    }
    Ok(())
}

fn validate_module_function_references(
    module: &BinaryModuleRecord,
    signatures: &BinarySignatureTable,
) -> Result<(), FormatError> {
    for initializer in module.initializers.iter().chain(module.finalizers.iter()) {
        require_signature(signatures, initializer.function_index)?;
    }
    Ok(())
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
