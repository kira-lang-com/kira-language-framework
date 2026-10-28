//! End-to-end: the KLF Kira phase and package pipeline.

use klf_kir_passes::monomorphize;
use klf_kira_lower::{KiraDefinition, elaborate_file, lower_elaborated, lower_file, parse_unit};
use klf_kira_syntax::parse;
use klf_services::{
    Definition, DefinitionPackage, DefinitionSource, DefinitionUnit, Session, TypeKind,
};
use klf_vm::{Capabilities, Outcome, run_module, run_module_with_capabilities};

#[test]
fn construct_phase_methods_lower_to_executable_kir() {
    let source = r"
construct Calc() extends Language<Tree, Program> {
    function parse(session: Session, unit: Unit) -> Tree {
        return definition_tree(session, unit, 42)
    }
}
";
    let target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target);
    let file = session.add_file("definition.klfd", source);
    let parsed = parse(session.file(file));
    assert!(
        parsed.diagnostics.is_empty(),
        "parse errors: {:?}",
        parsed.diagnostics
    );
    let module = lower_file(
        &mut session,
        "Definition",
        "aarch64-apple-darwin".parse().expect("valid target"),
        &parsed.file,
    )
    .unwrap_or_else(|| panic!("lowering errors: {:?}", session.diagnostics()));
    assert!(
        module
            .functions
            .iter()
            .any(|function| function.name.as_str() == "parse")
    );

    let mut capabilities = Capabilities::new();
    capabilities.register("klfsvc_definition_tree", |_| 7);
    assert_eq!(
        run_module_with_capabilities(
            &module,
            "parse",
            &[session.handle().as_u64(), 0],
            &capabilities,
        )
        .expect("phase method runs"),
        Outcome::Return(Some(7))
    );
}

#[test]
fn package_phase_registers_source_once() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let package = DefinitionPackage::single("t.klfd", "function main() -> Int { return 42 }");
    let mut session = Session::new(target.clone());
    let module = KiraDefinition
        .compile_package(&mut session, &package, "Test", target)
        .unwrap_or_else(|| panic!("package lowering errors: {:?}", session.diagnostics()));

    assert_eq!(session.files().len(), 1);
    assert_eq!(
        run_module(&monomorphize(&module), "main", &[]).expect("module runs"),
        Outcome::Return(Some(42))
    );
}

#[test]
fn package_phase_assembles_multiple_kira_sources() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let package = DefinitionPackage::from_sources(vec![
        DefinitionSource::new("helper.klfd", "function helper() -> Int { return 40 }"),
        DefinitionSource::new(
            "main.klfd",
            "function main() -> Int { return helper() + 2 }",
        ),
    ]);
    let mut session = Session::new(target.clone());
    let module = KiraDefinition
        .compile_package(&mut session, &package, "Test", target)
        .unwrap_or_else(|| panic!("package lowering errors: {:?}", session.diagnostics()));

    assert_eq!(session.files().len(), 2);
    assert_eq!(
        run_module(&monomorphize(&module), "main", &[]).expect("module runs"),
        Outcome::Return(Some(42))
    );
}

#[test]
fn package_phase_resolves_types_across_sources() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let package = DefinitionPackage::from_sources(vec![
        DefinitionSource::new("types.klfd", "struct Point { let x: Int  let y: Int }"),
        DefinitionSource::new(
            "main.klfd",
            "function main() -> Int { var point = Point { x = 40, y = 2 } return point.x + point.y }",
        ),
    ]);
    let mut session = Session::new(target.clone());
    let module = KiraDefinition
        .compile_package(&mut session, &package, "Test", target)
        .expect("package lowering");
    assert_eq!(
        run_module(&monomorphize(&module), "main", &[]).expect("module runs"),
        Outcome::Return(Some(42))
    );
}

#[test]
fn package_phase_accepts_kernel_import_and_rejects_unknown_import() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let accepted = DefinitionPackage::single(
        "main.klfd",
        "import KLF\nfunction main() -> Int { puti(42) return 42 }",
    );
    let mut session = Session::new(target.clone());
    assert!(
        KiraDefinition
            .compile_package(&mut session, &accepted, "Test", target.clone())
            .is_some()
    );

    let rejected = DefinitionPackage::single(
        "main.klfd",
        "import Other.Language\nfunction main() -> Int { return 42 }",
    );
    let mut session = Session::new(target.clone());
    assert!(
        KiraDefinition
            .compile_package(&mut session, &rejected, "Test", target)
            .is_none()
    );
    let diagnostic = session
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code == "KLFK-E0206")
        .expect("unsupported import diagnostic");
    assert_eq!(
        diagnostic.primary_span().expect("import span").file,
        klf_core::FileId::from_u32(0)
    );
}

#[test]
fn definition_phases_parse_elaborate_and_lower() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let mut session = Session::new(target.clone());
    let file = session.add_file("t.klfd", "function main() -> Int { return 42 }");
    let unit = DefinitionUnit::new(file);
    let parsed = parse_unit(&mut session, &unit).expect("parses");
    let elaborated = elaborate_file(&mut session, &parsed.file).expect("elaborates");
    let function_type = elaborated
        .function_type("main")
        .expect("typed function handle");
    assert!(matches!(
        session.type_info(function_type).map(|info| &info.kind),
        Some(TypeKind::Function(_))
    ));
    let module = lower_elaborated(&mut session, "Test", target, &elaborated).expect("lowers");

    assert_eq!(
        run_module(&monomorphize(&module), "main", &[]).expect("module runs"),
        Outcome::Return(Some(42))
    );
}
