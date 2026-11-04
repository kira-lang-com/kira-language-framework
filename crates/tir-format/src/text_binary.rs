use std::collections::HashMap;
use std::fmt::Write as _;

use tir_target::Platform;
use tir_types::{
    Abi, AddressSpace, ClosureType, Convention, CooperativeMatrixUse, EntryKind, FunctionType,
    MatrixScope, ParameterType, TextureAccess, TextureDimension, Type,
};

use crate::text::{
    Token, TokenKind, decode_kira_byte_string, decode_kira_string_literal_contents, lex,
};
use crate::{
    BinaryAddressSpace, BinaryAnalysisSummary, BinaryAnnotation, BinaryBlock, BinaryBlockParameter,
    BinaryCapabilities, BinaryCodeTable, BinaryConstantTable, BinaryConstantValue,
    BinaryCopyPolicy, BinaryDestroyPolicy, BinaryEngine, BinaryField, BinaryFpMode,
    BinaryFunctionAnalysis, BinaryFunctionBody, BinaryFunctionParameter, BinaryFunctionType,
    BinaryGenericStrategy, BinaryGlobal, BinaryGlobalTable, BinaryImport, BinaryImportEntityKind,
    BinaryImportKind, BinaryImportedEntity, BinaryInstruction, BinaryLayout, BinaryLinkage,
    BinaryModule, BinaryModuleRecord, BinaryNominalDeclaration, BinaryNominalDefinition,
    BinaryNominalKind, BinaryOperand, BinaryPolicy, BinaryProducer, BinaryRelocatePolicy,
    BinarySignature, BinarySignatureTable, BinaryTarget, BinaryType, BinaryTypeTable,
    BinaryVariant, BinaryVariantAttribute, FormatError, FunctionBody, Header, Section,
    SemanticsMode, StringTable, TIRB_FLAG_CANONICAL, TextItemKind, TextModule, encode_imports,
    parse_function_body, parse_function_signature, parse_text_module,
};

/// Encodes module metadata, imports, nominal types, and an initial function CODE subset into canonical TIRB.
pub fn assemble_text_module(module: &TextModule) -> Result<Vec<u8>, FormatError> {
    if !module.header.initializers.is_empty() || !module.header.finalizers.is_empty() {
        return Err(FormatError::malformed(
            None,
            "module init/fini records require function declarations",
        ));
    }

    let mut values = vec![String::new()];
    add_string(&mut values, &module.header.path);
    if let Some(producer) = &module.header.producer {
        add_string(&mut values, &producer.name);
        add_string(&mut values, &producer.version);
        for language in &producer.languages {
            add_string(&mut values, language);
        }
    }
    for (key, value) in &module.header.attributes {
        add_string(&mut values, key);
        add_string(&mut values, value);
    }
    let mut parsed_imports = Vec::new();
    let mut parsed_types = Vec::new();
    let mut parsed_globals = Vec::new();
    let mut parsed_functions = Vec::new();
    for item in &module.items {
        match item.kind {
            TextItemKind::Import => {
                parsed_imports.push(parse_import(module.item_text(*item))?);
            }
            TextItemKind::Type => {
                parsed_types.push(parse_explicit_struct(module.item_text(*item))?);
            }
            TextItemKind::Global => {
                parsed_globals.push(parse_text_global(module.item_text(*item))?);
            }
            TextItemKind::Function => {
                parsed_functions.push(parse_text_function(module.item_text(*item))?);
            }
            _ => {
                return Err(FormatError::malformed(
                    Some(item.span.start),
                    "this TIR assembler pass supports imports, nominal types, uninitialized globals, and function declarations with single return or unreachable bodies",
                ));
            }
        }
    }
    for import in &parsed_imports {
        add_string(&mut values, &import.name);
        for entity in &import.entities {
            add_string(&mut values, &entity.name);
            if let Some(symbol) = &entity.platform_symbol {
                add_string(&mut values, symbol);
            }
        }
    }
    for parsed_type in &parsed_types {
        add_string(&mut values, &parsed_type.declaration.name);
    }
    for parsed_type in &parsed_types {
        for field in &parsed_type.fields {
            add_string(&mut values, &field.name);
            for annotation in &field.annotations {
                add_string(&mut values, &annotation.name);
                for argument in &annotation.arguments {
                    add_string(&mut values, argument);
                }
            }
        }
        for variant in &parsed_type.variants {
            add_string(&mut values, &variant.name);
        }
    }
    for global in &parsed_globals {
        add_string(&mut values, &global.name);
        if let Some(BinaryConstantValue::String(value)) = &global.initializer {
            add_string(&mut values, value);
        }
    }
    for function in &parsed_functions {
        add_string(&mut values, &function.name);
        if let Some(platform_export) = &function.signature.platform_export {
            add_string(&mut values, platform_export);
        }
        if let Some(variant) = &function.signature.variant {
            for feature in &variant.features {
                add_string(&mut values, feature);
            }
        }
    }
    let strings = StringTable::from_values(values).map_err(|error| {
        FormatError::malformed(usize::try_from(error.offset).ok(), error.message)
    })?;
    let source_hash = module
        .header
        .source_hash
        .as_deref()
        .map(parse_source_hash)
        .transpose()?;
    let record = BinaryModuleRecord {
        name: module.header.path.clone(),
        producer: module
            .header
            .producer
            .as_ref()
            .map(|producer| BinaryProducer {
                name: producer.name.clone(),
                version: producer.version.clone(),
                languages: producer.languages.clone(),
            }),
        semantics: module.header.semantics.unwrap_or(SemanticsMode::Defined),
        attributes: module.header.attributes.clone(),
        initializers: Vec::new(),
        finalizers: Vec::new(),
        source_hash,
    };
    let module_bytes = record.encode(&strings, module.header.profile)?;
    let mut strings_bytes = Vec::new();
    strings
        .encode(&mut strings_bytes)
        .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    let capabilities = BinaryCapabilities::default().encode(&strings, &[])?;
    let declarations = parsed_types
        .iter()
        .map(|parsed_type| parsed_type.declaration.clone())
        .collect::<Vec<_>>();
    let declaration_indices = declarations
        .iter()
        .enumerate()
        .map(|(index, declaration)| {
            u64::try_from(index)
                .map(|index| (declaration.name.clone(), index))
                .map_err(|_| FormatError::malformed(None, "nominal declaration index overflows"))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut entries = Vec::new();
    let mut imports = Vec::with_capacity(parsed_imports.len());
    for parsed_import in parsed_imports {
        let mut entities = Vec::with_capacity(parsed_import.entities.len());
        for entity in parsed_import.entities {
            let type_index = intern_type(&mut entries, &entity.ty, &declaration_indices)?;
            entities.push(BinaryImportedEntity {
                kind: entity.kind,
                name: entity.name,
                platform_symbol: entity.platform_symbol,
                type_index,
                required_entitlements: Vec::new(),
                implied_entitlements: Vec::new(),
            });
        }
        imports.push(BinaryImport {
            kind: parsed_import.kind,
            name: parsed_import.name,
            abi_uuid: parsed_import.abi_uuid,
            weak: parsed_import.weak,
            entities,
        });
    }
    let import_bytes = encode_imports(&imports, &strings)?;
    let mut definitions = Vec::with_capacity(parsed_types.len());
    for (declaration_index, parsed_type) in parsed_types.iter().enumerate() {
        let declaration_index = u64::try_from(declaration_index)
            .map_err(|_| FormatError::malformed(None, "nominal declaration index overflows"))?;
        let mut fields = Vec::with_capacity(parsed_type.fields.len());
        for parsed_field in &parsed_type.fields {
            let type_index = intern_type(&mut entries, &parsed_field.ty, &declaration_indices)?;
            fields.push(BinaryField {
                name: parsed_field.name.clone(),
                ty: type_index,
                offset: parsed_field.offset,
                bit_offset: parsed_field.bit_offset,
                bit_width: parsed_field.bit_width,
                annotations: parsed_field.annotations.clone(),
            });
        }
        let mut variants = Vec::with_capacity(parsed_type.variants.len());
        for parsed_variant in &parsed_type.variants {
            let payload = parsed_variant
                .payload
                .iter()
                .map(|ty| intern_type(&mut entries, ty, &declaration_indices))
                .collect::<Result<_, _>>()?;
            variants.push(BinaryVariant {
                name: parsed_variant.name.clone(),
                payload,
            });
        }
        definitions.push(BinaryNominalDefinition {
            declaration: declaration_index,
            layout: parsed_type.layout,
            fields,
            variants,
            policy: parsed_type.policy.clone(),
        });
    }
    let mut globals = Vec::with_capacity(parsed_globals.len());
    for parsed_global in parsed_globals {
        let ty = intern_type(&mut entries, &parsed_global.ty, &declaration_indices)?;
        globals.push(BinaryGlobal {
            name: parsed_global.name,
            linkage: parsed_global.linkage,
            ty,
            address_space: parsed_global.address_space,
            thread_local: parsed_global.thread_local,
            constant: parsed_global.constant,
            per_cpu: parsed_global.per_cpu,
            alignment: parsed_global.alignment,
            initializer: None,
        });
    }
    let global_table = BinaryGlobalTable { records: globals };
    let global_bytes = if global_table.records.is_empty() {
        None
    } else {
        let global_type_table = BinaryTypeTable {
            declarations: declarations.clone(),
            entries: entries.clone(),
            definitions: definitions.clone(),
        };
        Some(global_table.encode(
            &strings,
            &global_type_table,
            &BinaryConstantTable::default(),
        )?)
    };
    let mut signatures = Vec::with_capacity(parsed_functions.len());
    for function in &parsed_functions {
        let function_type = Type::Function(function.function_type.clone());
        let function_type = intern_type(&mut entries, &function_type, &declaration_indices)?;
        let coroutine = function
            .signature
            .coroutine
            .as_ref()
            .map(|(yield_type, resume_type)| {
                Ok((
                    intern_type(&mut entries, yield_type, &declaration_indices)?,
                    intern_type(&mut entries, resume_type, &declaration_indices)?,
                ))
            })
            .transpose()?;
        let instance_types = function
            .signature
            .instance_types
            .iter()
            .map(|ty| intern_type(&mut entries, ty, &declaration_indices))
            .collect::<Result<Vec<_>, _>>()?;
        signatures.push(BinarySignature {
            name: function.name.clone(),
            linkage: function.linkage,
            function_type,
            generic_parameters: Vec::new(),
            modifiers: function.signature.modifiers,
            structured: function.signature.structured,
            kernel_fp: function.signature.kernel_fp,
            returns_twice: function.signature.returns_twice,
            inlinable: function.signature.inlinable,
            engine: function.signature.engine,
            semantics: function.signature.semantics,
            target: function.signature.target,
            fp_mode: function.signature.fp_mode,
            generic_strategy: function.signature.generic_strategy,
            coroutine,
            instance_types,
            variant: function.signature.variant.clone(),
            entry_kind: None,
            platform_export: function.signature.platform_export.clone(),
        });
    }
    let type_table = BinaryTypeTable {
        declarations,
        entries,
        definitions,
    };
    let type_bytes = if type_table.declarations.is_empty() && type_table.entries.is_empty() {
        None
    } else {
        Some(type_table.encode(&strings)?)
    };
    let signature_table = BinarySignatureTable {
        records: signatures,
    };
    let signature_bytes = if signature_table.records.is_empty() {
        None
    } else {
        Some(signature_table.encode(&strings, &type_table)?)
    };
    let code_table = BinaryCodeTable {
        functions: parsed_functions
            .iter()
            .enumerate()
            .filter_map(|(index, function)| function.body.as_ref().map(|body| (index, body)))
            .map(|(index, body)| {
                let function_index = u64::try_from(index)
                    .map_err(|_| FormatError::malformed(None, "function index overflows"))?;
                lower_simple_function_body(function_index, body, &type_table, &signature_table)
            })
            .collect::<Result<Vec<_>, _>>()?,
    };
    let code_bytes = if code_table.functions.is_empty() {
        None
    } else {
        Some(code_table.encode(
            &signature_table,
            &type_table,
            &BinaryConstantTable::default(),
            &strings,
            0,
        )?)
    };
    let analysis = BinaryAnalysisSummary {
        functions: vec![
            BinaryFunctionAnalysis {
                trust_class: 0,
                reason_mask: 0,
            };
            signature_table.records.len()
        ],
        ..BinaryAnalysisSummary::default()
    }
    .encode()?;
    let mut flags = TIRB_FLAG_CANONICAL;
    if signature_table.records.iter().all(|signature| {
        signature
            .semantics
            .unwrap_or_else(|| module.header.semantics.unwrap_or(SemanticsMode::Defined))
            == SemanticsMode::Defined
    }) {
        flags |= 1 << 0;
    }
    if code_table.functions.iter().any(|function| {
        usize::try_from(function.function_index)
            .ok()
            .and_then(|index| signature_table.records.get(index))
            .is_some_and(|signature| {
                matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both)
            })
    }) {
        flags |= 1 << 1;
    }
    if signature_table
        .records
        .iter()
        .any(|signature| signature.engine == Some(BinaryEngine::Runtime))
    {
        flags |= 1 << 4;
    }
    let header = Header {
        year: module.header.version.year,
        month: module.header.version.month,
        week: module.header.version.week,
        revision: 0,
        flags,
        profile: module.header.profile,
        platform: module.header.platform.unwrap_or(Platform::Portable),
        section_count: 0,
        file_size: 0,
    };
    let mut sections = vec![
        Section::from_uncompressed(1, strings_bytes),
        Section::from_uncompressed(2, module_bytes),
    ];
    if !imports.is_empty() {
        sections.push(Section::from_uncompressed(3, import_bytes));
    }
    if let Some(type_bytes) = type_bytes {
        sections.push(Section::from_uncompressed(4, type_bytes));
    }
    if let Some(global_bytes) = global_bytes {
        sections.push(Section::from_uncompressed(6, global_bytes));
    }
    if let Some(signature_bytes) = signature_bytes {
        sections.push(Section::from_uncompressed(7, signature_bytes));
    }
    if let Some(code_bytes) = code_bytes {
        sections.push(Section::from_uncompressed(8, code_bytes));
    }
    sections.push(Section::from_uncompressed(13, capabilities));
    sections.push(Section::from_uncompressed(14, analysis));
    let binary = BinaryModule::from_sections(header, sections);
    let mut output = Vec::new();
    binary.write_uncompressed_container(&mut output)?;
    Ok(output)
}

/// Renders the canonical TIRB subset with metadata, imports, types, signatures,
/// and one-block return or unreachable bodies.
pub fn disassemble_binary_module(module: &BinaryModule) -> Result<String, FormatError> {
    if module.header.flags & TIRB_FLAG_CANONICAL == 0 || module.header.flags & !0x7f != 0 {
        return Err(FormatError::malformed(
            None,
            "metadata disassembly requires a canonical module with no other header flags",
        ));
    }
    let section_kinds = module
        .sections
        .iter()
        .map(|section| section.kind)
        .collect::<Vec<_>>();
    if !is_supported_declaration_sections(&section_kinds) {
        return Err(FormatError::malformed(
            None,
            "declaration disassembly supports STRINGS, MODULE, IMPORTS, TYPES, GLOBALS, SIGNATURES, CODE, CAPS, and ANALYSIS sections",
        ));
    }
    let record = module.module_record(u64::MAX)?;
    if !record.initializers.is_empty() || !record.finalizers.is_empty() {
        return Err(FormatError::malformed(
            None,
            "module init/fini records require SIGNATURES",
        ));
    }
    if module.capabilities(u64::MAX)? != BinaryCapabilities::default() {
        return Err(FormatError::malformed(
            None,
            "metadata disassembly cannot represent nonempty capabilities",
        ));
    }
    let imports = module.imports(u64::MAX)?;
    for import in &imports {
        for entity in &import.entities {
            if !entity.required_entitlements.is_empty() || !entity.implied_entitlements.is_empty() {
                return Err(FormatError::malformed(
                    None,
                    "TIR text import syntax cannot represent binary entitlement metadata",
                ));
            }
        }
    }
    let analysis = module.analysis_summary(u64::MAX)?;
    if analysis
        .functions
        .iter()
        .any(|function| function.trust_class != 0 || function.reason_mask != 0)
        || !analysis.sandbox_regions.is_empty()
        || !analysis.indirect_call_typeids.is_empty()
        || !analysis.assembly_regions.is_empty()
    {
        return Err(FormatError::malformed(
            None,
            "metadata disassembly cannot represent nonempty analysis records",
        ));
    }
    let type_table = module.type_table(u64::MAX)?;
    let signatures = module.signature_table(u64::MAX, &type_table)?;
    let constants = module.constant_table(u64::MAX, &type_table)?;
    let globals = module.global_table(u64::MAX, &type_table, &constants)?;
    if globals
        .records
        .iter()
        .any(|global| global.initializer.is_some())
    {
        return Err(FormatError::malformed(
            None,
            "global initializers require CONSTANTS text disassembly",
        ));
    }
    if signatures
        .records
        .iter()
        .any(|signature| signature.entry_kind.is_some())
    {
        return Err(FormatError::malformed(
            None,
            "entry signatures require GPU entry metadata for text disassembly",
        ));
    }
    let code = module.code_table(u64::MAX, &signatures)?;
    let code_by_function = code
        .functions
        .iter()
        .map(|function| (function.function_index, function))
        .collect::<HashMap<_, _>>();
    let mut expected_flags = TIRB_FLAG_CANONICAL;
    if signatures
        .records
        .iter()
        .all(|signature| signature.semantics.unwrap_or(record.semantics) == SemanticsMode::Defined)
    {
        expected_flags |= 1 << 0;
    }
    if code.functions.iter().any(|function| {
        usize::try_from(function.function_index)
            .ok()
            .and_then(|index| signatures.records.get(index))
            .is_some_and(|signature| {
                matches!(signature.target, BinaryTarget::Gpu | BinaryTarget::Both)
            })
    }) {
        expected_flags |= 1 << 1;
    }
    if analysis
        .functions
        .iter()
        .any(|function| function.trust_class == 2)
    {
        expected_flags |= 1 << 2;
    }
    if !analysis.assembly_regions.is_empty() {
        expected_flags |= 1 << 3;
    }
    if signatures
        .records
        .iter()
        .any(|signature| signature.engine == Some(BinaryEngine::Runtime))
    {
        expected_flags |= 1 << 4;
    }
    if analysis
        .functions
        .iter()
        .any(|function| function.trust_class == 1)
    {
        expected_flags |= 1 << 5;
    }
    if module.header.flags != expected_flags {
        return Err(FormatError::malformed(
            None,
            "module summary flags disagree with SIGNATURES, CODE, or ANALYSIS",
        ));
    }

    let mut lines = vec![format!(
        "module @{} tir {}.{}.{} profile {} platform {} semantics {}",
        record.name,
        module.header.year,
        module.header.month,
        module.header.week,
        module.header.profile.name(),
        module.header.platform.name(),
        match record.semantics {
            SemanticsMode::Defined => "defined",
            SemanticsMode::Permissive => "permissive",
        }
    )];
    if let Some(producer) = record.producer {
        lines.push(format!(
            "  producer {} {} languages [{}]",
            quote(&producer.name)?,
            quote(&producer.version)?,
            producer.languages.join(", ")
        ));
    }
    for (key, value) in record.attributes {
        lines.push(format!("  attribute {} = {}", quote(&key)?, quote(&value)?));
    }
    if let Some(source_hash) = record.source_hash {
        let mut hash = String::with_capacity(64);
        for byte in source_hash {
            write!(&mut hash, "{byte:02x}")
                .map_err(|error| FormatError::malformed(None, error.to_string()))?;
        }
        lines.push(format!("  source {}", quote(&hash)?));
    }
    let mut import_type_roots = Vec::new();
    for import in imports {
        lines.push(render_import(&import, &type_table, &mut import_type_roots)?);
    }
    let signature_type_roots = signatures
        .records
        .iter()
        .flat_map(|signature| {
            std::iter::once(signature.function_type)
                .chain(
                    signature
                        .coroutine
                        .into_iter()
                        .flat_map(|(yield_ty, resume_ty)| [yield_ty, resume_ty]),
                )
                .chain(signature.instance_types.iter().copied())
        })
        .collect::<Vec<_>>();
    let global_type_roots = globals
        .records
        .iter()
        .map(|global| global.ty)
        .collect::<Vec<_>>();
    let mut post_type_roots = global_type_roots;
    post_type_roots.extend(signature_type_roots);
    if let Some(type_items) =
        render_explicit_structs(&type_table, &import_type_roots, &post_type_roots)?
    {
        lines.extend(type_items);
    }
    for global in &globals.records {
        lines.push(render_global(global, &type_table)?);
    }
    for (index, signature) in signatures.records.iter().enumerate() {
        let function_index = u64::try_from(index)
            .map_err(|_| FormatError::malformed(None, "signature index overflows"))?;
        lines.push(render_signature(
            signature,
            &type_table,
            code_by_function.get(&function_index).copied(),
        )?);
    }
    let source = format!("{}\n", lines.join("\n"));
    parse_text_module(&source).map_err(|error| {
        FormatError::malformed(
            Some(error.span.start),
            format!("cannot render text TIR: {}", error.message),
        )
    })?;
    Ok(source)
}

fn parse_import(source: &str) -> Result<ParsedTextImport, FormatError> {
    let mut tokens = lex(source).map_err(|error| {
        FormatError::malformed(
            Some(error.span.start),
            format!("invalid import: {}", error.message),
        )
    })?;
    tokens.retain(|token| !matches!(token.kind, TokenKind::End));
    while tokens
        .first()
        .is_some_and(|token| matches!(token.kind, TokenKind::Newline))
    {
        tokens.remove(0);
    }
    while tokens
        .last()
        .is_some_and(|token| matches!(token.kind, TokenKind::Newline))
    {
        tokens.pop();
    }
    let mut cursor = 0;
    expect_atom(&tokens, &mut cursor, "import")?;
    let kind = take_atom(&tokens, &mut cursor)?;
    let mut import = match kind.as_str() {
        "package" => {
            expect_punctuation(&tokens, &mut cursor, '@')?;
            let name = take_path(&tokens, &mut cursor)?;
            ParsedTextImport {
                kind: BinaryImportKind::Package,
                name,
                abi_uuid: [0; 16],
                weak: false,
                entities: Vec::new(),
            }
        }
        "ridl" => {
            expect_punctuation(&tokens, &mut cursor, '@')?;
            let name = take_path(&tokens, &mut cursor)?;
            expect_atom(&tokens, &mut cursor, "abi")?;
            let uuid_offset = tokens.get(cursor).map(|token| token.span.start);
            let uuid = take_string(&tokens, &mut cursor)?;
            let abi_uuid = parse_abi_uuid(&uuid).ok_or_else(|| {
                FormatError::malformed(uuid_offset, "RIDL ABI must be a canonical UUID string")
            })?;
            ParsedTextImport {
                kind: BinaryImportKind::Ridl,
                name,
                abi_uuid,
                weak: false,
                entities: Vec::new(),
            }
        }
        "platform" => {
            let name = take_string(&tokens, &mut cursor)?;
            let weak = if tokens.get(cursor).and_then(atom) == Some("weak") {
                cursor += 1;
                true
            } else {
                false
            };
            ParsedTextImport {
                kind: BinaryImportKind::Platform,
                name,
                abi_uuid: [0; 16],
                weak,
                entities: Vec::new(),
            }
        }
        _ => {
            return Err(FormatError::malformed(
                tokens.get(1).map(|token| token.span.start),
                "expected package, RIDL, or platform import",
            ));
        }
    };
    skip_newlines(&tokens, &mut cursor);
    if tokens
        .get(cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('{')))
    {
        cursor += 1;
        skip_newlines(&tokens, &mut cursor);
        while !tokens
            .get(cursor)
            .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('}')))
        {
            if cursor >= tokens.len() {
                return Err(FormatError::malformed(
                    None,
                    "unterminated import entity block",
                ));
            }
            let entity_name = take_atom(&tokens, &mut cursor)?;
            let (entity_kind, is_platform_entity) = match (import.kind, entity_name.as_str()) {
                (BinaryImportKind::Platform, "func") => (BinaryImportEntityKind::Function, true),
                (BinaryImportKind::Platform, "data") => {
                    (BinaryImportEntityKind::PlatformData, true)
                }
                (BinaryImportKind::Platform, _) => {
                    return Err(FormatError::malformed(
                        tokens.get(cursor - 1).map(|token| token.span.start),
                        "platform imports contain only func and data entities",
                    ));
                }
                (_, "func") => (BinaryImportEntityKind::Function, false),
                (_, "global") => (BinaryImportEntityKind::Global, false),
                (_, "type") => (BinaryImportEntityKind::Type, false),
                (_, "witness") => (BinaryImportEntityKind::Witness, false),
                (_, "descriptor") => (BinaryImportEntityKind::Descriptor, false),
                _ => {
                    return Err(FormatError::malformed(
                        tokens.get(cursor - 1).map(|token| token.span.start),
                        "unknown imported entity kind",
                    ));
                }
            };
            expect_punctuation(&tokens, &mut cursor, '@')?;
            let name = take_path(&tokens, &mut cursor)?;
            let platform_symbol = if is_platform_entity {
                expect_atom(&tokens, &mut cursor, "symbol")?;
                Some(take_string(&tokens, &mut cursor)?)
            } else {
                None
            };
            expect_punctuation(&tokens, &mut cursor, ':')?;
            let ty = take_type(source, &tokens, &mut cursor)?;
            import.entities.push(ParsedTextImportEntity {
                kind: entity_kind,
                name,
                platform_symbol,
                ty,
            });
            skip_newlines(&tokens, &mut cursor);
            if tokens
                .get(cursor)
                .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(',')))
            {
                cursor += 1;
                skip_newlines(&tokens, &mut cursor);
            }
        }
        cursor += 1;
        skip_newlines(&tokens, &mut cursor);
    }
    if cursor < tokens.len() {
        return Err(FormatError::malformed(
            tokens.get(cursor).map(|token| token.span.start),
            "unexpected token after import declaration",
        ));
    }
    Ok(import)
}

