use super::*;

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
