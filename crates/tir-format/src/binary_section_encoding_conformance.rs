use crate::{
    BinaryAbiStamp, BinaryCapabilities, BinaryLinkage, BinaryMetadata, BinaryMetadataAttachment,
    BinaryModule, BinaryModuleBuilder, BinaryModuleRecord, BinaryProducer, BinaryRidlExport,
    BinaryRidlExportKind, Header, SemanticsMode, assemble_text_module, parse_text_module,
};
use tir_target::{Platform, Profile};

fn empty_builder() -> BinaryModuleBuilder {
    BinaryModuleBuilder::new(
        Header {
            year: 26,
            month: 11,
            week: 1,
            revision: 0,
            flags: 1,
            profile: Profile::App,
            platform: Platform::Portable,
            section_count: 0,
            file_size: 0,
        },
        BinaryModuleRecord {
            name: "Sections".to_owned(),
            producer: Some(BinaryProducer {
                name: "klf".to_owned(),
                version: "26.11.1".to_owned(),
                languages: vec!["kira".to_owned()],
            }),
            semantics: SemanticsMode::Defined,
            attributes: Vec::new(),
            initializers: Vec::new(),
            finalizers: Vec::new(),
            source_hash: None,
        },
    )
}

fn builder_with_function() -> BinaryModuleBuilder {
    let source = "module @Sections tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Sections.simple() -> unit engine(native) {\nbb0:\n  return\n}\n";
    let parsed = parse_text_module(source).expect("fixture parses");
    let bytes = assemble_text_module(&parsed).expect("fixture assembles");
    let binary = BinaryModule::decode(&bytes).expect("fixture binary decodes");
    let limit = u64::MAX;
    let types = binary.type_table(limit).expect("fixture types decode");
    let signatures = binary
        .signature_table(limit, &types)
        .expect("fixture signatures decode");
    let constants = binary
        .constant_table(limit, &types)
        .expect("fixture constants decode");
    let globals = binary
        .global_table(limit, &types, &constants)
        .expect("fixture globals decode");
    let code = binary
        .code_table(limit, &signatures)
        .expect("fixture CODE decodes");
    let mut builder = BinaryModuleBuilder::new(
        binary.header,
        binary.module_record(limit).expect("fixture module decodes"),
    );
    builder.types = types;
    builder.signatures = signatures;
    builder.constants = constants;
    builder.globals = globals;
    builder.code = code;
    builder.analysis = binary
        .analysis_summary(limit)
        .expect("fixture ANALYSIS decodes");
    builder
}

#[test]
fn caps_is_always_emitted_in_canonical_section_order() {
    let builder = empty_builder();
    let bytes = builder.encode().expect("minimal section set encodes");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let kinds: Vec<_> = binary.sections.iter().map(|section| section.kind).collect();
    assert_eq!(kinds, [1, 2, 13, 14]);
    assert_eq!(
        binary.capabilities(u64::MAX).expect("CAPS parses"),
        BinaryCapabilities::default()
    );
    assert_eq!(
        binary
            .sections
            .iter()
            .find(|section| section.kind == 13)
            .expect("CAPS exists")
            .stored_bytes(),
        &[0, 0, 0, 0, 0]
    );
    assert_eq!(builder.encode().expect("stable encoder"), bytes);
}

#[test]
fn requested_entitlements_are_interned_and_round_trip() {
    let mut builder = empty_builder();
    builder.capabilities.requested_entitlements = vec![
        "com.example.first".to_owned(),
        "com.example.second".to_owned(),
    ];
    let binary =
        BinaryModule::decode(&builder.encode().expect("CAPS encodes")).expect("binary decodes");
    assert_eq!(
        binary.capabilities(u64::MAX).expect("CAPS decodes"),
        builder.capabilities
    );
    assert!(
        binary
            .string_table(u64::MAX)
            .expect("STRINGS decodes")
            .index_of("com.example.first")
            .is_some()
    );
}

#[test]
fn metadata_instruction_attachment_round_trips_in_builder() {
    let mut builder = builder_with_function();
    builder.metadata.attachments.push(BinaryMetadataAttachment {
        function_index: 0,
        instruction_ordinal: 0,
        metadata: BinaryMetadata::ProfileId(7),
    });
    let binary =
        BinaryModule::decode(&builder.encode().expect("metadata encodes")).expect("binary decodes");
    let kinds = binary
        .sections
        .iter()
        .map(|section| section.kind)
        .collect::<Vec<_>>();
    assert!(kinds.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        binary.metadata_table(u64::MAX).expect("metadata decodes"),
        builder.metadata
    );
    assert!(kinds.contains(&13));
    assert!(kinds.contains(&15));
}