fn skip_newlines(tokens: &[Token], cursor: &mut usize) {
    while tokens
        .get(*cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Newline))
    {
        *cursor += 1;
    }
}

fn take_path(tokens: &[Token], cursor: &mut usize) -> Result<String, FormatError> {
    let mut path = take_atom(tokens, cursor)?;
    while tokens
        .get(*cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('.')))
    {
        *cursor += 1;
        path.push('.');
        path.push_str(&take_atom(tokens, cursor)?);
    }
    Ok(path)
}

fn take_string(tokens: &[Token], cursor: &mut usize) -> Result<String, FormatError> {
    let Some(Token {
        kind: TokenKind::String(raw),
        span,
    }) = tokens.get(*cursor)
    else {
        return Err(FormatError::malformed(
            tokens.get(*cursor).map(|token| token.span.start),
            "expected a quoted string",
        ));
    };
    let value = decode_kira_string_literal_contents(raw).map_err(|error| {
        FormatError::malformed(Some(span.start), format!("invalid string literal: {error}"))
    })?;
    *cursor += 1;
    Ok(value)
}

fn parse_abi_uuid(value: &str) -> Option<[u8; 16]> {
    if value.len() != 36
        || ![8, 13, 18, 23]
            .into_iter()
            .all(|index| value.as_bytes().get(index) == Some(&b'-'))
    {
        return None;
    }
    let mut bytes = [0; 16];
    let mut digits = value.bytes().filter(|byte| *byte != b'-');
    for byte in &mut bytes {
        let high = hex_value(digits.next()?)?;
        let low = hex_value(digits.next()?)?;
        *byte = (high << 4) | low;
    }
    digits.next().is_none().then_some(bytes)
}

fn render_import(
    import: &BinaryImport,
    types: &BinaryTypeTable,
    type_roots: &mut Vec<u64>,
) -> Result<String, FormatError> {
    let header = match import.kind {
        BinaryImportKind::Package => Ok(format!("import package @{}", import.name)),
        BinaryImportKind::Ridl => Ok(format!(
            "import ridl @{} abi {}",
            import.name,
            quote(&format_abi_uuid(&import.abi_uuid))?
        )),
        BinaryImportKind::Platform => Ok(format!(
            "import platform {}{}",
            quote(&import.name)?,
            if import.weak { " weak" } else { "" }
        )),
    }?;
    if import.entities.is_empty() {
        return Ok(header);
    }
    let mut entities = Vec::with_capacity(import.entities.len());
    for entity in &import.entities {
        let kind = match (import.kind, entity.kind) {
            (_, BinaryImportEntityKind::Function) => "func",
            (BinaryImportKind::Platform, BinaryImportEntityKind::PlatformData) => "data",
            (BinaryImportKind::Platform, _) => {
                return Err(FormatError::malformed(
                    None,
                    "platform import has an unsupported entity kind",
                ));
            }
            (_, BinaryImportEntityKind::Global) => "global",
            (_, BinaryImportEntityKind::Type) => "type",
            (_, BinaryImportEntityKind::Witness) => "witness",
            (_, BinaryImportEntityKind::Descriptor) => "descriptor",
            (_, BinaryImportEntityKind::PlatformData) => {
                return Err(FormatError::malformed(
                    None,
                    "non-platform import has platform data",
                ));
            }
        };
        let is_platform = import.kind == BinaryImportKind::Platform;
        let rendered_type = binary_type_to_tir(
            &types.entries,
            &types.declarations,
            entity.type_index,
            None,
            0,
        )?;
        type_roots.push(entity.type_index);
        let symbol = if is_platform {
            let symbol = entity.platform_symbol.as_deref().ok_or_else(|| {
                FormatError::malformed(None, "platform import entity is missing its symbol")
            })?;
            format!(" symbol {}", quote(symbol)?)
        } else {
            if entity.platform_symbol.is_some() {
                return Err(FormatError::malformed(
                    None,
                    "non-platform import entity has a platform symbol",
                ));
            }
            String::new()
        };
        entities.push(format!(
            "  {kind} @{}{symbol}: {rendered_type}",
            entity.name
        ));
    }
    Ok(format!("{header} {{\n{}\n}}", entities.join("\n")))
}

