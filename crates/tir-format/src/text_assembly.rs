use std::fmt::Write as _;

use tir_diag::Span;
use tir_target::Platform;

use crate::{
    BinaryAddressSpace, BinaryAnalysisSummary, BinaryCodeTable, BinaryConstant,
    BinaryConstantTable, BinaryFunctionAnalysis, BinaryGlobalTable, BinaryImportEntityKind,
    BinaryImportKind, BinaryModule, BinaryModuleBuilder, BinaryModuleRecord, BinaryProducer,
    BinarySignatureTable, Header, ModuleHeader, SemanticsMode, TextItemKind, TextModule,
    TextParseError, parse_text_module,
};

/// Assembles supported text records into a canonical binary container.
pub fn assemble_text_module(module: &TextModule) -> Result<Vec<u8>, TextParseError> {
    let mut parsed_imports = Vec::new();
    let mut parsed_globals = Vec::new();
    let mut parsed_functions = Vec::new();
    for item in &module.items {
        match item.kind {
            TextItemKind::Type => {}
            TextItemKind::Import => parsed_imports.push(crate::text_import_assembly::parse_import(
                module.item_text(*item),
                *item,
            )?),
            TextItemKind::Global => parsed_globals.push(crate::text_global_assembly::parse_global(
                module.item_text(*item),
                *item,
            )?),
            TextItemKind::Function => parsed_functions.push(
                crate::text_signature_assembly::parse_function(module.item_text(*item), *item)?,
            ),
            kind => {
                return Err(error(
                    item.span,
                    format!("tirc asm does not yet support {} items", item_name(kind)),
                ));
            }
        }
    }
    if !module.header.initializers.is_empty() || !module.header.finalizers.is_empty() {
        return Err(error(
            Span::new(0, 0),
            "tirc asm requires initializer and finalizer functions to be declared",
        ));
    }
    for function in &parsed_functions {
        let poison = function
            .body
            .as_ref()
            .and_then(|body| body.blocks.first())
            .and_then(|block| block.instructions.first())
            .is_some_and(|instruction| instruction.opcode == "const.poison");
        let semantics = function
            .record
            .semantics
            .or(module.header.semantics)
            .unwrap_or(SemanticsMode::Defined);
        if poison && semantics != SemanticsMode::Permissive {
            return Err(error(
                function.item.span,
                "const.poison requires permissive semantics",
            ));
        }
    }

    let mut function_float_constants = Vec::with_capacity(parsed_functions.len());
    for function in &parsed_functions {
        let constant = function
            .body
            .as_ref()
            .and_then(|body| body.blocks.first())
            .and_then(|block| block.instructions.first())
            .filter(|instruction| instruction.opcode == "const.float")
            .map(|instruction| {
                crate::text_function_constant_assembly::parse_entry_constant(
                    instruction,
                    module.item_text(function.item),
                    function.item,
                )
            })
            .transpose()?;
        function_float_constants.push(constant);
    }
    let function_code_strings = parsed_functions
        .iter()
        .map(|function| {
            function
                .body
                .as_ref()
                .and_then(|body| body.blocks.first())
                .and_then(|block| block.instructions.first())
                .filter(|instruction| instruction.opcode == "const.string")
                .map(|instruction| {
                    crate::text_function_constant_assembly::parse_entry_constant(
                        instruction,
                        module.item_text(function.item),
                        function.item,
                    )
                    .map(|constant| match constant {
                        crate::text_function_constant_assembly::ParsedEntryConstant::String(
                            value,
                        ) => value.value,
                        _ => unreachable!("const.string parser returned another constant kind"),
                    })
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;

    let leading_roots = parsed_imports
        .iter()
        .flat_map(|import| {
            import
                .entity_types
                .iter()
                .cloned()
                .map(|ty| (ty, import.item))
        })
        .collect::<Vec<_>>();
    let mut middle_roots = parsed_globals
        .iter()
        .filter(|global| global.initializer.is_some())
        .map(|global| (global.ty.clone(), global.item))
        .collect::<Vec<_>>();
    for (function, constant) in parsed_functions.iter().zip(&function_float_constants) {
        if let Some(constant) = constant {
            middle_roots.push((constant.ty().clone(), function.item));
        }
    }
    let mut trailing_roots = parsed_globals
        .iter()
        .map(|global| (global.ty.clone(), global.item))
        .collect::<Vec<_>>();
    trailing_roots.extend(
        parsed_functions
            .iter()
            .map(|function| (function.ty.clone(), function.item)),
    );
    for function in &parsed_functions {
        if let Some(body) = &function.body {
            for block in body.blocks.iter().skip(1) {
                for parameter in &block.parameters {
                    trailing_roots.push((parameter.ty.clone(), function.item));
                }
            }
        }
    }
    // CODE can use types not present in a function signature. Intern them
    // after SIGNATURES roots in first-use order, including implicit i1 results.
    for function in &parsed_functions {
        if let Some(body) = &function.body {
            for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                match instruction.opcode.as_str() {
                    "stack.alloc" => {
                        let memory = crate::text_memory_assembly::parse(
                            instruction,
                            module.item_text(function.item),
                            function.item,
                        )?;
                        if let crate::text_memory_assembly::MemoryOperation::StackAlloc { ty } =
                            memory
                        {
                            trailing_roots.push((
                                tir_types::Type::Address {
                                    pointee: Box::new(ty),
                                    space: None,
                                },
                                function.item,
                            ));
                        }
                    }
                    "icmp" | "const.bool" => {
                        trailing_roots.push((tir_types::Type::Integer(1), function.item))
                    }
                    "const.int" if body.blocks.iter().any(|block| block.instructions.len() > 1) => {
                        let constant =
                            crate::text_function_constant_assembly::parse_entry_constant(
                                instruction,
                                module.item_text(function.item),
                                function.item,
                            )?;
                        trailing_roots.push((constant.ty().clone(), function.item));
                    }
                    _ => {}
                }
            }
        }
    }
    let (type_table, import_indices, constant_type_indices, global_indices) =
        crate::text_type_assembly::parse_text_type_table_with_roots(
            module,
            &leading_roots,
            &middle_roots,
            &trailing_roots,
        )?;
    let mut import_indices = import_indices.into_iter();
    let mut imports = Vec::with_capacity(parsed_imports.len());
    for mut parsed in parsed_imports {
        for entity in &mut parsed.record.entities {
            entity.type_index = import_indices.next().ok_or_else(|| {
                error(
                    parsed.item.span,
                    "imported entity type index was not lowered",
                )
            })?;
        }
        imports.push(parsed.record);
    }
    let mut constant_type_indices = constant_type_indices.into_iter();
    let mut global_indices = global_indices.into_iter();
    let mut constants = BinaryConstantTable::default();
    let mut globals = Vec::with_capacity(parsed_globals.len());
    for mut parsed in parsed_globals {
        if let Some(value) = parsed.initializer {
            let ty = constant_type_indices.next().ok_or_else(|| {
                error(
                    parsed.item.span,
                    "global constant type index was not lowered",
                )
            })?;
            let index = u64::try_from(constants.records.len())
                .map_err(|_| error(parsed.item.span, "global constant count exceeds ULEB128"))?;
            constants.records.push(BinaryConstant { ty, value });
            parsed.record.initializer = Some(index);
        }
        parsed.record.ty = global_indices
            .next()
            .ok_or_else(|| error(parsed.item.span, "global type index was not lowered"))?;
        globals.push(parsed.record);
    }
    let mut function_float_indices = Vec::with_capacity(parsed_functions.len());
    for (function, constant) in parsed_functions.iter().zip(&function_float_constants) {
        let Some(crate::text_function_constant_assembly::ParsedEntryConstant::Float(value)) =
            constant
        else {
            function_float_indices.push(None);
            continue;
        };
        let ty = constant_type_indices.next().ok_or_else(|| {
            error(
                function.item.span,
                "function constant type index was not lowered",
            )
        })?;
        let index = u64::try_from(constants.records.len())
            .map_err(|_| error(function.item.span, "constant count exceeds ULEB128"))?;
        constants.records.push(BinaryConstant {
            ty,
            value: crate::types::BinaryConstantValue::Float(value.bytes.clone()),
        });
        function_float_indices.push(Some((ty, index)));
    }
    let mut signatures = Vec::with_capacity(parsed_functions.len());
    for parsed in &mut parsed_functions {
        parsed.record.function_type = global_indices
            .next()
            .ok_or_else(|| error(parsed.item.span, "function type index was not lowered"))?;
        signatures.push(parsed.record.clone());
    }

    let mut block_parameter_indices = Vec::with_capacity(parsed_functions.len());
    for function in &parsed_functions {
        let mut blocks = Vec::new();
        if let Some(body) = &function.body {
            for (block_index, block) in body.blocks.iter().enumerate() {
                let mut parameters = Vec::with_capacity(block.parameters.len());
                if block_index != 0 {
                    for _ in &block.parameters {
                        parameters.push(global_indices.next().ok_or_else(|| {
                            error(
                                function.item.span,
                                "block parameter type index was not lowered",
                            )
                        })?);
                    }
                }
                blocks.push(parameters);
            }
        }
        block_parameter_indices.push(blocks);
    }

    let header = &module.header;
    let source_hash = parse_source_hash(header)?;
    let record = BinaryModuleRecord {
        name: header.path.clone(),
        producer: header.producer.as_ref().map(|producer| BinaryProducer {
            name: producer.name.clone(),
            version: producer.version.clone(),
            languages: producer.languages.clone(),
        }),
        semantics: header.semantics.unwrap_or(SemanticsMode::Defined),
        attributes: header.attributes.clone(),
        initializers: Vec::new(),
        finalizers: Vec::new(),
        source_hash,
    };

    let mut builder = BinaryModuleBuilder::new(
        Header {
            year: header.version.year,
            month: header.version.month,
            week: header.version.week,
            revision: 0,
            flags: (1 << 6) | 1,
            profile: header.profile,
            platform: header.platform.unwrap_or(Platform::Portable),
            section_count: 0,
            file_size: 0,
        },
        record,
    );
    builder.imports = imports;
    builder.types = type_table;
    builder.constants = constants;
    builder.globals = BinaryGlobalTable { records: globals };
    builder.signatures = BinarySignatureTable {
        records: signatures,
    };
    builder.code_strings = function_code_strings.iter().flatten().cloned().collect();
    let function_string_indices = function_code_strings
        .iter()
        .map(|value| {
            value
                .as_ref()
                .map(|value| {
                    builder.string_index(value).and_then(|index| {
                        index.ok_or_else(|| {
                            crate::FormatError::malformed(
                                None,
                                "const.string was not interned in STRINGS",
                            )
                        })
                    })
                })
                .transpose()
                .map_err(|format_error| error(Span::new(0, 0), format_error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut code_functions = Vec::new();
    for (function_position, parsed) in parsed_functions.iter().enumerate() {
        let Some(body) = &parsed.body else {
            continue;
        };
        let function = match builder.types.entries.get(
            usize::try_from(parsed.record.function_type)
                .map_err(|_| error(parsed.item.span, "function type index overflows memory"))?,
        ) {
            Some(crate::types::BinaryType::Function(function)) => function,
            _ => return Err(error(parsed.item.span, "function type was not lowered")),
        };
        let function_index = u64::try_from(function_position)
            .map_err(|_| error(parsed.item.span, "SIGNATURES count exceeds ULEB128"))?;
        code_functions.push(crate::text_function_body_encoding::encode_return_body(
            body,
            function_index,
            function,
            &builder.types,
            function_float_indices[function_position],
            function_string_indices[function_position],
            &block_parameter_indices[function_position],
            module.item_text(parsed.item),
            parsed.item,
        )?);
    }
    builder.code = BinaryCodeTable {
        functions: code_functions,
    };
    builder.analysis.functions = vec![
        BinaryFunctionAnalysis {
            trust_class: 0,
            reason_mask: 0,
        };
        builder.signatures.records.len()
    ];
    builder
        .encode()
        .map_err(|format_error| error(Span::new(0, 0), format_error.to_string()))
}

/// Disassembles a canonical binary module to its normalized text form.
pub fn disassemble_text_module(
    module: &BinaryModule,
    memory_limit: u64,
) -> Result<String, crate::FormatError> {
    if !module.header.claims_canonical() {
        return Err(crate::FormatError::noncanonical(
            None,
            "tirc dis requires a canonical TIR module",
        ));
    }
    if module
        .sections
        .iter()
        .any(|section| !matches!(section.kind, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 14))
    {
        return Err(crate::FormatError::malformed(
            None,
            "tirc dis does not yet support nonempty TIR sections",
        ));
    }
    let record = module.module_record(memory_limit)?;
    let imports = module.imports(memory_limit)?;
    let types = module.type_table(memory_limit)?;
    let signatures = module.signature_table(memory_limit, &types)?;
    let code = module.code_table(memory_limit, &signatures)?;
    let constants = module.constant_table(memory_limit, &types)?;
    let strings = module.string_table(memory_limit)?;
    let globals = module.global_table(memory_limit, &types, &constants)?;
    let type_text = crate::text_type_disassembly::format_type_declarations(&types, &signatures)?;
    let signature_text = crate::text_signature_disassembly::format_function_declarations(
        &signatures,
        &types,
        &code,
        &constants,
        &strings,
    )?;
    let analysis = module.analysis_summary(memory_limit)?;
    if !is_empty_analysis(&analysis, signatures.records.len())
        || !record.initializers.is_empty()
        || !record.finalizers.is_empty()
    {
        return Err(crate::FormatError::malformed(
            None,
            "tirc dis does not yet support function references",
        ));
    }

    let header = &module.header;
    let mut text = format!(
        "module @{} tir {}.{}.{} profile {} platform {} semantics {}",
        record.name,
        header.year,
        header.month,
        header.week,
        profile_name(header.profile),
        platform_name(header.platform),
        semantics_name(record.semantics),
    );
    if let Some(producer) = record.producer {
        write!(
            text,
            "\n  producer {} {} languages [{}]",
            crate::text_global_disassembly::quote(&producer.name)?,
            crate::text_global_disassembly::quote(&producer.version)?,
            producer.languages.join(", "),
        )
        .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
    }
    for (key, value) in record.attributes {
        write!(
            text,
            "\n  attribute {} = {}",
            crate::text_global_disassembly::quote(&key)?,
            crate::text_global_disassembly::quote(&value)?
        )
        .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
    }
    if let Some(source_hash) = record.source_hash {
        text.push_str("\n  source \"");
        for byte in source_hash {
            write!(text, "{byte:02x}")
                .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
        }
        text.push('\"');
    }
    for import in imports {
        match import.kind {
            BinaryImportKind::Ridl => {
                write!(
                    text,
                    "\nimport ridl @{} abi {}",
                    import.name,
                    crate::text_global_disassembly::quote(&format_uuid(import.abi_uuid))?,
                )
                .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
            }
            BinaryImportKind::Package => {
                write!(text, "\nimport package @{}", import.name).map_err(|_| {
                    crate::FormatError::malformed(None, "TIR text formatting failed")
                })?;
            }
            BinaryImportKind::Platform => {
                write!(
                    text,
                    "\nimport platform {}{}",
                    crate::text_global_disassembly::quote(&import.name)?,
                    if import.weak { " weak" } else { "" },
                )
                .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
            }
        }
        if !import.entities.is_empty() {
            text.push_str(" {");
            for entity in import.entities {
                let ty = crate::text_type_disassembly::format_type_reference(
                    &types,
                    entity.type_index,
                    &signatures,
                )?;
                let kind = match entity.kind {
                    BinaryImportEntityKind::Function => "func",
                    BinaryImportEntityKind::Global => "global",
                    BinaryImportEntityKind::Type => "type",
                    BinaryImportEntityKind::Witness => "witness",
                    BinaryImportEntityKind::Descriptor => "descriptor",
                    BinaryImportEntityKind::PlatformData => "data",
                };
                write!(text, "\n    {kind} @{}", entity.name).map_err(|_| {
                    crate::FormatError::malformed(None, "TIR text formatting failed")
                })?;
                if let Some(symbol) = entity.platform_symbol {
                    write!(
                        text,
                        " symbol {}",
                        crate::text_global_disassembly::quote(&symbol)?
                    )
                    .map_err(|_| {
                        crate::FormatError::malformed(None, "TIR text formatting failed")
                    })?;
                }
                write!(text, " : {ty}").map_err(|_| {
                    crate::FormatError::malformed(None, "TIR text formatting failed")
                })?;
            }
            text.push_str("\n  }");
        }
    }
    if !type_text.is_empty() {
        text.push('\n');
        text.push_str(&type_text);
    }
    for global in globals.records {
        let ty =
            crate::text_type_disassembly::format_type_reference(&types, global.ty, &signatures)?;
        write!(
            text,
            "\nglobal {} @{} : {}",
            crate::text_global_disassembly::linkage_name(global.linkage),
            global.name,
            ty
        )
        .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
        if global.address_space != BinaryAddressSpace::Generic {
            write!(
                text,
                " <{}>",
                crate::text_global_disassembly::address_space_name(global.address_space)
            )
            .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
        }
        if let Some(index) = global.initializer {
            let constant = constants
                .records
                .get(usize::try_from(index).map_err(|_| {
                    crate::FormatError::malformed(None, "global initializer index overflows memory")
                })?)
                .ok_or_else(|| {
                    crate::FormatError::malformed(None, "global initializer index is out of range")
                })?;
            if constant.ty != global.ty {
                return Err(crate::FormatError::malformed(
                    None,
                    "global initializer type does not match its declared type",
                ));
            }
            let value =
                crate::text_global_disassembly::format_global_initializer(constant, &types)?;
            write!(text, " = {value}")
                .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
        }
        if global.thread_local {
            text.push_str(" thread_local");
        }
        if global.constant {
            text.push_str(" constant");
        }
        if global.per_cpu {
            text.push_str(" percpu");
        }
        if let Some(alignment) = global.alignment {
            write!(text, " align({alignment})")
                .map_err(|_| crate::FormatError::malformed(None, "TIR text formatting failed"))?;
        }
    }
    text.push_str(&signature_text);
    text.push('\n');
    parse_text_module(&text)
        .map_err(|error| crate::FormatError::malformed(Some(error.span.start), error.message))?;
    Ok(text)
}

fn is_empty_analysis(analysis: &BinaryAnalysisSummary, signature_count: usize) -> bool {
    analysis.functions.len() == signature_count
        && analysis
            .functions
            .iter()
            .all(|function| function.trust_class == 0 && function.reason_mask == 0)
        && analysis.sandbox_regions.is_empty()
        && analysis.indirect_call_typeids.is_empty()
        && analysis.assembly_regions.is_empty()
}

fn profile_name(profile: tir_target::Profile) -> &'static str {
    match profile {
        tir_target::Profile::App => "app",
        tir_target::Profile::System => "system",
        tir_target::Profile::Driver => "driver",
        tir_target::Profile::Kernel => "kernel",
    }
}

fn platform_name(platform: tir_target::Platform) -> &'static str {
    match platform {
        Platform::Portable => "portable",
        Platform::Tessera => "tessera",
        Platform::Macos => "macos",
        Platform::Ios => "ios",
        Platform::Windows => "windows",
        Platform::Linux => "linux",
        Platform::Android => "android",
        Platform::Web => "web",
        Platform::Wasi => "wasi",
    }
}

fn semantics_name(semantics: SemanticsMode) -> &'static str {
    match semantics {
        SemanticsMode::Defined => "defined",
        SemanticsMode::Permissive => "permissive",
    }
}

fn format_uuid(uuid: [u8; 16]) -> String {
    let mut text = String::with_capacity(36);
    for (index, byte) in uuid.into_iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn parse_source_hash(header: &ModuleHeader) -> Result<Option<[u8; 32]>, TextParseError> {
    let Some(source_hash) = &header.source_hash else {
        return if header.profile == tir_target::Profile::Driver {
            Err(error(
                Span::new(0, 0),
                "driver module header is missing its source hash",
            ))
        } else {
            Ok(None)
        };
    };
    if header.profile != tir_target::Profile::Driver {
        return Err(error(
            Span::new(0, 0),
            "source hash is only valid for driver modules",
        ));
    }
    if source_hash.len() != 64
        || !source_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(error(
            Span::new(0, 0),
            "driver source hash must contain 64 lowercase hexadecimal digits",
        ));
    }

    let mut bytes = [0; 32];
    for (index, pair) in source_hash.as_bytes().chunks_exact(2).enumerate() {
        let value = std::str::from_utf8(pair)
            .ok()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            .ok_or_else(|| error(Span::new(0, 0), "invalid driver source hash"))?;
        bytes[index] = value;
    }
    Ok(Some(bytes))
}

fn item_name(kind: TextItemKind) -> &'static str {
    match kind {
        TextItemKind::Import => "import",
        TextItemKind::Type => "type",
        TextItemKind::Global => "global",
        TextItemKind::Function => "function",
        TextItemKind::Entry => "entry",
        TextItemKind::Variants => "variants",
        TextItemKind::Trait => "trait",
        TextItemKind::Conformance => "conformance",
        TextItemKind::VTable => "vtable",
    }
}

fn error(span: Span, message: impl Into<String>) -> TextParseError {
    TextParseError {
        span,
        message: message.into(),
    }
}
