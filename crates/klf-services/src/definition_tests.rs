use super::*;

#[test]
fn one_file_units_use_the_inline_representation() {
    let file = FileId::from_u32(7);
    let direct = DefinitionUnit::new(file);
    let from_files = DefinitionUnit::from_files(vec![file]).expect("nonempty unit");

    assert_eq!(direct, from_files);
    assert_eq!(direct.files(), &[file]);
    assert!(matches!(direct.files, DefinitionUnitFiles::Single(_)));
}

#[test]
fn multi_file_units_compare_shared_and_independent_identities() {
    let files = vec![FileId::from_u32(7), FileId::from_u32(11)];
    let original = DefinitionUnit::from_files(files.clone()).expect("nonempty unit");
    let shared = original.clone();
    let independent = DefinitionUnit::from_files(files).expect("nonempty unit");
    let different = DefinitionUnit::from_files(vec![FileId::from_u32(7), FileId::from_u32(12)])
        .expect("nonempty unit");

    assert_eq!(original, shared);
    assert_eq!(original, independent);
    assert_ne!(original, different);
}

struct TestDefinition {
    name: String,
    extensions: Vec<String>,
}

impl Definition for TestDefinition {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn extensions(&self) -> Vec<String> {
        self.extensions.clone()
    }

    fn compile(
        &self,
        _session: &mut Session,
        _source_name: &str,
        _source: &str,
        _module_name: &str,
        _target: Target,
    ) -> Option<Module> {
        None
    }
}

#[test]
fn definition_registry_rejects_duplicate_names_and_extensions() {
    let mut registry = DefinitionRegistry::new();
    registry
        .register(TestDefinition {
            name: "first".to_owned(),
            extensions: vec!["one".to_owned()],
        })
        .expect("first definition registers");

    let duplicate_name = registry.register(TestDefinition {
        name: "first".to_owned(),
        extensions: vec!["two".to_owned()],
    });
    assert!(matches!(
        duplicate_name,
        Err(DefinitionRegistryError::DuplicateName { name }) if name == "first"
    ));

    let duplicate_extension = registry.register(TestDefinition {
        name: "second".to_owned(),
        extensions: vec!["one".to_owned()],
    });
    assert!(matches!(
        duplicate_extension,
        Err(DefinitionRegistryError::DuplicateExtension { extension, first, second })
            if extension == "one" && first == "first" && second == "second"
    ));
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry
            .select("source.one")
            .expect("selected definition")
            .name(),
        "first"
    );
    assert!(registry.select("source.two").is_none());
}
