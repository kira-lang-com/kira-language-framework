use klf_services::{
    Choice, DefinitionArtifact, DefinitionHandleError, DefinitionPackage, DefinitionPhaseArguments,
    DefinitionSource, DefinitionUnit, DiagnosticSpec, Exclusivity, KERNEL_SERVICE_MANIFEST,
    KernelServiceManifest, LayoutRule, Namespace, Pairing, Policy, QueryDependency, QueryPhase,
    Requirement, ServiceType, Session, Solution, TypeKind, TypeLayout,
};
use klf_source::Span;
use klf_target::Target;

fn target() -> Target {
    "aarch64-apple-darwin".parse().expect("valid target")
}

#[test]
fn definition_package_exposes_one_phase_input() {
    let package = DefinitionPackage::from_sources(vec![DefinitionSource::new("main.klfd", "body")]);
    assert_eq!(package.sources().len(), 1);
    assert_eq!(package.sources()[0].name, "main.klfd");
    assert_eq!(&*package.sources()[0].text, "body");
}

#[test]
fn session_reports_an_error_with_a_primary_span() {
    let mut session = Session::new(target());
    let file = session.add_file("input.kira", "abc");
    let span = Span::new(file, 1, 2);
    session
        .set_file_exclusivity(file, Exclusivity::Dynamic)
        .expect("registered source file");

    session.report_error("KLF-TEST-0001", span, "invalid byte");

    assert_eq!(session.file_exclusivity(file), Some(Exclusivity::Dynamic));
    assert_eq!(session.error_count(), 1);
    assert_eq!(session.diagnostics()[0].primary_span(), Some(span));
    assert_eq!(session.diagnostics()[0].message, "invalid byte");
}

#[test]
fn session_registers_runtime_diagnostic_specs_and_rolls_them_back() {
    let mut session = Session::new(target());
    let spec = DiagnosticSpec::new(
        2001,
        klf_diag::Severity::Error,
        "invalid token",
        "the token is not valid here",
    );
    assert!(session.register_diagnostic(spec.clone()));
    assert!(!session.register_diagnostic(spec));
    assert_eq!(
        session.diagnostic_spec(2001).expect("spec").title,
        "invalid token"
    );

    assert!(
        session
            .speculate(|session| {
                assert!(session.register_diagnostic(DiagnosticSpec::new(
                    2002,
                    klf_diag::Severity::Warning,
                    "temporary",
                    "temporary diagnostic",
                )));
                None::<()>
            })
            .is_none()
    );
    assert!(session.diagnostic_spec(2002).is_none());
}

#[test]
fn session_resolves_runtime_diagnostics_through_registered_specs() {
    let mut session = Session::new(target());
    let file = session.add_file("input.kira", "abcdef");
    session.register_diagnostic(DiagnosticSpec::new(
        2001,
        klf_diag::Severity::Error,
        "invalid token",
        "the token is not valid here",
    ));

    assert!(session.report_runtime_diagnostic(file, klf_diag::RuntimeDiagnostic::new(2001, 1, 3),));
    let report = &session.runtime_diagnostics()[0];
    assert_eq!(report.code, 2001);
    assert_eq!(report.severity, klf_diag::Severity::Error);
    assert_eq!(report.title, "invalid token");
    assert_eq!(report.explanation, "the token is not valid here");
    assert_eq!(report.span, Span::new(file, 1, 3));
    assert!(
        !session.report_runtime_diagnostic(file, klf_diag::RuntimeDiagnostic::new(2999, 0, 1),)
    );
}

#[test]
fn session_caches_and_invalidates_phase_artifacts() {
    let mut session = Session::new(target());
    let unit = DefinitionUnit::new(session.add_file("input.kira", "body"));
    let artifact = DefinitionArtifact::new(&mut session, unit.clone(), 42_u32);

    session.cache_artifact("test", QueryPhase::Parse, &artifact);
    let cached = session
        .cached_artifact("test", &unit, QueryPhase::Parse)
        .expect("cached artifact");
    assert_eq!(cached.id(), artifact.id());
    session.invalidate_queries();
    assert!(
        session
            .cached_artifact("test", &unit, QueryPhase::Parse)
            .is_none()
    );
}

#[test]
fn session_tracks_query_dependencies_and_file_invalidation() {
    let mut session = Session::new(target());
    let file = session.add_file("input.kira", "body");
    let unit = DefinitionUnit::new(file);
    let parse_id = session.begin_query("test", &unit, QueryPhase::Parse);
    let parsed = DefinitionArtifact::new(&mut session, unit.clone(), 42_u32);
    session.cache_artifact("test", QueryPhase::Parse, &parsed);
    session.finish_query(parse_id);

    let elaborate_id = session.begin_query("test", &unit, QueryPhase::Elaborate);
    assert!(session.record_query_dependency("test", &unit, QueryPhase::Parse));
    let elaborated = DefinitionArtifact::new(&mut session, unit.clone(), 7_u32);
    session.cache_artifact("test", QueryPhase::Elaborate, &elaborated);
    session.finish_query(elaborate_id);

    let elaborate_record = session.query_record(elaborate_id).expect("elaborate query");
    assert!(elaborate_record.valid);
    assert!(
        elaborate_record
            .dependencies
            .contains(&QueryDependency::File(file))
    );
    assert!(
        elaborate_record
            .dependencies
            .contains(&QueryDependency::Query(parse_id))
    );

    session.invalidate_file(file);
    assert!(!session.query_record(parse_id).expect("parse query").valid);
    assert!(
        !session
            .query_record(elaborate_id)
            .expect("elaborate query")
            .valid
    );
    assert!(
        session
            .cached_artifact("test", &unit, QueryPhase::Parse)
            .is_none()
    );
}