#[test]
fn metadata_rejects_unknown_instruction_ordinals_and_functions() {
    let mut builder = builder_with_function();
    builder.metadata.attachments.push(BinaryMetadataAttachment {
        function_index: 0,
        instruction_ordinal: 1,
        metadata: BinaryMetadata::ProfileId(3),
    });
    assert!(
        builder.encode().is_err(),
        "out-of-range instruction ordinal emitted"
    );
    builder.metadata.attachments[0].instruction_ordinal = 0;
    builder.metadata.attachments[0].function_index = 99;
    assert!(builder.encode().is_err(), "unknown CODE function emitted");
}

#[test]
fn ridl_exports_and_abi_stamps_encode_in_canonical_order() {
    let mut builder = builder_with_function();
    builder.signatures.records[0].linkage = BinaryLinkage::Exported;
    let uuid = [0x42; 16];
    builder.ridl.exports.push(BinaryRidlExport {
        symbol: "Sections.simple".to_owned(),
        kind: BinaryRidlExportKind::Function,
        entity_index: 0,
        abi_uuid: uuid,
    });
    builder.ridl.abi_stamps.push(BinaryAbiStamp {
        symbol: "Sections.simple".to_owned(),
        uuid,
    });
    builder.ridl.canonical_hash = Some([0x85; 32]);
    let encoded = builder.encode().expect("RIDL export encodes");
    let module = BinaryModule::decode(&encoded).expect("RIDL module decodes");
    let kinds = module
        .sections
        .iter()
        .map(|section| section.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&12));
    assert!(kinds.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        module.ridl_table(u64::MAX).expect("RIDL decodes"),
        builder.ridl
    );
    assert_eq!(builder.encode().expect("stable RIDL encoding"), encoded);
}

#[test]
fn ridl_rejects_bad_uuids_order_and_undefined_functions() {
    let mut builder = builder_with_function();
    builder.ridl.exports.push(BinaryRidlExport {
        symbol: "Sections.simple".to_owned(),
        kind: BinaryRidlExportKind::Function,
        entity_index: 0,
        abi_uuid: [0; 16],
    });
    builder.ridl.canonical_hash = Some([4; 32]);
    assert!(builder.encode().is_err(), "zero UUID emitted");
    builder.ridl.exports[0].abi_uuid = [1; 16];
    builder.ridl.exports[0].entity_index = 42;
    assert!(
        builder.encode().is_err(),
        "unknown exported function emitted"
    );
    builder.ridl.exports[0].entity_index = 0;
    builder.ridl.abi_stamps.push(BinaryAbiStamp {
        symbol: "Sections.simple".to_owned(),
        uuid: [2; 16],
    });
    assert!(builder.encode().is_err(), "inconsistent ABI stamp emitted");
}

#[test]
fn builder_computes_all_summary_bits_from_section_records() {
    use crate::{BinaryEngine, BinaryFunctionAnalysis, BinaryTarget};

    let mut builder = builder_with_function();
    builder.header.flags = 0x3f;
    builder.signatures.records[0].engine = Some(BinaryEngine::Runtime);
    builder.signatures.records[0].target = BinaryTarget::Both;
    builder.analysis.functions[0] = BinaryFunctionAnalysis {
        trust_class: 1,
        reason_mask: 0,
    };
    let binary = BinaryModule::decode(&builder.encode().expect("summary flags encode"))
        .expect("binary decodes");
    assert_eq!(binary.header.flags & 0x7f, 0b111_0011);
    builder.analysis.functions[0].trust_class = 2;
    let binary = BinaryModule::decode(&builder.encode().expect("summary flags reencode"))
        .expect("binary decodes");
    assert_eq!(binary.header.flags & 0x7f, 0b101_0111);
}

fn debug_fixture() -> crate::BinaryDebugTable {
    use crate::{
        DebugCompileUnit, DebugFileRecord, DebugLabel, DebugLocation, DebugScope, DebugScopeKind,
        DebugSourceLocation, DebugSourceType, DebugTypeMember, DebugVariable,
    };
    let source = DebugSourceLocation {
        file_index: 0,
        line: 3,
        column: 1,
    };
    crate::BinaryDebugTable {
        files: vec![DebugFileRecord {
            path: "src/generated.kira".to_owned(),
            content_hash: Some([0x42; 32]),
            generated_text: Some("fn main() {}".to_owned()),
        }],
        compile_units: vec![DebugCompileUnit {
            language: "kira".to_owned(),
            producer: "klf".to_owned(),
        }],
        source_types: vec![
            DebugSourceType::Base {
                name: "i32".to_owned(),
                encoding: 5,
                bit_width: 32,
            },
            DebugSourceType::Composite {
                name: "Pair".to_owned(),
                kind: 0,
                size: 8,
                alignment: 4,
                members: vec![DebugTypeMember {
                    name: "first".to_owned(),
                    type_index: 0,
                    byte_offset: 0,
                }],
            },
            DebugSourceType::Pointer { pointee_type: 1 },
            DebugSourceType::Array {
                element_type: 0,
                count: Some(4),
            },
            DebugSourceType::Qualified {
                type_index: 2,
                qualifiers: 1,
            },
        ],
        scopes: vec![
            DebugScope {
                kind: DebugScopeKind::Function,
                function_index: 0,
                compile_unit: 0,
                parent: None,
            },
            DebugScope {
                kind: DebugScopeKind::Lexical,
                function_index: 0,
                compile_unit: 0,
                parent: Some(0),
            },
            DebugScope {
                kind: DebugScopeKind::Inlined { call_site: source },
                function_index: 0,
                compile_unit: 0,
                parent: Some(1),
            },
        ],
        locations: vec![DebugLocation {
            function_index: 0,
            instruction_ordinal: 0,
            source,
            scope: Some(1),
        }],
        variables: vec![DebugVariable {
            name: "count".to_owned(),
            source_type: 0,
            scope: 1,
        }],
        labels: vec![DebugLabel {
            name: "entry".to_owned(),
            scope: 0,
            source,
        }],
        data: Vec::new(),
    }
}

