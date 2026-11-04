use super::*;

#[test]
fn descriptor_metadata_round_trips_modes_and_typed_flag_defaults() {
    let file_modes = vec![
        FileMode {
            name: "legacy\tmode\n\\name".to_owned(),
            pragma: "legacy\npragma".to_owned(),
            flag: "--legacy\tmode".to_owned(),
            extensions: vec![".old".to_owned(), ".legacy\tcpp".to_owned()],
            memory: Some(MemoryModel::Affine),
            convention: Some(DefaultConvention::Ridl),
        },
        FileMode {
            name: "modern".to_owned(),
            pragma: "modern".to_owned(),
            flag: "--modern".to_owned(),
            extensions: Vec::new(),
            memory: Some(MemoryModel::Unmanaged),
            convention: Some(DefaultConvention::Platform),
        },
        FileMode {
            name: "inherited".to_owned(),
            pragma: String::new(),
            flag: String::new(),
            extensions: vec![".inherit".to_owned()],
            memory: None,
            convention: None,
        },
    ];
    let descriptor = LanguageDescriptor::new(
        "test",
        "test",
        "0.1.0",
        vec![".test".to_owned()],
        MemoryModel::Affine,
        Exclusivity::Dynamic,
        DefaultConvention::Ridl,
        "",
    )
    .with_file_modes(file_modes.clone())
    .with_flags(vec![
        DriverFlag {
            name: "warnings".to_owned(),
            default_value: DriverFlagValue::Boolean(false),
        },
        DriverFlag {
            name: "jobs".to_owned(),
            default_value: DriverFlagValue::Integer(4),
        },
        DriverFlag {
            name: "sysroot".to_owned(),
            default_value: DriverFlagValue::String("std\troot".to_owned()),
        },
    ])
    .with_editor(Some(editor_spec()));
    let mut metadata = FrontendMetadata::from_descriptor(&descriptor, "test-target");
    metadata.declare(Hook::Emit);
    metadata.declare(Hook::CompatDriver);
    metadata.declare(Hook::RunProgram);
    metadata.std_root = "/definitions/kira/std".to_owned();

    assert_eq!(metadata.file_modes, file_modes);
    assert_eq!(metadata.memory, MemoryModel::Affine);
    assert_eq!(metadata.exclusivity, Exclusivity::Dynamic);
    assert_eq!(metadata.convention, DefaultConvention::Ridl);
    assert_eq!(metadata.flags, descriptor.flags);
    assert_eq!(FrontendMetadata::decode(&metadata.encode()), metadata);
    let old = FrontendMetadata::decode("name\tlegacy\n");
    assert_eq!(old.memory, MemoryModel::Counted);
    assert_eq!(old.exclusivity, Exclusivity::None);
    assert_eq!(old.convention, DefaultConvention::Platform);
    assert_eq!(old.file_modes, Vec::<FileMode>::new());
    assert_eq!(old.flags, Vec::<DriverFlag>::new());
    assert_eq!(
        old.editor_spec(),
        klf_facts::spec::EditorSpec::minimal("legacy")
    );
}

fn editor_spec() -> klf_facts::spec::EditorSpec {
    use klf_facts::spec::{BracketPair, EnterAction, GrammarRef, KeywordGroup, OnEnterRule};
    use klf_facts::{TokenClass, TokenModifier};
    let mut spec = klf_facts::spec::EditorSpec::minimal("test");
    spec.root_markers = vec!["kira.toml".to_owned()];
    spec.strings[0].interpolation = Some(BracketPair {
        open: "\\(".to_owned(),
        close: ")".to_owned(),
    });
    spec.keywords = vec![KeywordGroup {
        class: TokenClass::Keyword,
        modifiers: vec![TokenModifier::Control, TokenModifier::Contextual],
        words: vec!["if".to_owned(), "else".to_owned()],
    }];
    spec.on_enter = vec![OnEnterRule {
        before: "^\\s*///".to_owned(),
        after: None,
        action: EnterAction::None,
        append: "/// ".to_owned(),
    }];
    spec.region_markers = Some(("// region\t".to_owned(), "// endregion\n".to_owned()));
    spec.grammar = Some(GrammarRef {
        repository: "https://example.org/grammar".to_owned(),
        revision: "abc".to_owned(),
        path: String::new(),
    });
    spec
}