fn format_abi_uuid(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct ParsedTextStruct {
    declaration: BinaryNominalDeclaration,
    layout: BinaryLayout,
    fields: Vec<ParsedTextField>,
    variants: Vec<ParsedTextVariant>,
    policy: BinaryPolicy,
}

#[derive(Clone, Debug)]
struct ParsedTextVariant {
    name: String,
    payload: Vec<Type>,
}

#[derive(Clone, Debug)]
struct ParsedTextImport {
    kind: BinaryImportKind,
    name: String,
    abi_uuid: [u8; 16],
    weak: bool,
    entities: Vec<ParsedTextImportEntity>,
}

#[derive(Clone, Debug)]
struct ParsedTextImportEntity {
    kind: BinaryImportEntityKind,
    name: String,
    platform_symbol: Option<String>,
    ty: Type,
}

#[derive(Clone, Debug)]
struct ParsedTextField {
    name: String,
    ty: Type,
    offset: Option<u64>,
    bit_offset: Option<u64>,
    bit_width: Option<u64>,
    annotations: Vec<BinaryAnnotation>,
}

#[derive(Clone, Debug)]
struct ParsedTextGlobal {
    name: String,
    linkage: BinaryLinkage,
    ty: Type,
    address_space: BinaryAddressSpace,
    thread_local: bool,
    constant: bool,
    per_cpu: bool,
    alignment: Option<u64>,
    initializer: Option<BinaryConstantValue>,
}

fn parse_text_global(source: &str) -> Result<ParsedTextGlobal, FormatError> {
    let mut tokens = lex(source).map_err(|error| {
        FormatError::malformed(
            Some(error.span.start),
            format!("invalid global: {}", error.message),
        )
    })?;
    tokens.retain(|token| !matches!(token.kind, TokenKind::Newline | TokenKind::End));
    let mut cursor = 0;
    expect_atom(&tokens, &mut cursor, "global")?;
    let linkage = parse_linkage(&tokens, &mut cursor)?;
    expect_punctuation(&tokens, &mut cursor, '@')?;
    let name = take_path(&tokens, &mut cursor)?;
    expect_punctuation(&tokens, &mut cursor, ':')?;
    let type_start = cursor;
    let mut parsed_type = None;
    for end in type_start + 1..=tokens.len() {
        let start_offset = tokens[type_start].span.start;
        let end_offset = tokens[end - 1].span.end;
        let Some(type_text) = source.get(start_offset..end_offset) else {
            continue;
        };
        if let Ok(ty) = crate::parse_type(type_text) {
            parsed_type = Some((ty, end));
        }
    }
    let (ty, end) = parsed_type.ok_or_else(|| {
        malformed_at(
            &tokens,
            type_start,
            "global declaration is missing a valid type",
        )
    })?;
    cursor = end;
    let mut address_space = BinaryAddressSpace::Generic;
    if tokens
        .get(cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('<')))
    {
        cursor += 1;
        address_space = parse_global_address_space(&take_atom(&tokens, &mut cursor)?)?;
        expect_punctuation(&tokens, &mut cursor, '>')?;
    }
    let initializer = if tokens
        .get(cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('=')))
    {
        cursor += 1;
        let (value, consumed) = parse_global_constant(source, &tokens[cursor..], &ty)?;
        cursor += consumed;
        Some(value)
    } else {
        None
    };
    let mut thread_local = false;
    let mut constant = false;
    let mut per_cpu = false;
    let mut alignment = None;
    let mut seen = std::collections::HashSet::new();
    while cursor < tokens.len() {
        let attr_offset = tokens[cursor].span.start;
        let attribute = take_atom(&tokens, &mut cursor)?;
        if !seen.insert(attribute.clone()) {
            return Err(FormatError::malformed(
                Some(attr_offset),
                format!("duplicate global attribute `{attribute}`"),
            ));
        }
        match attribute.as_str() {
            "thread_local" => thread_local = true,
            "constant" => constant = true,
            "percpu" => per_cpu = true,
            "align" => {
                let arguments = take_attribute_arguments(&tokens, &mut cursor)?;
                expect_single_atom(&arguments, "align")?;
                alignment = Some(
                    parse_unsigned_number(atom(&arguments[0][0]).unwrap_or_default()).ok_or_else(
                        || {
                            malformed_at(
                                &tokens,
                                cursor,
                                "global alignment must be a nonnegative integer",
                            )
                        },
                    )?,
                );
            }
            _ => {
                return Err(FormatError::malformed(
                    Some(attr_offset),
                    format!("unsupported global attribute `{attribute}`"),
                ));
            }
        }
    }
    Ok(ParsedTextGlobal {
        name,
        linkage,
        ty,
        address_space,
        thread_local,
        constant,
        per_cpu,
        alignment,
        initializer,
    })
}

fn parse_global_constant(
    source: &str,
    tokens: &[Token],
    ty: &Type,
) -> Result<(BinaryConstantValue, usize), FormatError> {
    let Some(first) = tokens.first() else {
        return Err(FormatError::malformed(
            None,
            "global initializer is missing a constant",
        ));
    };
    if let TokenKind::String(raw) = &first.kind {
        let value = decode_kira_string_literal_contents(raw).map_err(|error| {
            FormatError::malformed(
                Some(first.span.start),
                format!("invalid string literal: {error}"),
            )
        })?;
        return Ok((BinaryConstantValue::String(value), 1));
    }
    if atom(first) == Some("b") {
        let Some(Token {
            kind: TokenKind::String(raw),
            span,
        }) = tokens.get(1)
        else {
            return Err(malformed_at(
                tokens,
                1,
                "byte-string constant requires a quoted string",
            ));
        };
        let value = decode_kira_byte_string(raw).map_err(|error| {
            FormatError::malformed(Some(span.start), format!("invalid byte string: {error}"))
        })?;
        return Ok((BinaryConstantValue::Bytes(value), 2));
    }
    match atom(first) {
        Some("true") => {
            if *ty != Type::Integer(1) {
                return Err(FormatError::malformed(
                    Some(first.span.start),
                    "boolean global initializer requires type i1",
                ));
            }
            return Ok((integer_constant(1, 1)?, 1));
        }
        Some("false") => {
            if *ty != Type::Integer(1) {
                return Err(FormatError::malformed(
                    Some(first.span.start),
                    "boolean global initializer requires type i1",
                ));
            }
            return Ok((integer_constant(0, 1)?, 1));
        }
        Some("null") => return Ok((BinaryConstantValue::Null, 1)),
        _ => {}
    }
    let consumed = if matches!(first.kind, TokenKind::Punctuation('-' | '+'))
        && tokens.get(1).is_some_and(|token| atom(token).is_some())
    {
        2
    } else {
        1
    };
    let last = &tokens[consumed - 1];
    let literal = source
        .get(first.span.start..last.span.end)
        .ok_or_else(|| FormatError::malformed(Some(first.span.start), "invalid constant span"))?;
    if let Type::Integer(bits) = ty {
        return Ok((integer_constant_from_literal(literal, *bits)?, consumed));
    }
    Err(FormatError::malformed(
        Some(first.span.start),
        "this TIR CONSTANTS pass supports integer, boolean, null, string, and byte-string literals",
    ))
}

fn integer_constant_from_literal(
    literal: &str,
    bits: u16,
) -> Result<BinaryConstantValue, FormatError> {
    let width = usize::from(bits);
    if width == 0 || width > 128 {
        return Err(FormatError::malformed(
            None,
            "integer constant width must be in 1..=128",
        ));
    }
    let raw = if let Some(magnitude) = literal.strip_prefix('-') {
        let magnitude = crate::numeric_literals::parse_unsigned_integer(magnitude)
            .ok_or_else(|| FormatError::malformed(None, "invalid signed integer literal"))?;
        let minimum_magnitude = if width == 128 {
            1u128 << 127
        } else {
            1u128 << (width - 1)
        };
        if magnitude > minimum_magnitude {
            return Err(FormatError::malformed(
                None,
                "negative integer literal is out of range",
            ));
        }
        magnitude.wrapping_neg()
    } else {
        let magnitude = crate::numeric_literals::parse_unsigned_integer(literal)
            .ok_or_else(|| FormatError::malformed(None, "invalid unsigned integer literal"))?;
        let maximum = if width == 128 {
            u128::MAX
        } else {
            (1u128 << width) - 1
        };
        if magnitude > maximum {
            return Err(FormatError::malformed(
                None,
                "integer literal is out of range",
            ));
        }
        magnitude
    };
    integer_constant(raw, bits)
}

fn integer_constant(raw: u128, bits: u16) -> Result<BinaryConstantValue, FormatError> {
    let width = usize::from(bits);
    let byte_width = width.div_ceil(8);
    let mut bytes = Vec::with_capacity(byte_width);
    for index in 0..byte_width {
        bytes.push((raw >> (index * 8)) as u8);
    }
    if width % 8 != 0 {
        if let Some(last) = bytes.last_mut() {
            *last &= (1 << (width % 8)) - 1;
        }
    }
    Ok(BinaryConstantValue::Integer(bytes))
}

fn parse_linkage(tokens: &[Token], cursor: &mut usize) -> Result<BinaryLinkage, FormatError> {
    match take_atom(tokens, cursor)?.as_str() {
        "private" => Ok(BinaryLinkage::Private),
        "module" => Ok(BinaryLinkage::Module),
        "package" => Ok(BinaryLinkage::Package),
        "exported" => Ok(BinaryLinkage::Exported),
        _ => Err(malformed_at(
            tokens,
            cursor.saturating_sub(1),
            "invalid linkage",
        )),
    }
}

fn parse_global_address_space(value: &str) -> Result<BinaryAddressSpace, FormatError> {
    match value {
        "generic" => Ok(BinaryAddressSpace::Generic),
        "device" => Ok(BinaryAddressSpace::Device),
        "constant" => Ok(BinaryAddressSpace::Constant),
        "threadgroup" => Ok(BinaryAddressSpace::Threadgroup),
        "thread" => Ok(BinaryAddressSpace::Thread),
        "object_payload" => Ok(BinaryAddressSpace::ObjectPayload),
        "ray_payload" => Ok(BinaryAddressSpace::RayPayload),
        "hit_attribute" => Ok(BinaryAddressSpace::HitAttribute),
        "tile" => Ok(BinaryAddressSpace::Tile),
        "node_record" => Ok(BinaryAddressSpace::NodeRecord),
        _ => Err(FormatError::malformed(None, "invalid global address space")),
    }
}

#[derive(Clone, Debug)]
struct ParsedTextFunction {
    name: String,
    linkage: BinaryLinkage,
    function_type: FunctionType,
    signature: ParsedFunctionAttributes,
    body: Option<FunctionBody>,
}

#[derive(Clone, Debug)]
struct ParsedFunctionAttributes {
    modifiers: u64,
    structured: bool,
    kernel_fp: bool,
    returns_twice: bool,
    inlinable: bool,
    engine: Option<BinaryEngine>,
    semantics: Option<SemanticsMode>,
    target: BinaryTarget,
    fp_mode: Option<BinaryFpMode>,
    generic_strategy: Option<BinaryGenericStrategy>,
    coroutine: Option<(Type, Type)>,
    instance_types: Vec<Type>,
    variant: Option<BinaryVariantAttribute>,
    platform_export: Option<String>,
    asynchronous: bool,
    abi: Abi,
}