#[test]
fn debug_eight_tables_encode_and_decode_with_canonical_strings() {
    let mut builder = builder_with_function();
    builder.debug = debug_fixture();
    let encoded = builder.encode().expect("DEBUG section encodes");
    let binary = BinaryModule::decode(&encoded).expect("binary decodes");
    assert_eq!(binary.sections.last().expect("last section").kind, 17);
    assert_eq!(
        binary.debug_table(u64::MAX).expect("DEBUG decodes"),
        Some(builder.debug.clone())
    );
    assert_eq!(encoded, builder.encode().expect("DEBUG is byte-stable"));
    let strings = binary.string_table(u64::MAX).expect("STRINGS decodes");
    for name in [
        "src/generated.kira",
        "i32",
        "Pair",
        "first",
        "count",
        "entry",
    ] {
        assert!(
            strings.index_of(name).is_some(),
            "DEBUG string was not interned: {name}"
        );
    }
}

#[test]
fn debug_rejects_invalid_scopes_locations_and_source_types() {
    let mut builder = builder_with_function();
    builder.debug = debug_fixture();
    builder.debug.scopes[1].parent = Some(1);
    assert!(builder.encode().is_err(), "self-parented scope was encoded");
    builder.debug.scopes[1].parent = Some(0);
    builder.debug.locations[0].instruction_ordinal = 1;
    assert!(
        builder.encode().is_err(),
        "out-of-range instruction location was encoded"
    );
    builder.debug.locations[0].instruction_ordinal = 0;
    builder.debug.source_types[1] = crate::DebugSourceType::Composite {
        name: "Pair".to_owned(),
        kind: 0,
        size: 8,
        alignment: 3,
        members: Vec::new(),
    };
    assert!(
        builder.encode().is_err(),
        "non-power-of-two alignment was encoded"
    );
}

#[test]
fn debug_enums_forward_references_and_data_record_rejections() {
    let mut builder = builder_with_function();
    let mut debug = debug_fixture();
    debug.source_types.push(crate::DebugSourceType::Enum {
        name: "Direction".to_owned(),
        underlying_type: 0,
        variants: vec![crate::DebugEnumerator {
            name: "north".to_owned(),
            value: crate::leb::IntegerImmediate {
                negative: false,
                magnitude: 1,
            },
        }],
    });
    debug.source_types.push(crate::DebugSourceType::Typedef {
        name: "Next".to_owned(),
        type_index: 7,
    });
    debug.source_types.push(crate::DebugSourceType::Reference {
        pointee_type: 6,
        mutable: true,
    });
    builder.debug = debug;
    let encoded = builder.encode().expect("forward-reference DEBUG encodes");
    let binary = BinaryModule::decode(&encoded).expect("binary decodes");
    assert_eq!(
        binary.debug_table(u64::MAX).expect("DEBUG decodes"),
        Some(builder.debug.clone())
    );

    builder.debug.data.push(crate::DebugDataRecord {
        data_index: 0,
        source_type: 0,
        source: crate::DebugSourceLocation {
            file_index: 0,
            line: 5,
            column: 1,
        },
    });
    assert!(
        builder.encode().is_err(),
        "DEBUG DATA references missing GLOBALS"
    );
}

#[test]
fn multiple_custom_sections_preserve_private_bytes_in_order() {
    let mut builder = empty_builder();
    builder.custom_sections = vec![vec![0x00, 0xff, 0x17], Vec::new(), vec![0x81, 0x80, 0x00]];
    let encoded = builder.encode().expect("CUSTOM sections encode");
    let binary = BinaryModule::decode(&encoded).expect("CUSTOM sections decode");
    let custom = binary
        .sections
        .iter()
        .filter(|section| section.kind == 255)
        .map(|section| section.stored_bytes().to_vec())
        .collect::<Vec<_>>();
    assert_eq!(custom, builder.custom_sections);
    assert_eq!(
        builder.encode().expect("CUSTOM encoding is deterministic"),
        encoded
    );
}
