use crate::text_assembly_helpers::*;
use std::fmt::Write as _;

use tir_diag::Span;
use tir_target::Platform;

use crate::{
    BinaryAddressSpace, BinaryCodeTable, BinaryConstant, BinaryConstantTable, BinaryGlobalTable,
    BinaryImportEntityKind, BinaryImportKind, BinaryModule, BinaryModuleBuilder,
    BinaryModuleRecord, BinaryProducer, BinarySignatureTable, Header, SemanticsMode, TextItemKind,
    TextModule, TextParseError, parse_text_module,
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
    let callees =
        crate::text_direct_call::index_callees(&parsed_functions, &parsed_globals, module)?;
    for function in &parsed_functions {
        let Some(body) = &function.body else {
            continue;
        };
        if body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| {
                crate::text_gpu_texture_atomic::handles(&instruction.opcode)
                    || crate::text_gpu_texture_sample::handles(&instruction.opcode)
                    || crate::text_gpu_texture_basic::handles(&instruction.opcode)
                    || crate::text_gpu_tile_node::handles(&instruction.opcode)
                    || crate::text_gpu_mesh::handles(&instruction.opcode)
                    || crate::text_gpu_subgroup::handles(&instruction.opcode)
                    || matches!(
                        instruction.opcode.as_str(),
                        "call"
                            | "tail.call"
                            | "func.ref"
                            | "global.addr"
                            | "tls.addr"
                            | "enum.tag"
                            | "struct.extract"
                            | "struct.destructure"
                            | "enum.take"
                            | "enum.project"
                            | "field.addr"
                            | "elem.addr"
                            | "enum.payload.addr"
                            | "enum.inject"
                            | "addr.toptr"
                            | "ptr.toaddr"
                            | "addr.cast"
                            | "const.aggregate"
                            | "gpu.builtin"
                            | "gpu.spec_const"
                            | "frag.demote"
                            | "frag.load_attachment"
                            | "frag.raster_order.begin"
                            | "frag.raster_order.end"
                            | "frag.imageblock.addr"
                            | "gpu.printf"
                            | "gpu.ddx"
                            | "gpu.ddy"
                            | "gpu.fwidth"
                            | "quad.broadcast"
                            | "quad.swap"
                            | "quad.all"
                            | "quad.any"
                            | "gpu.barrier"
                            | "gpu.memfence"
                    )
            })
        {
            let tir_types::Type::Function(signature) = &function.ty else {
                unreachable!()
            };
            crate::text_function_body_assembly::validate_return_body(
                body,
                signature,
                module.item_text(function.item),
                function.item,
                Some(&callees),
            )?;
        }
    }
    if !module.header.initializers.is_empty() || !module.header.finalizers.is_empty() {
        return Err(error(
            Span::new(0, 0),
            "tirc asm requires initializer and finalizer functions to be declared",
        ));
    }
    for function in &parsed_functions {
        let poison = function.body.as_ref().is_some_and(|body| {
            body.blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|instruction| instruction.opcode == "const.poison")
        });
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
        let mut constants = Vec::new();
        if let Some(body) = &function.body {
            for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                if instruction.opcode == "const.float" {
                    let constant = crate::text_function_constant_assembly::parse_entry_constant(
                        instruction,
                        module.item_text(function.item),
                        function.item,
                    )?;
                    constants.push((instruction.span.start, constant));
                }
            }
        }
        function_float_constants.push(constants);
    }
    let function_large_vectors =
        crate::text_vector_constant_large::collect(module, &parsed_functions)?;
    let function_aggregates =
        crate::text_aggregate_constant_intern::collect(module, &parsed_functions, &callees)?;
    let function_gpu_specializations =
        crate::text_gpu_spec_const_intern::collect(module, &parsed_functions)?;
    let function_code_strings = crate::text_code_strings::collect(module, &parsed_functions)?;

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
    for (function, constants) in parsed_functions.iter().zip(&function_float_constants) {
        for (_, constant) in constants {
            middle_roots.push((constant.ty().clone(), function.item));
        }
    }
    crate::text_vector_constant_large::append_roots(
        &parsed_functions,
        &function_large_vectors,
        &mut middle_roots,
    );
    crate::text_aggregate_constant_intern::append_roots(
        &parsed_functions,
        &function_aggregates,
        &mut middle_roots,
    );
    crate::text_gpu_spec_const_intern::append_roots(
        &parsed_functions,
        &function_gpu_specializations,
        &mut middle_roots,
    );
    let mut trailing_roots = parsed_globals
        .iter()
        .map(|global| (global.ty.clone(), global.item))
        .collect::<Vec<_>>();
    trailing_roots.extend(
        parsed_functions
            .iter()
            .map(|function| (function.ty.clone(), function.item)),
    );
    let block_parameter_positions = crate::text_code_type_roots::collect(
        module,
        &parsed_functions,
        &callees,
        &mut trailing_roots,
    )?;
    let (type_table, import_indices, constant_type_indices, global_indices) =
        crate::text_type_assembly::parse_text_type_table_with_roots(
            module,
            &leading_roots,
            &middle_roots,
            &trailing_roots,
        )?;
    for function in &parsed_functions {
        let Some(body) = &function.body else {
            continue;
        };
        if body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| crate::text_nominal_construction::handles(&instruction.opcode))
        {
            crate::text_nominal_construction::validate_typed(
                body,
                module.item_text(function.item),
                function.item,
                &type_table,
                &callees,
            )?;
        }
    }
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
    let mut global_index_cursor = global_indices.iter().copied();
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
        parsed.record.ty = global_index_cursor
            .next()
            .ok_or_else(|| error(parsed.item.span, "global type index was not lowered"))?;
        globals.push(parsed.record);
    }
    let mut function_float_indices = Vec::with_capacity(parsed_functions.len());
    let mut function_float_maps = Vec::with_capacity(parsed_functions.len());
    for (function, float_constants) in parsed_functions.iter().zip(&function_float_constants) {
        let mut by_span = std::collections::HashMap::new();
        for (span, constant) in float_constants {
            let crate::text_function_constant_assembly::ParsedEntryConstant::Float(value) =
                constant
            else {
                unreachable!("const.float parser returned a different constant kind")
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
            by_span.insert(*span, (ty, index));
        }
        let entry_span = function
            .body
            .as_ref()
            .and_then(|body| body.blocks.first())
            .and_then(|block| block.instructions.first())
            .map(|instruction| instruction.span.start);
        function_float_indices.push(entry_span.and_then(|span| by_span.get(&span).copied()));
        function_float_maps.push(by_span);
    }
    crate::text_vector_constant_large::intern(
        &parsed_functions,
        &function_large_vectors,
        &mut constant_type_indices,
        &mut constants,
        &mut function_float_maps,
    )?;
    crate::text_aggregate_constant_intern::intern(
        &parsed_functions,
        function_aggregates,
        &mut constant_type_indices,
        &mut constants,
        &mut function_float_maps,
    )?;
    crate::text_gpu_spec_const_intern::intern(
        &parsed_functions,
        function_gpu_specializations,
        &mut constant_type_indices,
        &mut constants,
        &mut function_float_maps,
    )?;
    let mut signatures = Vec::with_capacity(parsed_functions.len());
    for parsed in &mut parsed_functions {
        parsed.record.function_type = global_index_cursor
            .next()
            .ok_or_else(|| error(parsed.item.span, "function type index was not lowered"))?;
        signatures.push(parsed.record.clone());
    }

    let mut block_parameter_indices = Vec::with_capacity(parsed_functions.len());
    for (function, positions) in parsed_functions.iter().zip(&block_parameter_positions) {
        let mut blocks = Vec::new();
        if let Some(body) = &function.body {
            for (block_index, block) in body.blocks.iter().enumerate() {
                let mut parameters = Vec::with_capacity(block.parameters.len());
                if block_index != 0 {
                    for root_position in &positions[block_index] {
                        parameters.push(*global_indices.get(*root_position).ok_or_else(|| {
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
    let default_semantics = header.semantics.unwrap_or(SemanticsMode::Defined);
    let defined_only = parsed_functions.iter().all(|function| {
        function.record.semantics.unwrap_or(default_semantics) == SemanticsMode::Defined
    });

    let mut builder = BinaryModuleBuilder::new(
        Header {
            year: header.version.year,
            month: header.version.month,
            week: header.version.week,
            revision: 0,
            flags: (1 << 6) | u32::from(defined_only),
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
    builder.code_strings = function_code_strings
        .iter()
        .flatten()
        .map(|(_, value)| value.clone())
        .collect();
    let mut function_string_indices = Vec::with_capacity(parsed_functions.len());
    let mut function_string_maps = Vec::with_capacity(parsed_functions.len());
    for (function, strings) in parsed_functions.iter().zip(&function_code_strings) {
        let mut by_span = std::collections::HashMap::new();
        for (span, value) in strings {
            let index = builder
                .string_index(value)
                .map_err(|format_error| error(function.item.span, format_error.to_string()))?
                .ok_or_else(|| {
                    error(
                        function.item.span,
                        "const.string was not interned in STRINGS",
                    )
                })?;
            by_span.insert(*span, index);
        }
        let first_instruction = function
            .body
            .as_ref()
            .and_then(|body| body.blocks.first())
            .and_then(|block| block.instructions.first())
            .map(|instruction| instruction.span.start);
        function_string_indices
            .push(first_instruction.and_then(|span| by_span.get(&span).copied()));
        function_string_maps.push(by_span);
    }
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
            &function_float_maps[function_position],
            function_string_indices[function_position],
            &function_string_maps[function_position],
            &callees,
            &block_parameter_indices[function_position],
            module.item_text(parsed.item),
            parsed.item,
        )?);
    }
    builder.code = BinaryCodeTable {
        functions: code_functions,
    };
    builder.analysis.functions = parsed_functions
        .iter()
        .map(crate::text_analysis_derivation::derive)
        .collect();
    builder.analysis.indirect_call_typeids =
        crate::text_analysis_derivation::indirect_typeids_from_text(module, &parsed_functions)?;
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
        .any(|section| !matches!(section.kind, 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 13 | 14))
    {
        return Err(crate::FormatError::malformed(
            None,
            "tirc dis does not yet support nonempty TIR sections",
        ));
    }
    if module.capabilities(memory_limit)? != crate::BinaryCapabilities::default() {
        return Err(crate::FormatError::malformed(
            None,
            "tirc dis cannot represent nonempty CAPS contents in text",
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
        &globals,
        &strings,
    )?;
    let analysis = module.analysis_summary(memory_limit)?;
    if !crate::text_analysis_derivation::reconstructible_from_code(
        &analysis,
        signatures.records.len(),
        &code,
        &types,
        &signatures,
        &record.name,
    ) || !record.initializers.is_empty()
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