fn parse_text_function(source: &str) -> Result<ParsedTextFunction, FormatError> {
    let mut tokens = lex(source).map_err(|error| {
        FormatError::malformed(
            Some(error.span.start),
            format!("invalid function: {}", error.message),
        )
    })?;
    tokens.retain(|token| !matches!(token.kind, TokenKind::End));
    let mut cursor = 0;
    expect_atom(&tokens, &mut cursor, "func")?;
    let linkage = match take_atom(&tokens, &mut cursor)?.as_str() {
        "private" => BinaryLinkage::Private,
        "module" => BinaryLinkage::Module,
        "package" => BinaryLinkage::Package,
        "exported" => BinaryLinkage::Exported,
        _ => return Err(malformed_at(&tokens, cursor, "invalid function linkage")),
    };
    expect_punctuation(&tokens, &mut cursor, '@')?;
    let name = take_path(&tokens, &mut cursor)?;
    if tokens
        .get(cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('<')))
    {
        return Err(malformed_at(
            &tokens,
            cursor,
            "generic function declarations require generic signature assembly",
        ));
    }

    let parsed = parse_function_signature(source)
        .map_err(|error| FormatError::malformed(Some(error.span.start), error.message))?
        .ok_or_else(|| FormatError::malformed(None, "expected function signature"))?;
    let attribute_start = crate::signature::function_attribute_start(source)
        .map_err(|error| FormatError::malformed(Some(error.span.start), error.message))?
        .ok_or_else(|| FormatError::malformed(None, "expected function attributes"))?;

    let mut attributes = ParsedFunctionAttributes {
        modifiers: 0,
        structured: false,
        kernel_fp: false,
        returns_twice: false,
        inlinable: false,
        engine: None,
        semantics: None,
        target: BinaryTarget::Cpu,
        fp_mode: None,
        generic_strategy: None,
        coroutine: None,
        instance_types: Vec::new(),
        variant: None,
        platform_export: None,
        asynchronous: false,
        abi: Abi::Ridl,
    };
    let mut seen = std::collections::HashSet::new();
    let mut cursor = attribute_start;
    let mut has_body = false;
    loop {
        skip_newlines(&tokens, &mut cursor);
        let Some(token) = tokens.get(cursor) else {
            break;
        };
        if matches!(token.kind, TokenKind::Punctuation('{')) {
            has_body = true;
            break;
        }
        if matches!(token.kind, TokenKind::Newline) {
            cursor += 1;
            continue;
        }
        let raw_name = take_atom(&tokens, &mut cursor)?;
        let attr_name = if raw_name == "export" || raw_name == "kernel" {
            expect_punctuation(&tokens, &mut cursor, '.')?;
            format!("{raw_name}.{}", take_atom(&tokens, &mut cursor)?)
        } else {
            raw_name
        };
        if !seen.insert(attr_name.clone()) {
            return Err(FormatError::malformed(
                Some(token.span.start),
                format!("duplicate function attribute `{attr_name}`"),
            ));
        }
        if attr_name == "export.platform" {
            attributes.platform_export = Some(take_string(&tokens, &mut cursor)?);
            continue;
        }
        if matches!(
            attr_name.as_str(),
            "async"
                | "structured"
                | "kernel.fp"
                | "returns_twice"
                | "inlinable"
                | "cold"
                | "nounwind"
                | "noreturn"
                | "inline"
                | "noinline"
        ) {
            match attr_name.as_str() {
                "async" => attributes.asynchronous = true,
                "structured" => attributes.structured = true,
                "kernel.fp" => attributes.kernel_fp = true,
                "returns_twice" => attributes.returns_twice = true,
                "inlinable" => attributes.inlinable = true,
                "cold" => attributes.modifiers |= 1 << 41,
                "nounwind" => attributes.modifiers |= 1 << 42,
                "noreturn" => attributes.modifiers |= 1 << 43,
                "inline" => attributes.modifiers |= 1 << 44,
                "noinline" => attributes.modifiers |= 1 << 45,
                _ => unreachable!(),
            }
            continue;
        }

        let arguments = take_attribute_arguments(&tokens, &mut cursor)?;
        match attr_name.as_str() {
            "engine" => {
                expect_single_atom(&arguments, "engine")?;
                attributes.engine = Some(match atom(&arguments[0][0]) {
                    Some("native") => BinaryEngine::Native,
                    Some("runtime") => BinaryEngine::Runtime,
                    Some("any") => BinaryEngine::Any,
                    _ => return Err(malformed_at(&tokens, cursor, "invalid engine attribute")),
                });
            }
            "semantics" => {
                expect_single_atom(&arguments, "semantics")?;
                attributes.semantics = Some(match atom(&arguments[0][0]) {
                    Some("defined") => SemanticsMode::Defined,
                    Some("permissive") => SemanticsMode::Permissive,
                    _ => return Err(malformed_at(&tokens, cursor, "invalid semantics attribute")),
                });
            }
            "abi" => {
                expect_single_atom(&arguments, "ABI")?;
                attributes.abi = match atom(&arguments[0][0]) {
                    Some("ridl") => Abi::Ridl,
                    Some("c") => Abi::C,
                    _ => return Err(malformed_at(&tokens, cursor, "invalid ABI attribute")),
                };
            }
            "target" => {
                let targets = arguments
                    .iter()
                    .map(|argument| {
                        if argument.len() != 1 {
                            return Err(malformed_at(&tokens, cursor, "invalid target attribute"));
                        }
                        match atom(&argument[0]) {
                            Some("cpu") => Ok(BinaryTarget::Cpu),
                            Some("gpu") => Ok(BinaryTarget::Gpu),
                            _ => Err(malformed_at(&tokens, cursor, "invalid target attribute")),
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                attributes.target = match targets.as_slice() {
                    [BinaryTarget::Cpu] => BinaryTarget::Cpu,
                    [BinaryTarget::Gpu] => BinaryTarget::Gpu,
                    [BinaryTarget::Cpu, BinaryTarget::Gpu]
                    | [BinaryTarget::Gpu, BinaryTarget::Cpu] => BinaryTarget::Both,
                    _ => return Err(malformed_at(&tokens, cursor, "invalid target attribute")),
                };
            }
            "fp" => {
                expect_single_atom(&arguments, "floating-point mode")?;
                attributes.fp_mode = Some(match atom(&arguments[0][0]) {
                    Some("precise") => BinaryFpMode::Precise,
                    Some("fast") => BinaryFpMode::Fast,
                    Some("ieee_denormals") => BinaryFpMode::IeeeDenormals,
                    _ => return Err(malformed_at(&tokens, cursor, "invalid floating-point mode")),
                });
            }
            "strategy" => {
                expect_single_atom(&arguments, "generic strategy")?;
                attributes.generic_strategy = Some(match atom(&arguments[0][0]) {
                    Some("specialize") => BinaryGenericStrategy::Specialize,
                    Some("polymorphic") => BinaryGenericStrategy::Polymorphic,
                    Some("both") => BinaryGenericStrategy::Both,
                    _ => return Err(malformed_at(&tokens, cursor, "invalid generic strategy")),
                });
            }
            "variant" => {
                if arguments.is_empty() || arguments[0].len() != 1 {
                    return Err(malformed_at(
                        &tokens,
                        cursor,
                        "variant attribute requires an architecture",
                    ));
                }
                let architecture = match atom(&arguments[0][0]) {
                    Some("aarch64") => 1,
                    Some("x64") => 2,
                    Some("rv64") => 3,
                    Some("wasm") => 4,
                    _ => {
                        return Err(malformed_at(
                            &tokens,
                            cursor,
                            "invalid variant architecture",
                        ));
                    }
                };
                let features = arguments[1..]
                    .iter()
                    .map(|argument| {
                        if argument.len() != 1 {
                            return Err(malformed_at(&tokens, cursor, "invalid variant feature"));
                        }
                        atom(&argument[0])
                            .map(str::to_owned)
                            .ok_or_else(|| malformed_at(&tokens, cursor, "invalid variant feature"))
                    })
                    .collect::<Result<_, _>>()?;
                attributes.variant = Some(BinaryVariantAttribute {
                    architecture,
                    features,
                });
            }
            "instance" => {
                if arguments.is_empty() {
                    return Err(malformed_at(
                        &tokens,
                        cursor,
                        "instance attribute requires at least one type",
                    ));
                }
                attributes.instance_types = arguments
                    .iter()
                    .map(|argument| parse_type_argument(source, argument))
                    .collect::<Result<Vec<_>, _>>()?;
            }
            "coro" => {
                if arguments.len() != 2 {
                    return Err(malformed_at(
                        &tokens,
                        cursor,
                        "coro requires yield and resume types",
                    ));
                }
                attributes.coroutine = Some((
                    parse_named_type_argument(source, arguments[0], "yield")?,
                    parse_named_type_argument(source, arguments[1], "resume")?,
                ));
            }
            _ => {
                return Err(FormatError::malformed(
                    Some(token.span.start),
                    format!("unsupported function attribute `{attr_name}`"),
                ));
            }
        }
    }

    if attributes.target == BinaryTarget::Gpu {
        if attributes.engine.is_some() {
            return Err(FormatError::malformed(
                None,
                "GPU-only functions do not have an engine attribute",
            ));
        }
    } else if attributes.engine.is_none() {
        attributes.engine = Some(BinaryEngine::Native);
    }
    let function_type = FunctionType {
        parameters: parsed
            .parameters
            .into_iter()
            .map(|parameter| ParameterType {
                context: parameter.context,
                convention: parameter.convention,
                ty: parameter.ty,
            })
            .collect(),
        result: Box::new(parsed.result),
        failure: parsed.failure.map(Box::new),
        asynchronous: attributes.asynchronous,
        abi: attributes.abi,
        foreign: false,
    };
    let body = parse_function_body(source)
        .map_err(|error| FormatError::malformed(Some(error.span.start), error.message))?;
    if has_body != body.is_some() {
        return Err(FormatError::malformed(
            None,
            "function body boundary is malformed",
        ));
    }
    Ok(ParsedTextFunction {
        name,
        linkage,
        function_type,
        signature: attributes,
        body,
    })
}

fn lower_simple_function_body(
    function_index: u64,
    body: &FunctionBody,
    types: &BinaryTypeTable,
    signatures: &BinarySignatureTable,
) -> Result<BinaryFunctionBody, FormatError> {
    if body.blocks.len() != 1 {
        return Err(FormatError::malformed(
            None,
            "this TIR CODE assembler pass supports one-block functions",
        ));
    }
    let signature =
        signatures
            .records
            .get(usize::try_from(function_index).map_err(|_| {
                FormatError::malformed(None, "function index does not fit in memory")
            })?)
            .ok_or_else(|| FormatError::malformed(None, "function signature is missing"))?;
    let function_type_index = usize::try_from(signature.function_type)
        .map_err(|_| FormatError::malformed(None, "function type index overflows"))?;
    let Some(BinaryType::Function(function_type)) = types.entries.get(function_type_index) else {
        return Err(FormatError::malformed(
            None,
            "signature type is not a function type",
        ));
    };
    let source_block = &body.blocks[0];
    if source_block.parameters.len() != function_type.parameters.len() {
        return Err(FormatError::malformed(
            Some(source_block.span.start),
            "entry block parameters do not match the function signature",
        ));
    }
    let mut values = HashMap::with_capacity(source_block.parameters.len());
    let mut parameters = Vec::with_capacity(source_block.parameters.len());
    for (index, (parameter, signature_parameter)) in source_block
        .parameters
        .iter()
        .zip(&function_type.parameters)
        .enumerate()
    {
        let signature_type = binary_type_to_tir(
            &types.entries,
            &types.declarations,
            signature_parameter.ty,
            None,
            0,
        )?;
        if parameter.ty != signature_type
            || parameter.context != signature_parameter.context
            || convention_code(parameter.convention) != signature_parameter.convention
        {
            return Err(FormatError::malformed(
                Some(parameter.span.start),
                "entry block parameter does not match the function signature",
            ));
        }
        let value_index = u64::try_from(index)
            .map_err(|_| FormatError::malformed(None, "parameter index overflows"))?;
        if values.insert(parameter.name.clone(), value_index).is_some() {
            return Err(FormatError::malformed(
                Some(parameter.span.start),
                "duplicate entry block parameter name",
            ));
        }
        parameters.push(BinaryBlockParameter {
            convention: signature_parameter.convention,
            context: signature_parameter.context,
            ty: signature_parameter.ty,
        });
    }
    if source_block
        .header_tokens
        .iter()
        .any(|token| atom(token) == Some("loop"))
    {
        return Err(FormatError::malformed(
            Some(source_block.span.start),
            "loop block annotations require multi-block CODE assembly",
        ));
    }
    if source_block.instructions.len() != 1 {
        return Err(FormatError::malformed(
            Some(source_block.span.start),
            "this TIR CODE assembler pass supports a single return or unreachable terminator",
        ));
    }
    let instruction = &source_block.instructions[0];
    if !instruction.results.is_empty()
        || instruction.tokens.first().and_then(atom) != Some(instruction.opcode.as_str())
    {
        return Err(FormatError::malformed(
            Some(instruction.span.start),
            "malformed function terminator",
        ));
    }
    let mut cursor = 1usize;
    let mut value_operand = None;
    let (opcode, modifiers) = match instruction.opcode.as_str() {
        "return" => {
            let result_type_index = usize::try_from(function_type.result)
                .map_err(|_| FormatError::malformed(None, "result type index overflows"))?;
            let is_unit = matches!(types.entries.get(result_type_index), Some(BinaryType::Unit));
            if is_unit {
                if cursor != instruction.tokens.len() {
                    return Err(malformed_at(
                        &instruction.tokens,
                        cursor,
                        "unit function return does not take a value",
                    ));
                }
            } else {
                let value_name = take_text_value(&instruction.tokens, &mut cursor)?;
                value_operand = Some(*values.get(&value_name).ok_or_else(|| {
                    FormatError::malformed(
                        Some(instruction.span.start),
                        format!("return references unknown value `%{value_name}`"),
                    )
                })?);
            }
            (0x1e5, 0)
        }
        "unreachable" => {
            let modifiers = if cursor == instruction.tokens.len() {
                0
            } else {
                if !matches!(instruction.tokens[cursor].kind, TokenKind::Punctuation('['))
                    || !matches!(
                        instruction.tokens.get(cursor + 2).map(|token| &token.kind),
                        Some(TokenKind::Punctuation(']'))
                    )
                    || cursor + 3 != instruction.tokens.len()
                {
                    return Err(malformed_at(
                        &instruction.tokens,
                        cursor,
                        "unreachable accepts only the `[trap]` or `[ub]` modifier",
                    ));
                }
                match instruction.tokens.get(cursor + 1).and_then(atom) {
                    Some("trap") => 0,
                    Some("ub") => 1 << 48,
                    _ => {
                        return Err(malformed_at(
                            &instruction.tokens,
                            cursor + 1,
                            "unreachable accepts only `[trap]` or `[ub]`",
                        ));
                    }
                }
            };
            cursor = instruction.tokens.len();
            (0x1e8, modifiers)
        }
        _ => {
            return Err(FormatError::malformed(
                Some(instruction.span.start),
                format!(
                    "unsupported instruction `{}` in initial CODE assembler pass",
                    instruction.opcode
                ),
            ));
        }
    };
    if cursor != instruction.tokens.len() {
        return Err(malformed_at(
            &instruction.tokens,
            cursor,
            "unexpected operand after function terminator",
        ));
    }
    let current_value = u64::try_from(function_type.parameters.len())
        .map_err(|_| FormatError::malformed(None, "SSA value index overflows"))?;
    let value_delta = value_operand
        .map(|value| {
            current_value
                .checked_sub(value)
                .filter(|delta| *delta != 0)
                .ok_or_else(|| FormatError::malformed(None, "invalid return SSA value"))
        })
        .transpose()?;
    let mut instruction_bytes = Vec::new();
    write_code_index(opcode, &mut instruction_bytes)?;
    write_code_index(modifiers, &mut instruction_bytes)?;
    write_code_index(0, &mut instruction_bytes)?;
    if let Some(value_delta) = value_delta {
        write_code_index(value_delta, &mut instruction_bytes)?;
    }
    let operands = value_operand.map_or_else(Vec::new, |value| vec![BinaryOperand::Value(value)]);
    Ok(BinaryFunctionBody {
        function_index,
        body_byte_size: 0,
        block_count: 1,
        blocks: vec![BinaryBlock {
            cold: false,
            loop_merge: None,
            loop_continue: None,
            parameters,
            instruction_count: 1,
            instruction_bytes,
            instructions: Some(vec![BinaryInstruction {
                offset: 0,
                opcode,
                modifiers,
                result_count: 0,
                terminator: true,
                operands,
            }]),
        }],
        canonical_bytes: Vec::new(),
    })
}

fn take_text_value(tokens: &[Token], cursor: &mut usize) -> Result<String, FormatError> {
    if !tokens
        .get(*cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('%')))
    {
        return Err(malformed_at(tokens, *cursor, "expected SSA value"));
    }
    *cursor += 1;
    take_atom(tokens, cursor)
}

fn write_code_index(value: u64, output: &mut Vec<u8>) -> Result<(), FormatError> {
    crate::leb::write_uleb(value, output)
        .map_err(|error| FormatError::malformed(None, format!("CODE ULEB128: {error}")))
}

const fn convention_code(convention: Option<Convention>) -> u8 {
    match convention {
        Some(Convention::Owned) => 0,
        Some(Convention::Borrow) => 1,
        Some(Convention::BorrowMut) => 2,
        None => 3,
    }
}

fn take_attribute_arguments<'a>(
    tokens: &'a [Token],
    cursor: &mut usize,
) -> Result<Vec<&'a [Token]>, FormatError> {
    if !tokens
        .get(*cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('(')))
    {
        return Err(malformed_at(
            tokens,
            *cursor,
            "expected function attribute arguments",
        ));
    }
    let open = *cursor;
    let close = matching_tokens(tokens, open)?;
    let arguments = split_token_arguments(&tokens[open + 1..close])?;
    *cursor = close + 1;
    Ok(arguments)
}

fn matching_tokens(tokens: &[Token], open: usize) -> Result<usize, FormatError> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(index);
                    }
                }
                _ => {}
            }
        }
    }
    Err(malformed_at(
        tokens,
        open,
        "unterminated function attribute",
    ))
}

