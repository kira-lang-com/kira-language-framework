//! End-to-end: the KLF Kira phase and package pipeline.

use klf_kir_passes::monomorphize;
use klf_kira_lower::{KiraDefinition, elaborate_file, lower_elaborated, lower_file, parse_unit};
use klf_kira_syntax::parse;
use klf_services::{
    Definition, DefinitionPackage, DefinitionSource, DefinitionUnit, QueryDependency, QueryPhase,
    Session, TypeKind,
};
use klf_vm::{Capabilities, Outcome, run_module, run_module_with_capabilities};
use std::sync::Arc;

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
fn package_noop_build_reuses_lowered_output() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let package = DefinitionPackage::single("main.klfd", "function main() -> Int { return 42 }");
    let mut session = Session::new(target.clone());
    let first = KiraDefinition
        .compile_package(&mut session, &package, "Test", target.clone())
        .expect("initial build");
    let before_noop = session.query_stats();
    let second = KiraDefinition
        .compile_package(&mut session, &package, "Test", target.clone())
        .expect("no-op build");
    let after_noop = session.query_stats();

    assert_eq!(first, second);
    assert_eq!(after_noop.executions, before_noop.executions);
    assert_eq!(after_noop.cache_hits - before_noop.cache_hits, 3);

    let before_renamed = session.query_stats();
    let renamed = KiraDefinition
        .compile_package(&mut session, &package, "Renamed", target)
        .expect("build with another module name");
    let after_renamed = session.query_stats();
    assert_eq!(after_renamed.executions - before_renamed.executions, 2);
    assert_eq!(renamed.name.as_str(), "Renamed");
}

#[test]
fn lowered_modules_are_cached_per_target() {
    let session_target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid session target");
    let other_target = "x86_64-unknown-linux-gnu"
        .parse::<klf_target::Target>()
        .expect("valid compilation target");
    let package = DefinitionPackage::single("main.klfd", "function main() -> Int { return 42 }");
    let mut session = Session::new(session_target.clone());
    let unit = KiraDefinition
        .units(&mut session, &package)
        .expect("one source unit")
        .pop()
        .expect("unit");
    let parsed = KiraDefinition
        .parse(&mut session, unit.clone())
        .expect("source parses");
    let elaborated = KiraDefinition
        .elaborate(&mut session, unit.clone(), &parsed)
        .expect("source checks");
    let first = KiraDefinition
        .lower_shared(
            &mut session,
            unit.clone(),
            &elaborated,
            "Targeted",
            session_target.clone(),
        )
        .expect("first target lowers");
    let second = KiraDefinition
        .lower_shared(
            &mut session,
            unit,
            &elaborated,
            "Targeted",
            other_target.clone(),
        )
        .expect("second target lowers");

    assert_eq!(first.target, session_target);
    assert_eq!(second.target, other_target);
    assert!(!Arc::ptr_eq(&first, &second));
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
    let elaborate = session
        .query_records()
        .iter()
        .find(|query| query.phase == QueryPhase::Elaborate)
        .expect("elaborate query");
    assert_eq!(elaborate.dependencies.len(), 1);
    assert!(matches!(
        elaborate.dependencies[0],
        QueryDependency::Query(_)
    ));
    let lower = session
        .query_records()
        .iter()
        .find(|query| query.phase == QueryPhase::Lower)
        .expect("lower query");
    assert_eq!(lower.dependencies.len(), 3);
    assert!(
        lower
            .dependencies
            .iter()
            .all(|dependency| matches!(dependency, QueryDependency::Query(_)))
    );
    assert_eq!(
        run_module(&monomorphize(&module), "main", &[]).expect("module runs"),
        Outcome::Return(Some(42))
    );
}

#[test]
fn package_parse_reuses_unchanged_source_queries_after_an_edit() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let package = DefinitionPackage::from_sources(vec![
        DefinitionSource::new("first.klfd", "function first() -> Int { return 1 }"),
        DefinitionSource::new("second.klfd", "function second() -> Int { return 2 }"),
    ]);
    let mut session = Session::new(target);
    let unit = KiraDefinition
        .units(&mut session, &package)
        .expect("package unit")
        .remove(0);
    KiraDefinition
        .parse(&mut session, unit.clone())
        .expect("initial parse");
    let first = session.query_stats();
    assert_eq!(first.executions, 3);

    let edited = DefinitionPackage::from_sources(vec![
        DefinitionSource::new("first.klfd", "function first() -> Int { return 3 }"),
        DefinitionSource::new("second.klfd", "function second() -> Int { return 2 }"),
    ]);
    let edited_unit = KiraDefinition
        .units(&mut session, &edited)
        .expect("updated package unit")
        .remove(0);
    assert_eq!(edited_unit, unit);
    KiraDefinition
        .parse(&mut session, edited_unit)
        .expect("incremental parse");
    let second = session.query_stats();
    assert_eq!(second.executions - first.executions, 2);
    assert_eq!(second.cache_hits - first.cache_hits, 1);
    assert_eq!(second.invalidated - first.invalidated, 2);
}

#[test]
fn package_parse_shares_file_ast_nodes_with_source_queries() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let package = DefinitionPackage::from_sources(vec![
        DefinitionSource::new("first.klfd", "function first() -> Int { return 1 }"),
        DefinitionSource::new("second.klfd", "function second() -> Int { return 2 }"),
    ]);
    let mut session = Session::new(target);
    let unit = KiraDefinition
        .units(&mut session, &package)
        .expect("package unit")
        .remove(0);
    let package_artifact = KiraDefinition
        .parse(&mut session, unit.clone())
        .expect("package parse");
    let package_decl = {
        let package_parsed = package_artifact
            .downcast_ref::<klf_kira_syntax::Parsed>(&session)
            .expect("package parsed artifact");
        Arc::clone(&package_parsed.file.decls[0])
    };
    let source_unit = DefinitionUnit::new(unit.files()[0]);
    let source_artifact = session
        .cached_artifact("klfkira", &source_unit, QueryPhase::Parse)
        .expect("cached source parse");
    let source_parsed = source_artifact
        .downcast_arc::<klf_kira_syntax::Parsed>(&session)
        .expect("shared source parse");

    assert!(Arc::ptr_eq(&package_decl, &source_parsed.file.decls[0]));
}

#[test]
fn package_parse_replays_cached_errors_and_clears_them_after_a_valid_edit() {
    let target = "aarch64-apple-darwin"
        .parse::<klf_target::Target>()
        .expect("valid target");
    let broken = DefinitionPackage::single("main.klfd", "function main( -> Int { return 42 }");
    let mut session = Session::new(target);
    let unit = KiraDefinition
        .units(&mut session, &broken)
        .expect("package unit")
        .remove(0);

    assert!(KiraDefinition.parse(&mut session, unit.clone()).is_none());
    let first_count = session.diagnostics().len();
    assert!(first_count > 0);
    assert!(KiraDefinition.parse(&mut session, unit.clone()).is_none());
    assert_eq!(session.diagnostics().len(), first_count);

    let fixed = DefinitionPackage::single("main.klfd", "function main() -> Int { return 42 }");
    let fixed_unit = KiraDefinition
        .units(&mut session, &fixed)
        .expect("updated package unit")
        .remove(0);
    assert_eq!(fixed_unit, unit);
    assert!(KiraDefinition.parse(&mut session, fixed_unit).is_some());
    assert!(session.diagnostics().is_empty());
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