#[test]
fn kernel_service_manifest_is_typed_and_rejects_duplicates() {
    let session = Session::new(target());
    assert_eq!(
        session
            .kernel_service("klf.io.put")
            .expect("session service")
            .symbol,
        "klfsvc_io_put"
    );
    let manifest = KernelServiceManifest::parse(KERNEL_SERVICE_MANIFEST).expect("valid manifest");
    assert!(manifest.services().len() >= 8);
    let put = manifest.service("klf.io.put").expect("put service");
    assert_eq!(put.symbol, "klfsvc_io_put");
    assert_eq!(
        put.signature.parameters,
        vec![ServiceType::Integer {
            bits: 32,
            signed: true
        }]
    );
    assert_eq!(
        put.signature.result,
        ServiceType::Integer {
            bits: 32,
            signed: true
        }
    );

    let duplicate = "one symbol (i32) -> void\ntwo symbol (i32) -> void";
    let error = KernelServiceManifest::parse(duplicate).expect_err("duplicate symbol");
    assert_eq!(error.line, 2);
}

#[test]
fn session_scopes_resolve_nearest_declarations_and_imports() {
    let mut session = Session::new(target());
    let root_kind = session.intern("root");
    let root = session.scope(None, root_kind);
    let child_kind = session.intern("child");
    let child = session.scope(Some(root), child_kind);
    let module_kind = session.intern("module");
    let module = session.scope(None, module_kind);
    let span = Span::empty(klf_core::FileId::from_u32(0), 0);
    let function_kind = session.intern("function");
    let root_entity = session.entity(1, function_kind.clone(), span);
    let child_entity = session.entity(2, function_kind.clone(), span);
    let module_entity = session.entity(3, function_kind.clone(), span);
    let name = session.intern("run");
    let outer_name = session.intern("outer");

    assert!(session.declare(root, name.clone(), Namespace::Value, root_entity));
    assert!(session.declare(root, outer_name.clone(), Namespace::Value, root_entity));
    assert!(session.declare(child, name.clone(), Namespace::Value, child_entity));
    assert!(session.declare(module, name.clone(), Namespace::Value, module_entity));
    assert!(!session.declare(child, name.clone(), Namespace::Value, root_entity));
    assert_eq!(session.scope_kind(child).as_str(), "child");
    assert_eq!(
        session.lookup_local(child, &name, &Namespace::Value),
        vec![child_entity]
    );
    assert_eq!(
        session.lookup(child, &name, &Namespace::Value),
        vec![child_entity]
    );
    assert_eq!(
        session.lookup(child, &outer_name, &Namespace::Value),
        vec![root_entity]
    );

    let imported = session.intern("imported");
    assert!(session.declare(module, imported.clone(), Namespace::Value, module_entity));
    let second_module_kind = session.intern("second-module");
    let second_module = session.scope(None, second_module_kind);
    let second_module_entity = session.entity(4, function_kind, span);
    assert!(session.declare(
        second_module,
        imported.clone(),
        Namespace::Value,
        second_module_entity
    ));
    session.import_edge(child, module, false);
    session.import_edge(child, second_module, false);
    assert_eq!(
        session.lookup(child, &imported, &Namespace::Value),
        vec![module_entity, second_module_entity]
    );
    assert_eq!(
        session
            .entity_data(module_entity)
            .map(|entity| entity.payload),
        Some(3)
    );
}

#[test]
fn session_type_services_build_nominals_and_compute_layout() {
    let mut session = Session::new(target());
    let int = session.int_type(64, true);
    let byte = session.int_type(8, false);
    let point_name = session.intern("Point");
    let point = session.nominal_type(
        point_name,
        TypeLayout::Struct {
            fields: vec![int, byte],
            rule: LayoutRule::Ridl,
        },
        Policy {
            copy: klf_services::CopyPolicy::Trivial,
            destroy: klf_services::DestroyPolicy::Trivial,
            relocate: klf_services::RelocatePolicy::Bitwise,
            exclusivity: klf_kir::Exclusivity::None,
            pairing: Pairing::None,
        },
    );

    assert!(matches!(
        session.type_info(int).map(|info| &info.kind),
        Some(TypeKind::Integer {
            bits: 64,
            signed: true
        })
    ));
    assert_eq!(session.size_of(point), Some(16));
    assert_eq!(session.align_of(point), Some(8));
    assert!(session.type_policy(point).is_some());

    let opaque_name = session.intern("Opaque");
    let opaque = session.opaque_nominal(opaque_name);
    assert_eq!(session.size_of(opaque), None);
    session.complete_nominal(
        opaque,
        TypeLayout::Explicit { size: 4, align: 4 },
        Policy {
            copy: klf_services::CopyPolicy::None,
            destroy: klf_services::DestroyPolicy::Trivial,
            relocate: klf_services::RelocatePolicy::Bitwise,
            exclusivity: klf_kir::Exclusivity::None,
            pairing: Pairing::None,
        },
    );
    assert_eq!(session.size_of(opaque), Some(4));
}