fn split_token_arguments(tokens: &[Token]) -> Result<Vec<&[Token]>, FormatError> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut arguments = Vec::new();
    let mut start = 0usize;
    let mut depth = [0i32; 4];
    for (index, token) in tokens.iter().enumerate() {
        if let TokenKind::Punctuation(character) = token.kind {
            match character {
                '(' => depth[0] += 1,
                ')' => depth[0] -= 1,
                '[' => depth[1] += 1,
                ']' => depth[1] -= 1,
                '<' => depth[2] += 1,
                '>' => depth[2] -= 1,
                '{' => depth[3] += 1,
                '}' => depth[3] -= 1,
                ',' if depth == [0; 4] => {
                    if start == index {
                        return Err(malformed_at(
                            tokens,
                            index,
                            "empty function attribute argument",
                        ));
                    }
                    arguments.push(&tokens[start..index]);
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    if start == tokens.len() {
        return Err(malformed_at(
            tokens,
            start.saturating_sub(1),
            "trailing attribute comma",
        ));
    }
    arguments.push(&tokens[start..]);
    Ok(arguments)
}

fn expect_single_atom<'a>(arguments: &[&'a [Token]], name: &str) -> Result<(), FormatError> {
    if arguments.len() == 1 && arguments[0].len() == 1 && atom(&arguments[0][0]).is_some() {
        Ok(())
    } else {
        Err(FormatError::malformed(
            arguments
                .first()
                .and_then(|argument| argument.first())
                .map(|token| token.span.start),
            format!("{name} attribute expects one identifier"),
        ))
    }
}

fn parse_type_argument(source: &str, tokens: &[Token]) -> Result<Type, FormatError> {
    let start = tokens.first().map_or(0, |token| token.span.start);
    let end = tokens.last().map_or(start, |token| token.span.end);
    let text = source
        .get(start..end)
        .ok_or_else(|| FormatError::malformed(Some(start), "invalid type span"))?;
    crate::parse_type(text).map_err(|error| {
        FormatError::malformed(
            Some(start + error.span.start),
            format!("invalid type: {}", error.message),
        )
    })
}

fn parse_named_type_argument(
    source: &str,
    tokens: &[Token],
    name: &str,
) -> Result<Type, FormatError> {
    if tokens.first().and_then(atom) != Some(name)
        || !tokens
            .get(1)
            .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(':')))
    {
        return Err(malformed_at(
            tokens,
            0,
            "coro requires `yield: type` and `resume: type`",
        ));
    }
    parse_type_argument(source, &tokens[2..])
}

fn malformed_at(tokens: &[Token], index: usize, message: &str) -> FormatError {
    FormatError::malformed(tokens.get(index).map(|token| token.span.start), message)
}

fn parse_explicit_struct(source: &str) -> Result<ParsedTextStruct, FormatError> {
    let tokens = lex(source).map_err(|error| {
        FormatError::malformed(
            Some(error.span.start),
            format!("invalid type: {}", error.message),
        )
    })?;
    let tokens = tokens
        .into_iter()
        .filter(|token| !matches!(&token.kind, TokenKind::Newline | TokenKind::End))
        .collect::<Vec<_>>();
    let mut cursor = 0;
    expect_atom(&tokens, &mut cursor, "type")?;
    expect_punctuation(&tokens, &mut cursor, '%')?;
    let mut name = take_atom(&tokens, &mut cursor)?;
    while tokens
        .get(cursor)
        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('.')))
    {
        cursor += 1;
        name.push('.');
        name.push_str(&take_atom(&tokens, &mut cursor)?);
    }
    expect_punctuation(&tokens, &mut cursor, '=')?;
    let nominal_kind = take_atom(&tokens, &mut cursor)?;
    let kind = match nominal_kind.as_str() {
        "struct" => BinaryNominalKind::Struct,
        "explicit" => BinaryNominalKind::Explicit,
        "enum" => BinaryNominalKind::Enum,
        _ => {
            return Err(FormatError::malformed(
                tokens.get(cursor - 1).map(|token| token.span.start),
                "expected struct, enum, or explicit type kind",
            ));
        }
    };
    let layout_kind = take_atom(&tokens, &mut cursor)?;
    let layout = match layout_kind.as_str() {
        "ridl" => BinaryLayout::Ridl,
        "c" => BinaryLayout::C,
        "packed" => {
            expect_punctuation(&tokens, &mut cursor, '(')?;
            let alignment = take_number(&tokens, &mut cursor)?;
            expect_punctuation(&tokens, &mut cursor, ')')?;
            BinaryLayout::Packed(alignment)
        }
        "explicit" => {
            expect_punctuation(&tokens, &mut cursor, '(')?;
            expect_atom(&tokens, &mut cursor, "size")?;
            let size = take_number(&tokens, &mut cursor)?;
            expect_punctuation(&tokens, &mut cursor, ',')?;
            expect_atom(&tokens, &mut cursor, "align")?;
            let align = take_number(&tokens, &mut cursor)?;
            expect_punctuation(&tokens, &mut cursor, ')')?;
            BinaryLayout::Explicit { size, align }
        }
        _ => {
            return Err(FormatError::malformed(
                tokens.get(cursor - 1).map(|token| token.span.start),
                "expected ridl, c, packed, or explicit layout rule",
            ));
        }
    };
    expect_punctuation(&tokens, &mut cursor, '{')?;
    let mut fields = Vec::new();
    let mut variants = Vec::new();
    if kind == BinaryNominalKind::Enum {
        while !tokens
            .get(cursor)
            .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('}')))
        {
            let variant_name = take_atom(&tokens, &mut cursor)?;
            let mut payload = Vec::new();
            if tokens
                .get(cursor)
                .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('(')))
            {
                cursor += 1;
                while !tokens
                    .get(cursor)
                    .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(')')))
                {
                    payload.push(take_type(source, &tokens, &mut cursor)?);
                    if tokens
                        .get(cursor)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(',')))
                    {
                        cursor += 1;
                    } else if !tokens
                        .get(cursor)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(')')))
                    {
                        return Err(FormatError::malformed(
                            tokens.get(cursor).map(|token| token.span.start),
                            "expected `,` or `)` in enum payload",
                        ));
                    }
                }
                expect_punctuation(&tokens, &mut cursor, ')')?;
            }
            variants.push(ParsedTextVariant {
                name: variant_name,
                payload,
            });
            if tokens
                .get(cursor)
                .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(',')))
            {
                cursor += 1;
            }
        }
    } else {
        while !tokens
            .get(cursor)
            .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('}')))
        {
            let field_name = take_atom(&tokens, &mut cursor)?;
            expect_punctuation(&tokens, &mut cursor, ':')?;
            let type_offset = tokens.get(cursor).map(|token| token.span.start);
            let ty = take_type(source, &tokens, &mut cursor)?;
            let mut offset = None;
            let mut bit_offset = None;
            let mut bit_width = None;
            let mut annotations = Vec::new();
            while tokens
                .get(cursor)
                .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('@')))
            {
                cursor += 1;
                let next_atom = tokens.get(cursor).and_then(atom);
                if next_atom.and_then(parse_unsigned_number).is_some() {
                    if offset.is_some() {
                        return Err(FormatError::malformed(
                            tokens.get(cursor).map(|token| token.span.start),
                            "field has more than one explicit offset",
                        ));
                    }
                    offset = Some(take_number(&tokens, &mut cursor)?);
                    if tokens
                        .get(cursor)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(':')))
                    {
                        cursor += 1;
                        bit_offset = Some(take_number(&tokens, &mut cursor)?);
                        expect_punctuation(&tokens, &mut cursor, ':')?;
                        bit_width = Some(take_number(&tokens, &mut cursor)?);
                        if !matches!(&ty, Type::Integer(_)) {
                            return Err(FormatError::malformed(
                                type_offset,
                                "explicit bit-fields require an integer field type",
                            ));
                        }
                    }
                } else {
                    let annotation_name = take_atom(&tokens, &mut cursor)?;
                    if tokens
                        .get(cursor)
                        .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation('(')))
                    {
                        return Err(FormatError::malformed(
                            tokens.get(cursor).map(|token| token.span.start),
                            "field annotation arguments are not supported by this assembler pass",
                        ));
                    }
                    annotations.push(BinaryAnnotation {
                        name: annotation_name,
                        arguments: Vec::new(),
                    });
                }
            }
            fields.push(ParsedTextField {
                name: field_name,
                ty,
                offset,
                bit_offset,
                bit_width,
                annotations,
            });
            if tokens
                .get(cursor)
                .is_some_and(|token| matches!(token.kind, TokenKind::Punctuation(',')))
            {
                cursor += 1;
            }
        }
    }
    expect_punctuation(&tokens, &mut cursor, '}')?;
    let policy = parse_policy(&tokens, &mut cursor)?;
    if cursor != tokens.len()
        || (fields.is_empty() && variants.is_empty())
        || matches!(layout, BinaryLayout::Explicit { size: 0, .. })
        || matches!(layout, BinaryLayout::Explicit { align: 0, .. })
    {
        return Err(FormatError::malformed(
            tokens.get(cursor).map(|token| token.span.start),
            "this TIR assembler pass requires one or more fields and a valid nonempty layout",
        ));
    }

    Ok(ParsedTextStruct {
        declaration: BinaryNominalDeclaration {
            name,
            generic_parameters: Vec::new(),
            kind,
        },
        layout,
        fields,
        variants,
        policy,
    })
}

fn parse_policy(tokens: &[Token], cursor: &mut usize) -> Result<BinaryPolicy, FormatError> {
    expect_atom(tokens, cursor, "policy")?;
    expect_punctuation(tokens, cursor, '{')?;
    expect_atom(tokens, cursor, "copy")?;
    expect_punctuation(tokens, cursor, ':')?;
    let copy = match take_atom(tokens, cursor)?.as_str() {
        "trivial" => BinaryCopyPolicy::Trivial,
        "none" => BinaryCopyPolicy::None,
        "implicit" | "explicit" => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "copy policy functions require function declarations",
            ));
        }
        _ => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "expected trivial, none, implicit(@function), or explicit(@function) copy policy",
            ));
        }
    };
    expect_punctuation(tokens, cursor, ',')?;
    expect_atom(tokens, cursor, "destroy")?;
    expect_punctuation(tokens, cursor, ':')?;
    let destroy = match take_atom(tokens, cursor)?.as_str() {
        "trivial" => BinaryDestroyPolicy::Trivial,
        "fn" => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "destroy policy functions require function declarations",
            ));
        }
        _ => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "expected trivial or fn(@function) destroy policy",
            ));
        }
    };
    expect_punctuation(tokens, cursor, ',')?;
    expect_atom(tokens, cursor, "relocate")?;
    expect_punctuation(tokens, cursor, ':')?;
    let relocate = match take_atom(tokens, cursor)?.as_str() {
        "bitwise" => BinaryRelocatePolicy::Bitwise,
        "fn" => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "relocate policy functions require function declarations",
            ));
        }
        _ => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "expected bitwise or fn(@function) relocate policy",
            ));
        }
    };
    expect_punctuation(tokens, cursor, ',')?;
    expect_atom(tokens, cursor, "exclusivity")?;
    expect_punctuation(tokens, cursor, ':')?;
    let exclusivity = match take_atom(tokens, cursor)?.as_str() {
        "static" => 0,
        "dynamic" => 1,
        "none" => 2,
        _ => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "expected static, dynamic, or none exclusivity",
            ));
        }
    };
    expect_punctuation(tokens, cursor, ',')?;
    expect_atom(tokens, cursor, "pairing")?;
    expect_punctuation(tokens, cursor, ':')?;
    let pairing = match take_atom(tokens, cursor)?.as_str() {
        "counted" => 0,
        "none" => 1,
        _ => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "expected counted or none pairing",
            ));
        }
    };
    expect_punctuation(tokens, cursor, ',')?;
    expect_atom(tokens, cursor, "lifetime")?;
    expect_punctuation(tokens, cursor, ':')?;
    let lifetime = match take_atom(tokens, cursor)?.as_str() {
        "eager" => 0,
        "scoped" => 1,
        _ => {
            return Err(FormatError::malformed(
                tokens
                    .get(cursor.saturating_sub(1))
                    .map(|token| token.span.start),
                "expected eager or scoped lifetime",
            ));
        }
    };
    expect_punctuation(tokens, cursor, '}')?;
    Ok(BinaryPolicy {
        copy,
        destroy,
        relocate,
        exclusivity,
        pairing,
        lifetime,
    })
}

fn intern_type(
    entries: &mut Vec<BinaryType>,
    ty: &Type,
    declarations: &HashMap<String, u64>,
) -> Result<u64, FormatError> {
    intern_type_inner(entries, ty, declarations, 0)
}

