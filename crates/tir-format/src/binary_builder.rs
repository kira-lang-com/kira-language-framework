use tir_core::ContentHash;

use crate::types::{
    BinaryCodeTable, BinaryConstantTable, BinaryCopyPolicy, BinaryDestroyPolicy, BinaryEngine,
    BinaryGlobalTable, BinaryGpuTable, BinaryRelocatePolicy, BinarySignatureTable, BinaryTarget,
    BinaryType, BinaryTypeTable, BinaryVariantSet,
};
use crate::{
    BinaryAnalysisSummary, BinaryCapabilities, BinaryGenericTable, BinaryImport, BinaryMetadata,
    BinaryMetadataTable, BinaryModule, BinaryModuleRecord, BinaryRidlTable, FormatError, Header,
    Section, SemanticsMode, StringTable, StringTableBuilder,
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
    pub ridl: BinaryRidlTable,
    pub capabilities: BinaryCapabilities,
    pub metadata: BinaryMetadataTable,
    pub debug: crate::BinaryDebugTable,
    pub custom_sections: Vec<Vec<u8>>,
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
            ridl: BinaryRidlTable::default(),
            capabilities: BinaryCapabilities::default(),
            metadata: BinaryMetadataTable::default(),
            debug: crate::BinaryDebugTable::default(),
            custom_sections: Vec::new(),
            analysis: BinaryAnalysisSummary::default(),
            assemblies: Vec::new(),
        }
    }

    /// Encodes modeled sections in kind order and canonical STRINGS order.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        validate_import_type_references(&self.imports, &self.types)?;
        validate_type_signature_references(&self.types, &self.signatures)?;
        validate_module_function_references(&self.module, &self.signatures)?;
        validate_metadata_references(&self.metadata, &self.code)?;
        validate_debug_references(&self.debug, &self.signatures, &self.code, &self.globals)?;
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
        if let Some(ridl) = self.ridl.encode(
            &strings,
            &self.types,
            &self.signatures,
            &self.generics,
            &self.globals,
            &self.code,
        )? {
            sections.push(Section::from_uncompressed(12, ridl));
        }
        sections.push(Section::from_uncompressed(
            13,
            self.capabilities.encode(&strings, &self.imports)?,
        ));
        sections.push(Section::from_uncompressed(
            14,
            self.analysis
                .encode(&self.signatures, &self.globals, &assembly_records)?,
        ));
        let metadata = self
            .metadata
            .encode(&strings)
            .map_err(|error| malformed(error.message))?;
        if !metadata.is_empty() {
            sections.push(Section::from_uncompressed(15, metadata));
        }
        if !assembly_records.is_empty() {
            sections.push(Section::from_uncompressed(
                16,
                crate::encode_assembly_table(&assembly_records, &strings)?,
            ));
        }

        if !self.debug.is_empty() {
            sections.push(Section::from_uncompressed(17, self.debug.encode(&strings)?));
        }
        for bytes in &self.custom_sections {
            sections.push(Section::from_uncompressed(255, bytes.clone()));
        }

        let mut header = self.header;
        header.flags = (header.flags & !0x7f) | self.canonical_header_flags();
        let module = BinaryModule {
            header,
            sections,
            content_hash: ContentHash::of(&[]),
        };
        let mut bytes = Vec::new();
        module.write_uncompressed_container(&mut bytes)?;
        Ok(bytes)
    }

    fn canonical_header_flags(&self) -> u32 {
        let defined = self.signatures.records.iter().all(|signature| {
            signature.semantics.unwrap_or(self.module.semantics) == SemanticsMode::Defined
        });
        let gpu = self
            .signatures
            .records
            .iter()
            .any(|signature| signature.target != BinaryTarget::Cpu);
        let runtime = self
            .signatures
            .records
            .iter()
            .any(|signature| signature.engine == Some(BinaryEngine::Runtime));
        let unmanaged = self
            .analysis
            .functions
            .iter()
            .any(|function| function.trust_class == 2);
        let sandboxed = self
            .analysis
            .functions
            .iter()
            .any(|function| function.trust_class == 1);
        u32::from(defined)
            | (u32::from(gpu) << 1)
            | (u32::from(unmanaged) << 2)
            | (u32::from(!self.analysis.assembly_regions.is_empty()) << 3)
            | (u32::from(runtime) << 4)
            | (u32::from(sandboxed) << 5)
            | (1 << 6)
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
        self.ridl.intern_strings(&mut strings)?;
        self.debug.intern_strings(&mut strings)?;
        for entitlement in self
            .capabilities
            .implied_entitlements
            .iter()
            .chain(&self.capabilities.requested_entitlements)
        {
            intern_string(&mut strings, entitlement)?;
        }
        for domain in &self.metadata.domains {
            intern_string(&mut strings, &domain.name)?;
        }
        for scope in &self.metadata.scopes {
            intern_string(&mut strings, &scope.name)?;
        }
        for attachment in &self.metadata.attachments {
            if let BinaryMetadata::Tbaa(name) = &attachment.metadata {
                intern_string(&mut strings, name)?;
            }
        }
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

fn intern_string(strings: &mut StringTableBuilder, value: &str) -> Result<(), FormatError> {
    strings
        .intern(value)
        .map(|_| ())
        .map_err(|error| malformed(error.message))
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

fn validate_metadata_references(
    metadata: &BinaryMetadataTable,
    code: &BinaryCodeTable,
) -> Result<(), FormatError> {
    for attachment in &metadata.attachments {
        let body = code
            .functions
            .iter()
            .find(|body| body.function_index == attachment.function_index)
            .ok_or_else(|| malformed("METADATA references a function without CODE"))?;
        let count = body
            .blocks
            .iter()
            .try_fold(0u64, |total, block| {
                total.checked_add(block.instruction_count)
            })
            .ok_or_else(|| malformed("CODE instruction ordinal overflow"))?;
        if attachment.instruction_ordinal >= count {
            return Err(malformed("METADATA instruction ordinal is out of range"));
        }
    }
    Ok(())
}

fn validate_debug_references(
    debug: &crate::BinaryDebugTable,
    signatures: &BinarySignatureTable,
    code: &BinaryCodeTable,
    globals: &BinaryGlobalTable,
) -> Result<(), FormatError> {
    for scope in &debug.scopes {
        require_signature(signatures, scope.function_index)?;
    }
    for location in &debug.locations {
        let body = code
            .functions
            .iter()
            .find(|body| body.function_index == location.function_index)
            .ok_or_else(|| malformed("DEBUG location references a function without CODE"))?;
        let total = body
            .blocks
            .iter()
            .try_fold(0u64, |sum, block| sum.checked_add(block.instruction_count))
            .ok_or_else(|| malformed("CODE instruction count overflows"))?;
        if location.instruction_ordinal >= total {
            return Err(malformed(
                "DEBUG location instruction ordinal is out of range",
            ));
        }
    }
    for record in &debug.data {
        if record.data_index >= globals.records.len() as u64 {
            return Err(malformed("DEBUG DATA index is outside GLOBALS"));
        }
    }
    for body in &code.functions {
        for instruction in body
            .blocks
            .iter()
            .flat_map(|block| block.instructions.iter().flatten())
        {
            if !matches!(instruction.opcode, 0x240..=0x242) {
                continue;
            }
            let index = instruction
                .operands
                .iter()
                .find_map(|operand| match operand {
                    crate::BinaryOperand::Index(index) => Some(*index),
                    _ => None,
                })
                .ok_or_else(|| malformed("DEBUG instruction is missing a record index"))?;
            let scope = if instruction.opcode == 0x242 {
                debug.labels.get(index as usize).map(|record| record.scope)
            } else {
                debug
                    .variables
                    .get(index as usize)
                    .map(|record| record.scope)
            }
            .ok_or_else(|| malformed("DEBUG instruction refers to a missing variable or label"))?;
            if debug
                .scopes
                .get(scope as usize)
                .is_none_or(|scope| scope.function_index != body.function_index)
            {
                return Err(malformed(
                    "DEBUG instruction references a record from another function",
                ));
            }
        }
    }
    Ok(())
}

fn malformed(message: impl Into<String>) -> FormatError {
    FormatError::malformed(None, message)
}