#[test]
fn session_traits_and_solver_resolve_scored_choices() {
    let mut session = Session::new(target());
    let int32 = session.int_type(32, true);
    let int64 = session.int_type(64, true);
    let equality_name = session.intern("Equatable");
    let equals_name = session.intern("equals");
    let equality = session.declare_trait(
        equality_name,
        vec![Requirement { name: equals_name }],
        Vec::new(),
    );
    let ordered_name = session.intern("Ordered");
    let ordered = session.declare_trait(ordered_name, Vec::new(), vec![equality]);
    let witness = session.intern("int64_equals");
    session.conformance(int64, ordered, vec![witness], vec![]);
    assert!(session.conforms(int64, ordered));
    assert!(session.conforms(int64, equality));
    assert_eq!(
        session.conformance_info(int64, ordered).unwrap().associated,
        Vec::<klf_services::TypeId>::new()
    );

    let origin = Span::empty(klf_core::FileId::from_u32(0), 0);
    let solution = {
        let mut solver = session.solver();
        let variable = solver.variable(origin);
        solver.disjunction(
            vec![
                Choice {
                    equalities: vec![(variable, int32)],
                    conformances: Vec::new(),
                    cost: 4,
                },
                Choice {
                    equalities: vec![(variable, int64)],
                    conformances: vec![(int64, equality)],
                    cost: 1,
                },
            ],
            origin,
        );
        solver.solve(8)
    };

    match solution {
        Solution::Unique(bindings) => {
            assert_eq!(bindings.len(), 1);
            assert_eq!(bindings[0].value, int64);
        }
        other => panic!("expected unique scored solution, got {other:?}"),
    }
}

#[test]
fn session_speculation_rolls_back_nested_state_and_arena_writes() {
    let mut session = Session::new(target());
    let file = session.add_file("base.kira", "base");
    let values_name = session.intern("values");
    let values = session.arena::<u32>(values_name);
    let value = values.alloc(&mut session, 1);
    let origin = Span::empty(file, 0);

    let result = session.speculate::<(), _>(|session| {
        let temporary_file = session.add_file("temporary.kira", "temporary");
        session
            .set_file_exclusivity(temporary_file, Exclusivity::Static)
            .expect("registered source file");
        let temporary_name = session.intern("temporary");
        let temporary = session.arena::<u16>(temporary_name.clone());
        temporary.alloc(session, 9);
        values.set(session, value, 2);
        session.report_error("KLF-TEST-SPEC", origin, "tentative");
        let inner = session.speculate::<(), _>(|session| {
            values.set(session, value, 3);
            Some(())
        });
        assert_eq!(inner, Some(()));
        values.set(session, value, 4);
        assert_eq!(temporary.count(session), 1);
        None
    });

    assert_eq!(result, None);
    assert_eq!(session.files().len(), 1);
    let after_rollback = session.add_file("after-rollback.kira", "after");
    assert_eq!(session.file_exclusivity(after_rollback), None);
    assert_eq!(values.count(&session), 1);
    assert_eq!(values.get(&session, value), Some(&1));
    assert_eq!(session.error_count(), 0);
    let temporary_name = session.intern("temporary");
    let temporary = session.arena::<u16>(temporary_name);
    assert_eq!(temporary.count(&session), 0);

    let committed = session.speculate::<u32, _>(|session| {
        values.set(session, value, 7);
        Some(42)
    });
    assert_eq!(committed, Some(42));
    assert_eq!(values.get(&session, value), Some(&7));
}

#[test]
fn session_rejects_artifacts_from_other_sessions() {
    let mut current = Session::new(target());
    let file = current.add_file("current.kira", "current");
    let unit = DefinitionUnit::new(file);
    current.register_unit(unit.clone());
    let _local = DefinitionArtifact::new(&mut current, unit.clone(), ());

    let mut foreign = Session::new(target());
    let foreign_file = foreign.add_file("foreign.kira", "foreign");
    let foreign_unit = DefinitionUnit::new(foreign_file);
    foreign.register_unit(foreign_unit.clone());
    let foreign_tree = DefinitionArtifact::new(&mut foreign, foreign_unit, ());
    let arguments = DefinitionPhaseArguments::Elaborate {
        unit,
        tree: foreign_tree,
    };

    assert!(matches!(
        arguments.marshal(&current),
        Err(DefinitionHandleError::ArtifactNotRegistered { .. })
    ));
}