fn intern_type_inner(
    entries: &mut Vec<BinaryType>,
    ty: &Type,
    declarations: &HashMap<String, u64>,
    depth: usize,
) -> Result<u64, FormatError> {
    if depth >= 256 {
        return Err(FormatError::malformed(None, "type nesting exceeds 255"));
    }
    let child = |entries: &mut Vec<BinaryType>, ty: &Type| {
        intern_type_inner(entries, ty, declarations, depth + 1)
    };
    let binary_type = match ty {
        Type::Integer(bits) => BinaryType::Integer(u64::from(*bits)),
        Type::Float(float) => BinaryType::Float(*float),
        Type::Pointer(space) => BinaryType::Pointer(address_space_code(*space)),
        Type::Unit => BinaryType::Unit,
        Type::Never => BinaryType::Never,
        Type::FixedVector { lanes, element } => BinaryType::FixedVector {
            lanes: u64::from(*lanes),
            element: child(entries, element)?,
        },
        Type::ScalableVector(element) => BinaryType::ScalableVector {
            element: child(entries, element)?,
        },
        Type::Tuple(elements) => BinaryType::Tuple(
            elements
                .iter()
                .map(|element| child(entries, element))
                .collect::<Result<_, _>>()?,
        ),
        Type::Array { count, element } => BinaryType::Array {
            count: *count,
            element: child(entries, element)?,
        },
        Type::Nominal { path, arguments } => {
            let declaration = declarations.get(path).copied().ok_or_else(|| {
                FormatError::malformed(None, format!("unknown nominal type `{path}`"))
            })?;
            BinaryType::Nominal {
                declaration,
                arguments: arguments
                    .iter()
                    .map(|argument| child(entries, argument))
                    .collect::<Result<_, _>>()?,
            }
        }
        Type::Generic(name) => {
            return Err(FormatError::malformed(
                None,
                format!("generic field type `${name}` requires generic declarations"),
            ));
        }
        Type::Reference(pointee) => BinaryType::Reference {
            pointee: child(entries, pointee)?,
        },
        Type::Unique(pointee) => BinaryType::Unique {
            pointee: child(entries, pointee)?,
        },
        Type::Weak(pointee) => BinaryType::Weak {
            pointee: child(entries, pointee)?,
        },
        Type::Address { pointee, space } => BinaryType::Address {
            pointee: child(entries, pointee)?,
            space: address_space_code(space.unwrap_or(AddressSpace::Generic)),
        },
        Type::Function(signature) => BinaryType::Function(intern_function_type(
            entries,
            signature,
            declarations,
            depth,
        )?),
        Type::Closure(closure) => {
            let function_type = child(entries, &Type::Function(closure.signature.clone()))?;
            BinaryType::Closure {
                function_type,
                borrowed: closure.borrowed,
            }
        }
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => BinaryType::Coroutine {
            yield_type: child(entries, yield_type)?,
            resume_type: child(entries, resume_type)?,
            result_type: child(entries, result_type)?,
        },
        Type::Frame(name) => {
            return Err(FormatError::malformed(
                None,
                format!("frame type `{name}` requires function index resolution"),
            ));
        }
        Type::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => BinaryType::Texture {
            dimension: texture_dimension_code(*dimension),
            component: child(entries, component)?,
            access: texture_access_code(*access),
            multisampled: *multisampled,
        },
        Type::Sampler => BinaryType::Sampler,
        Type::AccelerationStructure => BinaryType::AccelerationStructure,
        Type::VisibleFunctionTable(signature) => {
            let function_type = child(entries, &Type::Function((**signature).clone()))?;
            BinaryType::VisibleFunctionTable { function_type }
        }
        Type::Tensor { count, element } => BinaryType::Tensor {
            rank: u64::from(*count),
            element: child(entries, element)?,
        },
        Type::RayQuery => BinaryType::RayQuery,
        Type::HitObject => BinaryType::HitObject,
        Type::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        } => BinaryType::CooperativeMatrix {
            scope: matrix_scope_code(*scope),
            rows: u64::from(*rows),
            columns: u64::from(*columns),
            element: child(entries, element)?,
            use_kind: matrix_use_code(*use_kind),
        },
        Type::Entry {
            kind,
            root,
            immediate,
        } => BinaryType::Entry {
            kind: entry_kind_code(*kind),
            root: child(entries, root)?,
            immediate: child(entries, immediate)?,
        },
        Type::Code => {
            return Err(FormatError::malformed(
                None,
                "the TVISA `code` type is not a TIR field type",
            ));
        }
    };
    intern_binary_type(entries, binary_type)
}

fn intern_function_type(
    entries: &mut Vec<BinaryType>,
    signature: &FunctionType,
    declarations: &HashMap<String, u64>,
    depth: usize,
) -> Result<BinaryFunctionType, FormatError> {
    let mut parameters = Vec::with_capacity(signature.parameters.len());
    for parameter in &signature.parameters {
        let convention = match parameter.convention {
            Some(Convention::Owned) => 0,
            Some(Convention::Borrow) => 1,
            Some(Convention::BorrowMut) => 2,
            None => 3,
        };
        parameters.push(BinaryFunctionParameter {
            convention,
            context: parameter.context,
            ty: intern_type_inner(entries, &parameter.ty, declarations, depth + 1)?,
        });
    }
    let result = intern_type_inner(entries, &signature.result, declarations, depth + 1)?;
    let error = signature
        .failure
        .as_ref()
        .map(|failure| intern_type_inner(entries, failure, declarations, depth + 1))
        .transpose()?;
    Ok(BinaryFunctionType {
        parameters,
        result,
        error,
        asynchronous: signature.asynchronous,
        c_abi: signature.abi == Abi::C,
        foreign: signature.foreign,
    })
}

fn intern_binary_type(entries: &mut Vec<BinaryType>, ty: BinaryType) -> Result<u64, FormatError> {
    if let Some(index) = entries.iter().position(|entry| entry == &ty) {
        return u64::try_from(index)
            .map_err(|_| FormatError::malformed(None, "type table index overflows"));
    }
    let index = u64::try_from(entries.len())
        .map_err(|_| FormatError::malformed(None, "type table index overflows"))?;
    entries.push(ty);
    Ok(index)
}

const fn address_space_code(space: AddressSpace) -> u8 {
    match space {
        AddressSpace::Generic => 0,
        AddressSpace::Device => 1,
        AddressSpace::Constant => 2,
        AddressSpace::Threadgroup => 3,
        AddressSpace::Thread => 4,
        AddressSpace::ObjectPayload => 5,
        AddressSpace::RayPayload => 6,
        AddressSpace::HitAttribute => 7,
        AddressSpace::Tile => 8,
        AddressSpace::NodeRecord => 9,
    }
}

const fn texture_dimension_code(dimension: TextureDimension) -> u8 {
    match dimension {
        TextureDimension::D1 => 0,
        TextureDimension::D2 => 1,
        TextureDimension::D3 => 2,
        TextureDimension::Cube => 3,
        TextureDimension::D1Array => 4,
        TextureDimension::D2Array => 5,
        TextureDimension::CubeArray => 6,
    }
}

const fn texture_access_code(access: TextureAccess) -> u8 {
    match access {
        TextureAccess::Sample => 0,
        TextureAccess::Read => 1,
        TextureAccess::Write => 2,
        TextureAccess::ReadWrite => 3,
    }
}

const fn matrix_scope_code(scope: MatrixScope) -> u8 {
    match scope {
        MatrixScope::Subgroup => 0,
        MatrixScope::Threadgroup => 1,
    }
}

const fn matrix_use_code(use_kind: CooperativeMatrixUse) -> u8 {
    match use_kind {
        CooperativeMatrixUse::A => 0,
        CooperativeMatrixUse::B => 1,
        CooperativeMatrixUse::Acc => 2,
    }
}

const fn entry_kind_code(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Compute => 0,
        EntryKind::Object => 1,
        EntryKind::Mesh => 2,
        EntryKind::Vertex => 3,
        EntryKind::Fragment => 4,
        EntryKind::Tile => 5,
        EntryKind::Raygen => 6,
        EntryKind::Intersection => 7,
        EntryKind::Anyhit => 8,
        EntryKind::Closesthit => 9,
        EntryKind::Miss => 10,
        EntryKind::Callable => 11,
        EntryKind::Node => 12,
    }
}

fn expect_atom(tokens: &[Token], cursor: &mut usize, expected: &str) -> Result<(), FormatError> {
    let actual = tokens.get(*cursor).and_then(atom);
    if actual != Some(expected) {
        return Err(FormatError::malformed(
            tokens.get(*cursor).map(|token| token.span.start),
            format!("expected `{expected}` in TIR record"),
        ));
    }
    *cursor += 1;
    Ok(())
}

fn expect_punctuation(
    tokens: &[Token],
    cursor: &mut usize,
    expected: char,
) -> Result<(), FormatError> {
    if !tokens.get(*cursor).is_some_and(
        |token| matches!(token.kind, TokenKind::Punctuation(character) if character == expected),
    ) {
        return Err(FormatError::malformed(
            tokens.get(*cursor).map(|token| token.span.start),
            format!("expected `{expected}` in TIR record"),
        ));
    }
    *cursor += 1;
    Ok(())
}

fn take_atom(tokens: &[Token], cursor: &mut usize) -> Result<String, FormatError> {
    let Some(Token {
        kind: TokenKind::Atom(value),
        ..
    }) = tokens.get(*cursor)
    else {
        return Err(FormatError::malformed(
            tokens.get(*cursor).map(|token| token.span.start),
            "expected an identifier",
        ));
    };
    *cursor += 1;
    Ok(value.clone())
}

fn take_number(tokens: &[Token], cursor: &mut usize) -> Result<u64, FormatError> {
    let value = take_atom(tokens, cursor)?;
    parse_unsigned_number(&value).ok_or_else(|| {
        FormatError::malformed(
            tokens
                .get(cursor.saturating_sub(1))
                .map(|token| token.span.start),
            "expected a nonnegative integer",
        )
    })
}

fn parse_unsigned_number(value: &str) -> Option<u64> {
    u64::try_from(crate::numeric_literals::parse_unsigned_integer(value)?).ok()
}

fn take_type(source: &str, tokens: &[Token], cursor: &mut usize) -> Result<Type, FormatError> {
    let start = *cursor;
    let mut end = start;
    let mut nesting = Vec::new();
    while let Some(token) = tokens.get(end) {
        match &token.kind {
            TokenKind::Punctuation(character)
                if nesting.is_empty() && matches!(*character, '@' | ',' | '}' | ')') =>
            {
                break;
            }
            TokenKind::Newline if nesting.is_empty() => break,
            TokenKind::Punctuation(character) if matches!(*character, '<' | '(' | '[') => {
                nesting.push(*character);
            }
            TokenKind::Punctuation(character) if matches!(*character, '>' | ')' | ']') => {
                let expected = match *character {
                    '>' => '<',
                    ')' => '(',
                    ']' => '[',
                    _ => {
                        return Err(FormatError::malformed(
                            Some(token.span.start),
                            "invalid delimiter in field type",
                        ));
                    }
                };
                if nesting.last() == Some(&expected) {
                    nesting.pop();
                }
            }
            _ => {}
        }
        end += 1;
    }
    if start == end {
        return Err(FormatError::malformed(
            tokens.get(start).map(|token| token.span.start),
            "missing field type",
        ));
    }
    let start_offset = tokens[start].span.start;
    let end_offset = tokens[end - 1].span.end;
    let type_source = source
        .get(start_offset..end_offset)
        .ok_or_else(|| FormatError::malformed(Some(start_offset), "field type span is invalid"))?;
    *cursor = end;
    crate::parse_type(type_source).map_err(|error| {
        let offset = start_offset.checked_add(error.span.start);
        FormatError::malformed(offset, format!("invalid field type: {}", error.message))
    })
}

fn atom(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Atom(value) => Some(value),
        _ => None,
    }
}

fn render_explicit_structs(
    table: &BinaryTypeTable,
    preceding_roots: &[u64],
    trailing_roots: &[u64],
) -> Result<Option<Vec<String>>, FormatError> {
    if table.declarations.is_empty() && table.entries.is_empty() && table.definitions.is_empty() {
        return Ok(None);
    }
    if table.declarations.is_empty() && table.definitions.is_empty() {
        let mut roots = preceding_roots.to_vec();
        roots.extend_from_slice(trailing_roots);
        verify_canonical_type_order(table, &roots)?;
        return Ok(None);
    }
    if table.declarations.is_empty() || table.declarations.len() != table.definitions.len() {
        return Err(FormatError::malformed(
            None,
            "this TIR disassembler pass supports explicit struct type definitions only",
        ));
    }
    let mut type_roots = preceding_roots.to_vec();
    let mut items = Vec::with_capacity(table.declarations.len());
    for (index, (declaration, definition)) in table
        .declarations
        .iter()
        .zip(&table.definitions)
        .enumerate()
    {
        let declaration_index = u64::try_from(index)
            .map_err(|_| FormatError::malformed(None, "nominal declaration index overflows"))?;
        let is_enum = declaration.kind == BinaryNominalKind::Enum;
        if !declaration.generic_parameters.is_empty()
            || definition.declaration != declaration_index
            || (is_enum && !definition.fields.is_empty())
            || (!is_enum && (definition.fields.is_empty() || !definition.variants.is_empty()))
        {
            return Err(FormatError::malformed(
                None,
                "this TIR disassembler pass supports struct, enum, and explicit-union definitions without generic parameters only",
            ));
        }
        let nominal_kind = match declaration.kind {
            BinaryNominalKind::Struct => "struct",
            BinaryNominalKind::Enum => "enum",
            BinaryNominalKind::Explicit => "explicit",
        };
        let layout_text = render_layout(definition.layout)?;
        let mut rendered_members = Vec::new();
        if is_enum {
            for variant in &definition.variants {
                let mut payload = Vec::with_capacity(variant.payload.len());
                for type_index in &variant.payload {
                    type_roots.push(*type_index);
                    let ty = binary_type_to_tir(
                        &table.entries,
                        &table.declarations,
                        *type_index,
                        Some(index),
                        0,
                    )?;
                    payload.push(ty.to_string());
                }
                if payload.is_empty() {
                    rendered_members.push(format!("  {}", variant.name));
                } else {
                    rendered_members.push(format!("  {}({})", variant.name, payload.join(", ")));
                }
            }
        } else {
            for field in &definition.fields {
                let field_type_index = usize::try_from(field.ty).map_err(|_| {
                    FormatError::malformed(None, "field type index does not fit in memory")
                })?;
                if field_type_index >= table.entries.len() {
                    return Err(FormatError::malformed(
                        None,
                        "field type index is out of range",
                    ));
                }
                let location = match (field.offset, field.bit_offset, field.bit_width) {
                    (None, None, None) => String::new(),
                    (Some(offset), None, None) => format!(" @{offset}"),
                    (Some(offset), Some(bit_offset), Some(bit_width))
                        if bit_offset <= u64::from(u16::MAX)
                            && bit_width <= u64::from(u16::MAX)
                            && matches!(
                                &table.entries[field_type_index],
                                BinaryType::Integer(_)
                            ) =>
                    {
                        format!(" @{offset}:{bit_offset}:{bit_width}")
                    }
                    _ => {
                        return Err(FormatError::malformed(
                            None,
                            "explicit bit-fields require a 16-bit offset and width on an integer field",
                        ));
                    }
                };
                type_roots.push(field.ty);
                let ty = binary_type_to_tir(
                    &table.entries,
                    &table.declarations,
                    field.ty,
                    Some(index),
                    0,
                )?;
                let mut annotations = String::new();
                for annotation in &field.annotations {
                    if !annotation.arguments.is_empty() {
                        return Err(FormatError::malformed(
                            None,
                            "field annotation arguments cannot be represented by this TIR renderer",
                        ));
                    }
                    write!(&mut annotations, " @{}", annotation.name)
                        .map_err(|error| FormatError::malformed(None, error.to_string()))?;
                }
                rendered_members.push(format!("  {}: {}{}{annotations}", field.name, ty, location));
            }
        }
        let policy = render_policy(&definition.policy)?;
        items.push(format!(
            "type %{} = {} {layout_text} {{\n{}\n}}\n{policy}",
            declaration.name,
            nominal_kind,
            rendered_members.join(",\n")
        ));
    }
    type_roots.extend_from_slice(trailing_roots);
    verify_canonical_type_order(table, &type_roots)?;
    Ok(Some(items))
}

fn is_supported_declaration_sections(kinds: &[u32]) -> bool {
    if kinds.len() < 4 || kinds[0] != 1 || kinds[1] != 2 {
        return false;
    }
    let optional_end = kinds.len() - 2;
    if kinds[optional_end..] != [13, 14] {
        return false;
    }
    let optional = &kinds[2..optional_end];
    optional.iter().enumerate().all(|(index, kind)| {
        [3, 4, 6, 7, 8].contains(kind) && (index == 0 || optional[index - 1] < *kind)
    }) && (!optional.contains(&8) || optional.contains(&7))
}

fn render_global(global: &BinaryGlobal, table: &BinaryTypeTable) -> Result<String, FormatError> {
    if global.initializer.is_some() {
        return Err(FormatError::malformed(
            None,
            "global initializer requires CONSTANTS text disassembly",
        ));
    }
    let ty = binary_type_to_tir(&table.entries, &table.declarations, global.ty, None, 0)?;
    let linkage = match global.linkage {
        BinaryLinkage::Private => "private",
        BinaryLinkage::Module => "module",
        BinaryLinkage::Package => "package",
        BinaryLinkage::Exported => "exported",
    };
    let mut output = format!("global {linkage} @{}: {ty}", global.name);
    let address_space = match global.address_space {
        BinaryAddressSpace::Generic => None,
        BinaryAddressSpace::Device => Some("device"),
        BinaryAddressSpace::Constant => Some("constant"),
        BinaryAddressSpace::Threadgroup => Some("threadgroup"),
        BinaryAddressSpace::Thread => Some("thread"),
        BinaryAddressSpace::ObjectPayload => Some("object_payload"),
        BinaryAddressSpace::RayPayload => Some("ray_payload"),
        BinaryAddressSpace::HitAttribute => Some("hit_attribute"),
        BinaryAddressSpace::Tile => Some("tile"),
        BinaryAddressSpace::NodeRecord => Some("node_record"),
    };
    if let Some(address_space) = address_space {
        write!(&mut output, " <{address_space}>")
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if global.thread_local {
        output.push_str(" thread_local");
    }
    if global.constant {
        output.push_str(" constant");
    }
    if global.per_cpu {
        output.push_str(" percpu");
    }
    if let Some(alignment) = global.alignment {
        write!(&mut output, " align({alignment})")
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    Ok(output)
}

fn render_signature(
    signature: &BinarySignature,
    table: &BinaryTypeTable,
    body: Option<&BinaryFunctionBody>,
) -> Result<String, FormatError> {
    if !signature.generic_parameters.is_empty() {
        return Err(FormatError::malformed(
            None,
            "generic function signatures require generic text disassembly",
        ));
    }
    let function_ty = binary_type_to_tir(
        &table.entries,
        &table.declarations,
        signature.function_type,
        None,
        0,
    )?;
    let Type::Function(function_type) = function_ty else {
        return Err(FormatError::malformed(
            None,
            "signature type is not a function type",
        ));
    };
    let linkage = match signature.linkage {
        BinaryLinkage::Private => "private",
        BinaryLinkage::Module => "module",
        BinaryLinkage::Package => "package",
        BinaryLinkage::Exported => "exported",
    };
    let mut parameters = Vec::with_capacity(function_type.parameters.len());
    for (index, parameter) in function_type.parameters.iter().enumerate() {
        let context = if parameter.context { "context " } else { "" };
        let convention = match parameter.convention {
            Some(Convention::Owned) => "owned ",
            Some(Convention::Borrow) => "borrow ",
            Some(Convention::BorrowMut) => "borrowmut ",
            None => "",
        };
        parameters.push(format!(
            "%arg{index}: {context}{convention}{}",
            parameter.ty
        ));
    }
    let mut output = format!(
        "func {linkage} @{}({}) -> {}",
        signature.name,
        parameters.join(", "),
        function_type.result
    );
    if let Some(failure) = &function_type.failure {
        write!(&mut output, " fails {failure}")
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if function_type.abi == Abi::C {
        output.push_str(" abi(c)");
    }
    if function_type.asynchronous {
        output.push_str(" async");
    }
    if let Some(engine) = signature.engine {
        let engine = match engine {
            BinaryEngine::Native => "native",
            BinaryEngine::Runtime => "runtime",
            BinaryEngine::Any => "any",
        };
        write!(&mut output, " engine({engine})")
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if let Some(semantics) = signature.semantics {
        output.push_str(match semantics {
            SemanticsMode::Defined => " semantics(defined)",
            SemanticsMode::Permissive => " semantics(permissive)",
        });
    }
    match signature.target {
        BinaryTarget::Cpu => {}
        BinaryTarget::Gpu => output.push_str(" target(gpu)"),
        BinaryTarget::Both => output.push_str(" target(cpu, gpu)"),
    }
    if let Some(mode) = signature.fp_mode {
        let mode = match mode {
            BinaryFpMode::Precise => "precise",
            BinaryFpMode::Fast => "fast",
            BinaryFpMode::IeeeDenormals => "ieee_denormals",
        };
        write!(&mut output, " fp({mode})")
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if let Some((yield_type, resume_type)) = signature.coroutine {
        let yield_type =
            binary_type_to_tir(&table.entries, &table.declarations, yield_type, None, 0)?;
        let resume_type =
            binary_type_to_tir(&table.entries, &table.declarations, resume_type, None, 0)?;
        write!(
            &mut output,
            " coro(yield: {yield_type}, resume: {resume_type})"
        )
        .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if let Some(variant) = &signature.variant {
        let architecture = match variant.architecture {
            1 => "aarch64",
            2 => "x64",
            3 => "rv64",
            4 => "wasm",
            _ => return Err(FormatError::malformed(None, "invalid variant architecture")),
        };
        let mut members = vec![architecture.to_owned()];
        members.extend(variant.features.iter().cloned());
        write!(&mut output, " variant({})", members.join(", "))
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if signature.structured {
        output.push_str(" structured");
    }
    if signature.kernel_fp {
        output.push_str(" kernel.fp");
    }
    if signature.returns_twice {
        output.push_str(" returns_twice");
    }
    if signature.inlinable {
        output.push_str(" inlinable");
    }
    for (bit, name) in [
        (41, "cold"),
        (42, "nounwind"),
        (43, "noreturn"),
        (44, "inline"),
        (45, "noinline"),
    ] {
        if signature.modifiers & (1 << bit) != 0 {
            write!(&mut output, " {name}")
                .map_err(|error| FormatError::malformed(None, error.to_string()))?;
        }
    }
    if let Some(strategy) = signature.generic_strategy {
        let strategy = match strategy {
            BinaryGenericStrategy::Specialize => "specialize",
            BinaryGenericStrategy::Polymorphic => "polymorphic",
            BinaryGenericStrategy::Both => "both",
        };
        write!(&mut output, " strategy({strategy})")
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if !signature.instance_types.is_empty() {
        let instance_types = signature
            .instance_types
            .iter()
            .map(|type_index| {
                binary_type_to_tir(&table.entries, &table.declarations, *type_index, None, 0)
                    .map(|ty| ty.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        write!(&mut output, " instance({})", instance_types.join(", "))
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if let Some(platform_export) = &signature.platform_export {
        write!(&mut output, " export.platform {}", quote(platform_export)?)
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    if let Some(body) = body {
        render_simple_function_body(&mut output, body, &function_type, table)?;
    }
    Ok(output)
}

fn render_simple_function_body(
    output: &mut String,
    body: &BinaryFunctionBody,
    function_type: &FunctionType,
    table: &BinaryTypeTable,
) -> Result<(), FormatError> {
    if body.block_count != 1 || body.blocks.len() != 1 {
        return Err(FormatError::malformed(
            None,
            "this TIR CODE disassembler pass supports one-block functions",
        ));
    }
    let block = &body.blocks[0];
    if block.instruction_count != 1 || block.parameters.len() != function_type.parameters.len() {
        return Err(FormatError::malformed(
            None,
            "this TIR CODE disassembler pass supports one terminator and signature parameters only",
        ));
    }
    let instructions = block.instructions.as_ref().ok_or_else(|| {
        FormatError::malformed(None, "CODE block contains an unsupported instruction")
    })?;
    let instruction = instructions
        .first()
        .ok_or_else(|| FormatError::malformed(None, "CODE block has no instruction"))?;
    let mut parameters = Vec::with_capacity(block.parameters.len());
    for (index, (parameter, signature_parameter)) in block
        .parameters
        .iter()
        .zip(&function_type.parameters)
        .enumerate()
    {
        let ty = binary_type_to_tir(&table.entries, &table.declarations, parameter.ty, None, 0)?;
        if ty != signature_parameter.ty
            || parameter.context != signature_parameter.context
            || parameter.convention != convention_code(signature_parameter.convention)
        {
            return Err(FormatError::malformed(
                None,
                "CODE block parameter differs from signature",
            ));
        }
        let context = if parameter.context { "context " } else { "" };
        let convention = match parameter.convention {
            0 => "owned ",
            1 => "borrow ",
            2 => "borrowmut ",
            3 | 4 => "",
            _ => {
                return Err(FormatError::malformed(
                    None,
                    "invalid CODE block parameter convention",
                ));
            }
        };
        parameters.push(format!("%arg{index}: {context}{convention}{ty}"));
    }
    output.push_str(" {\nbb0");
    if !parameters.is_empty() {
        write!(output, "({})", parameters.join(", "))
            .map_err(|error| FormatError::malformed(None, error.to_string()))?;
    }
    output.push_str(":\n");
    match instruction.opcode {
        0x1e5 if instruction.modifiers == 0 && instruction.result_count == 0 => {
            if *function_type.result == Type::Unit {
                if !instruction.operands.is_empty() {
                    return Err(FormatError::malformed(None, "unit return has an operand"));
                }
                output.push_str("  return\n");
            } else if let [BinaryOperand::Value(value)] = instruction.operands.as_slice() {
                let value = usize::try_from(*value)
                    .map_err(|_| FormatError::malformed(None, "return value index overflows"))?;
                if value >= parameters.len() {
                    return Err(FormatError::malformed(
                        None,
                        "return references a value outside the supported entry parameters",
                    ));
                }
                writeln!(output, "  return %arg{value}")
                    .map_err(|error| FormatError::malformed(None, error.to_string()))?;
            } else {
                return Err(FormatError::malformed(
                    None,
                    "non-unit return has an invalid operand",
                ));
            }
        }
        0x1e8 if instruction.result_count == 0 && instruction.operands.is_empty() => {
            match instruction.modifiers {
                0 => output.push_str("  unreachable\n"),
                value if value == 1 << 48 => output.push_str("  unreachable [ub]\n"),
                _ => {
                    return Err(FormatError::malformed(
                        None,
                        "invalid unreachable modifiers",
                    ));
                }
            }
        }
        _ => {
            return Err(FormatError::malformed(
                None,
                "this TIR CODE disassembler pass supports return and unreachable only",
            ));
        }
    }
    output.push('}');
    Ok(())
}

fn render_layout(layout: BinaryLayout) -> Result<String, FormatError> {
    match layout {
        BinaryLayout::Ridl => Ok("ridl".to_owned()),
        BinaryLayout::C => Ok("c".to_owned()),
        BinaryLayout::Packed(alignment) => Ok(format!("packed({alignment})")),
        BinaryLayout::Explicit { size, align } => {
            if size == 0 || align == 0 {
                return Err(FormatError::malformed(
                    None,
                    "explicit type layout has a zero size or alignment",
                ));
            }
            Ok(format!("explicit(size {size}, align {align})"))
        }
    }
}

fn render_policy(policy: &BinaryPolicy) -> Result<String, FormatError> {
    let copy = match policy.copy {
        BinaryCopyPolicy::Trivial => "trivial",
        BinaryCopyPolicy::None => "none",
        BinaryCopyPolicy::Implicit(_) | BinaryCopyPolicy::Explicit(_) => {
            return Err(FormatError::malformed(
                None,
                "copy policy functions require SIGNATURES",
            ));
        }
    };
    let destroy = match policy.destroy {
        BinaryDestroyPolicy::Trivial => "trivial",
        BinaryDestroyPolicy::Function(_) => {
            return Err(FormatError::malformed(
                None,
                "destroy policy functions require SIGNATURES",
            ));
        }
    };
    let relocate = match policy.relocate {
        BinaryRelocatePolicy::Bitwise => "bitwise",
        BinaryRelocatePolicy::Function(_) => {
            return Err(FormatError::malformed(
                None,
                "relocate policy functions require SIGNATURES",
            ));
        }
    };
    let exclusivity = match policy.exclusivity {
        0 => "static",
        1 => "dynamic",
        2 => "none",
        _ => return Err(FormatError::malformed(None, "invalid exclusivity policy")),
    };
    let pairing = match policy.pairing {
        0 => "counted",
        1 => "none",
        _ => return Err(FormatError::malformed(None, "invalid pairing policy")),
    };
    let lifetime = match policy.lifetime {
        0 => "eager",
        1 => "scoped",
        _ => return Err(FormatError::malformed(None, "invalid lifetime policy")),
    };
    Ok(format!(
        "policy {{copy: {copy}, destroy: {destroy}, relocate: {relocate}, exclusivity: {exclusivity}, pairing: {pairing}, lifetime: {lifetime}}}"
    ))
}

fn verify_canonical_type_order(table: &BinaryTypeTable, roots: &[u64]) -> Result<(), FormatError> {
    let mut visited = vec![false; table.entries.len()];
    let mut ordered = Vec::with_capacity(table.entries.len());
    for root in roots {
        visit_type(*root, &table.entries, &mut visited, &mut ordered)?;
    }
    if ordered.len() != table.entries.len()
        || ordered
            .iter()
            .enumerate()
            .any(|(expected, actual)| expected != *actual)
    {
        return Err(FormatError::malformed(
            None,
            "TYPES entries are not in topological first-reference order",
        ));
    }
    Ok(())
}

fn visit_type(
    index: u64,
    entries: &[BinaryType],
    visited: &mut [bool],
    ordered: &mut Vec<usize>,
) -> Result<(), FormatError> {
    let index = usize::try_from(index)
        .map_err(|_| FormatError::malformed(None, "type index does not fit in memory"))?;
    let Some(ty) = entries.get(index) else {
        return Err(FormatError::malformed(None, "type index is out of range"));
    };
    if visited[index] {
        return Ok(());
    }
    for child in binary_type_dependencies(ty) {
        visit_type(child, entries, visited, ordered)?;
    }
    visited[index] = true;
    ordered.push(index);
    Ok(())
}

fn binary_type_dependencies(ty: &BinaryType) -> Vec<u64> {
    match ty {
        BinaryType::Address { pointee, .. }
        | BinaryType::ScalableVector { element: pointee }
        | BinaryType::Array {
            element: pointee, ..
        }
        | BinaryType::Reference { pointee }
        | BinaryType::Unique { pointee }
        | BinaryType::Weak { pointee } => vec![*pointee],
        BinaryType::FixedVector { element, .. }
        | BinaryType::Texture {
            component: element, ..
        }
        | BinaryType::Tensor { element, .. }
        | BinaryType::CooperativeMatrix { element, .. } => vec![*element],
        BinaryType::Tuple(elements) => elements.clone(),
        BinaryType::Nominal { arguments, .. } => arguments.clone(),
        BinaryType::Function(function) => {
            let mut dependencies = function
                .parameters
                .iter()
                .map(|parameter| parameter.ty)
                .collect::<Vec<_>>();
            dependencies.push(function.result);
            dependencies.extend(function.error);
            dependencies
        }
        BinaryType::Closure { function_type, .. }
        | BinaryType::VisibleFunctionTable { function_type } => vec![*function_type],
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => vec![*yield_type, *resume_type, *result_type],
        BinaryType::Entry {
            root, immediate, ..
        } => vec![*root, *immediate],
        BinaryType::Integer(_)
        | BinaryType::Float(_)
        | BinaryType::Pointer(_)
        | BinaryType::Unit
        | BinaryType::Never
        | BinaryType::Generic { .. }
        | BinaryType::Frame { .. }
        | BinaryType::Sampler
        | BinaryType::AccelerationStructure
        | BinaryType::RayQuery
        | BinaryType::HitObject => Vec::new(),
    }
}

fn binary_type_to_tir(
    entries: &[BinaryType],
    declarations: &[BinaryNominalDeclaration],
    index: u64,
    scope: Option<usize>,
    depth: usize,
) -> Result<Type, FormatError> {
    if depth >= 256 {
        return Err(FormatError::malformed(None, "type nesting exceeds 255"));
    }
    let index = usize::try_from(index)
        .map_err(|_| FormatError::malformed(None, "type index does not fit in memory"))?;
    let Some(ty) = entries.get(index) else {
        return Err(FormatError::malformed(None, "type index is out of range"));
    };
    let child = |index| binary_type_to_tir(entries, declarations, index, scope, depth + 1);
    match ty {
        BinaryType::Integer(bits) => {
            Ok(Type::Integer(u16::try_from(*bits).map_err(|_| {
                FormatError::malformed(None, "integer type width exceeds 16 bits")
            })?))
        }
        BinaryType::Float(float) => Ok(Type::Float(*float)),
        BinaryType::Pointer(space) => Ok(Type::Pointer(address_space_from_code(*space)?)),
        BinaryType::Address { pointee, space } => Ok(Type::Address {
            pointee: Box::new(child(*pointee)?),
            space: Some(address_space_from_code(*space)?),
        }),
        BinaryType::Unit => Ok(Type::Unit),
        BinaryType::Never => Ok(Type::Never),
        BinaryType::FixedVector { lanes, element } => Ok(Type::FixedVector {
            lanes: u32::try_from(*lanes)
                .map_err(|_| FormatError::malformed(None, "vector lane count exceeds 32 bits"))?,
            element: Box::new(child(*element)?),
        }),
        BinaryType::ScalableVector { element } => {
            Ok(Type::ScalableVector(Box::new(child(*element)?)))
        }
        BinaryType::Tuple(elements) => Ok(Type::Tuple(
            elements
                .iter()
                .map(|element| child(*element))
                .collect::<Result<_, _>>()?,
        )),
        BinaryType::Array { count, element } => Ok(Type::Array {
            count: *count,
            element: Box::new(child(*element)?),
        }),
        BinaryType::Nominal {
            declaration,
            arguments,
        } => {
            let declaration = declarations
                .get(usize::try_from(*declaration).map_err(|_| {
                    FormatError::malformed(None, "nominal declaration index overflows")
                })?)
                .ok_or_else(|| FormatError::malformed(None, "nominal declaration is missing"))?;
            Ok(Type::Nominal {
                path: declaration.name.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| child(*argument))
                    .collect::<Result<_, _>>()?,
            })
        }
        BinaryType::Generic { parameter } => {
            let parameter = scope
                .and_then(|scope| declarations.get(scope))
                .and_then(|declaration| {
                    usize::try_from(*parameter)
                        .ok()
                        .and_then(|index| declaration.generic_parameters.get(index))
                })
                .ok_or_else(|| {
                    FormatError::malformed(None, "generic parameter has no enclosing declaration")
                })?;
            Ok(Type::Generic(parameter.name.clone()))
        }
        BinaryType::Reference { pointee } => Ok(Type::Reference(Box::new(child(*pointee)?))),
        BinaryType::Unique { pointee } => Ok(Type::Unique(Box::new(child(*pointee)?))),
        BinaryType::Weak { pointee } => Ok(Type::Weak(Box::new(child(*pointee)?))),
        BinaryType::Function(function) => Ok(Type::Function(binary_function_to_tir(
            entries,
            declarations,
            function,
            scope,
            depth + 1,
        )?)),
        BinaryType::Closure {
            function_type,
            borrowed,
        } => {
            let signature =
                function_entry_to_tir(entries, declarations, *function_type, scope, depth + 1)?;
            Ok(Type::Closure(ClosureType {
                borrowed: *borrowed,
                signature,
            }))
        }
        BinaryType::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => Ok(Type::Coroutine {
            yield_type: Box::new(child(*yield_type)?),
            resume_type: Box::new(child(*resume_type)?),
            result_type: Box::new(child(*result_type)?),
        }),
        BinaryType::Frame { function_index } => Err(FormatError::malformed(
            None,
            format!("frame type for function {function_index} requires SIGNATURES"),
        )),
        BinaryType::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => Ok(Type::Texture {
            dimension: texture_dimension_from_code(*dimension)?,
            component: Box::new(child(*component)?),
            access: texture_access_from_code(*access)?,
            multisampled: *multisampled,
        }),
        BinaryType::Sampler => Ok(Type::Sampler),
        BinaryType::AccelerationStructure => Ok(Type::AccelerationStructure),
        BinaryType::VisibleFunctionTable { function_type } => {
            Ok(Type::VisibleFunctionTable(Box::new(function_entry_to_tir(
                entries,
                declarations,
                *function_type,
                scope,
                depth + 1,
            )?)))
        }
        BinaryType::Tensor { rank, element } => Ok(Type::Tensor {
            count: u32::try_from(*rank)
                .map_err(|_| FormatError::malformed(None, "tensor rank exceeds 32 bits"))?,
            element: Box::new(child(*element)?),
        }),
        BinaryType::RayQuery => Ok(Type::RayQuery),
        BinaryType::HitObject => Ok(Type::HitObject),
        BinaryType::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        } => Ok(Type::CooperativeMatrix {
            scope: matrix_scope_from_code(*scope)?,
            rows: u32::try_from(*rows)
                .map_err(|_| FormatError::malformed(None, "matrix row count exceeds 32 bits"))?,
            columns: u32::try_from(*columns)
                .map_err(|_| FormatError::malformed(None, "matrix column count exceeds 32 bits"))?,
            element: Box::new(child(*element)?),
            use_kind: matrix_use_from_code(*use_kind)?,
        }),
        BinaryType::Entry {
            kind,
            root,
            immediate,
        } => Ok(Type::Entry {
            kind: entry_kind_from_code(*kind)?,
            root: Box::new(child(*root)?),
            immediate: Box::new(child(*immediate)?),
        }),
    }
}

fn binary_function_to_tir(
    entries: &[BinaryType],
    declarations: &[BinaryNominalDeclaration],
    function: &BinaryFunctionType,
    scope: Option<usize>,
    depth: usize,
) -> Result<FunctionType, FormatError> {
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| {
            let convention = match parameter.convention {
                0 => Some(Convention::Owned),
                1 => Some(Convention::Borrow),
                2 => Some(Convention::BorrowMut),
                3 => None,
                _ => {
                    return Err(FormatError::malformed(
                        None,
                        "function type has an invalid parameter convention",
                    ));
                }
            };
            Ok(ParameterType {
                context: parameter.context,
                convention,
                ty: binary_type_to_tir(entries, declarations, parameter.ty, scope, depth + 1)?,
            })
        })
        .collect::<Result<_, FormatError>>()?;
    Ok(FunctionType {
        parameters,
        result: Box::new(binary_type_to_tir(
            entries,
            declarations,
            function.result,
            scope,
            depth + 1,
        )?),
        failure: function
            .error
            .map(|error| binary_type_to_tir(entries, declarations, error, scope, depth + 1))
            .transpose()?
            .map(Box::new),
        asynchronous: function.asynchronous,
        abi: if function.c_abi { Abi::C } else { Abi::Ridl },
        foreign: function.foreign,
    })
}

fn function_entry_to_tir(
    entries: &[BinaryType],
    declarations: &[BinaryNominalDeclaration],
    index: u64,
    scope: Option<usize>,
    depth: usize,
) -> Result<FunctionType, FormatError> {
    let index = usize::try_from(index)
        .map_err(|_| FormatError::malformed(None, "function type index overflows"))?;
    let Some(BinaryType::Function(function)) = entries.get(index) else {
        return Err(FormatError::malformed(
            None,
            "closure type does not reference a function",
        ));
    };
    binary_function_to_tir(entries, declarations, function, scope, depth)
}

fn address_space_from_code(code: u8) -> Result<AddressSpace, FormatError> {
    match code {
        0 => Ok(AddressSpace::Generic),
        1 => Ok(AddressSpace::Device),
        2 => Ok(AddressSpace::Constant),
        3 => Ok(AddressSpace::Threadgroup),
        4 => Ok(AddressSpace::Thread),
        5 => Ok(AddressSpace::ObjectPayload),
        6 => Ok(AddressSpace::RayPayload),
        7 => Ok(AddressSpace::HitAttribute),
        8 => Ok(AddressSpace::Tile),
        9 => Ok(AddressSpace::NodeRecord),
        _ => Err(FormatError::malformed(None, "invalid address space")),
    }
}

fn texture_dimension_from_code(code: u8) -> Result<TextureDimension, FormatError> {
    match code {
        0 => Ok(TextureDimension::D1),
        1 => Ok(TextureDimension::D2),
        2 => Ok(TextureDimension::D3),
        3 => Ok(TextureDimension::Cube),
        4 => Ok(TextureDimension::D1Array),
        5 => Ok(TextureDimension::D2Array),
        6 => Ok(TextureDimension::CubeArray),
        _ => Err(FormatError::malformed(None, "invalid texture dimension")),
    }
}

fn texture_access_from_code(code: u8) -> Result<TextureAccess, FormatError> {
    match code {
        0 => Ok(TextureAccess::Sample),
        1 => Ok(TextureAccess::Read),
        2 => Ok(TextureAccess::Write),
        3 => Ok(TextureAccess::ReadWrite),
        _ => Err(FormatError::malformed(None, "invalid texture access")),
    }
}

fn matrix_scope_from_code(code: u8) -> Result<MatrixScope, FormatError> {
    match code {
        0 => Ok(MatrixScope::Subgroup),
        1 => Ok(MatrixScope::Threadgroup),
        _ => Err(FormatError::malformed(
            None,
            "invalid cooperative matrix scope",
        )),
    }
}

fn matrix_use_from_code(code: u8) -> Result<CooperativeMatrixUse, FormatError> {
    match code {
        0 => Ok(CooperativeMatrixUse::A),
        1 => Ok(CooperativeMatrixUse::B),
        2 => Ok(CooperativeMatrixUse::Acc),
        _ => Err(FormatError::malformed(
            None,
            "invalid cooperative matrix use",
        )),
    }
}

fn entry_kind_from_code(code: u8) -> Result<EntryKind, FormatError> {
    match code {
        0 => Ok(EntryKind::Compute),
        1 => Ok(EntryKind::Object),
        2 => Ok(EntryKind::Mesh),
        3 => Ok(EntryKind::Vertex),
        4 => Ok(EntryKind::Fragment),
        5 => Ok(EntryKind::Tile),
        6 => Ok(EntryKind::Raygen),
        7 => Ok(EntryKind::Intersection),
        8 => Ok(EntryKind::Anyhit),
        9 => Ok(EntryKind::Closesthit),
        10 => Ok(EntryKind::Miss),
        11 => Ok(EntryKind::Callable),
        12 => Ok(EntryKind::Node),
        _ => Err(FormatError::malformed(None, "invalid entry type kind")),
    }
}

fn add_string(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|candidate| candidate == value) {
        values.push(value.to_owned());
    }
}

fn parse_source_hash(value: &str) -> Result<[u8; 32], FormatError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(FormatError::malformed(
            None,
            "driver source hash must be 64 lowercase hexadecimal digits",
        ));
    }
    let mut result = [0; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_digit(pair[0])
            .ok_or_else(|| FormatError::malformed(None, "invalid source hash"))?;
        let low = hex_digit(pair[1])
            .ok_or_else(|| FormatError::malformed(None, "invalid source hash"))?;
        result[index] = (high << 4) | low;
    }
    Ok(result)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn quote(value: &str) -> Result<String, FormatError> {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            '\r' => output.push_str("\\r"),
            '\u{1b}' => output.push_str("\\e"),
            '\0' => output.push_str("\\0"),
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            character if character.is_control() => {
                return Err(FormatError::malformed(
                    None,
                    format!(
                        "text TIR has no string escape for control character U+{:04X}",
                        character as u32
                    ),
                ));
            }
            character => output.push(character),
        }
    }
    output.push('"');
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_assembly_and_disassembly_roundtrip_canonical_bytes() {
        let source = concat!(
            "module @Demo.Empty tir 26.11.1 profile app\n",
            "  producer \"klf\" \"26.11.1\" languages [kira]\n",
            "  attribute \"purpose\" = \"test\\nmodule\"\n",
            "type %Demo.Value = struct explicit(size 4, align 4) {\n",
            "  value: i32 @0\n",
            "}\n",
            "policy {copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none, lifetime: eager}\n",
            "type %Demo.Other = struct explicit(size 4, align 4) {\n",
            "  other: i32 @0\n",
            "}\n",
            "policy {copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none, lifetime: eager}\n",
            "import package @Demo.Dependency\n",
        );
        let parsed = parse_text_module(source).unwrap();
        let bytes = assemble_text_module(&parsed).unwrap();
        let binary = BinaryModule::decode(&bytes).unwrap();
        let strings = binary.string_table(u64::MAX).unwrap();
        let referenced = [
            "Demo.Empty",
            "klf",
            "26.11.1",
            "kira",
            "purpose",
            "test\nmodule",
            "Demo.Dependency",
            "Demo.Value",
            "Demo.Other",
            "value",
            "other",
        ]
        .map(|value| strings.index_of(value).unwrap());
        assert!(referenced.windows(2).all(|indices| indices[0] < indices[1]));
        let text = disassemble_binary_module(&binary).unwrap();
        let reparsed = parse_text_module(&text).unwrap();
        assert_eq!(assemble_text_module(&reparsed).unwrap(), bytes);
        assert!(text.contains("import package @Demo.Dependency"));
        assert!(text.contains("type %Demo.Value = struct explicit(size 4, align 4)"));
        assert!(text.contains("type %Demo.Other = struct explicit(size 4, align 4)"));
    }

    #[test]
    fn driver_source_hash_roundtrips_as_lowercase_hex() {
        let source = concat!(
            "module @Demo.Driver tir 26.11.1 profile driver platform tessera\n",
            "  producer \"klf\" \"26.11.1\" languages [kira]\n",
            "  source \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\"\n",
        );
        let parsed = parse_text_module(source).unwrap();
        let bytes = assemble_text_module(&parsed).unwrap();
        let binary = BinaryModule::decode(&bytes).unwrap();
        let text = disassemble_binary_module(&binary).unwrap();
        assert!(text.contains(
            "source \"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\""
        ));
        let reparsed = parse_text_module(&text).unwrap();
        assert_eq!(assemble_text_module(&reparsed).unwrap(), bytes);
    }
}
