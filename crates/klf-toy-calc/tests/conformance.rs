//! The toy calculator's conformance suite (the M4 exit test): every corpus
//! program compiles through the definition and runs on every backend with the
//! oracle result. The native backend runs only when the provisioned toolchain
//! is installed; the VM always runs.

use klf_driver::{Backend, compile, compile_package, run, toolchain_available};
use klf_services::{DefinitionPackage, QueryDependency, QueryPhase, Session};
use klf_toy_calc::CalcDefinition;
use klf_vm::Outcome;

struct Case {
    file: &'static str,
    entry: &'static str,
    args: Vec<u64>,
    expected: Outcome,
}

fn ret(value: u64) -> Outcome {
    Outcome::Return(Some(value))
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            file: "arith.calc",
            entry: "add",
            args: vec![40, 2],
            expected: ret(42),
        },
        Case {
            file: "arith.calc",
            entry: "poly",
            args: vec![3],
            expected: ret(16),
        },
        Case {
            file: "arith.calc",
            entry: "main",
            args: vec![],
            expected: ret(26),
        },
        Case {
            file: "control.calc",
            entry: "abs",
            args: vec![(-5i64).cast_unsigned()],
            expected: ret(5),
        },
        Case {
            file: "control.calc",
            entry: "max",
            args: vec![3, 9],
            expected: ret(9),
        },
        Case {
            file: "control.calc",
            entry: "clamp",
            args: vec![15, 0, 10],
            expected: ret(10),
        },
        Case {
            file: "control.calc",
            entry: "clamp",
            args: vec![(-4i64).cast_unsigned(), 0, 10],
            expected: ret(0),
        },
        Case {
            file: "control.calc",
            entry: "clamp",
            args: vec![7, 0, 10],
            expected: ret(7),
        },
        Case {
            file: "recur.calc",
            entry: "fact",
            args: vec![5],
            expected: ret(120),
        },
        Case {
            file: "recur.calc",
            entry: "fib",
            args: vec![10],
            expected: ret(55),
        },
    ]
}

fn source(file: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn compiled(file: &str) -> klf_kir::Module {
    let target: klf_target::Target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target.clone());
    let module = compile(
        &CalcDefinition,
        &mut session,
        file,
        &source(file),
        "Calc",
        target,
    );
    module.unwrap_or_else(|| panic!("compile {file}: {:?}", session.diagnostics()))
}

#[test]
fn corpus_runs_on_vm() {
    for case in cases() {
        let module = compiled(case.file);
        let outcome = run(&module, case.entry, &case.args, Backend::Vm)
            .unwrap_or_else(|e| panic!("VM {}::{}: {e}", case.file, case.entry));
        assert_eq!(
            outcome, case.expected,
            "VM {}::{} {:?}",
            case.file, case.entry, case.args
        );
    }
}

#[test]
fn corpus_runs_on_native() {
    if !toolchain_available() {
        eprintln!("skipping native conformance: provisioned toolchain not installed");
        return;
    }
    for case in cases() {
        let module = compiled(case.file);
        let outcome = run(&module, case.entry, &case.args, Backend::Native)
            .unwrap_or_else(|e| panic!("native {}::{}: {e}", case.file, case.entry));
        assert_eq!(
            outcome, case.expected,
            "native {}::{} {:?}",
            case.file, case.entry, case.args
        );
    }
}

#[test]
fn package_compilation_tracks_phase_queries() {
    let target: klf_target::Target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target.clone());
    let package = DefinitionPackage::single("arith.calc", source("arith.calc"));
    compile_package(&CalcDefinition, &mut session, &package, "Calc", target)
        .expect("phase compilation");

    let phases: Vec<_> = session
        .query_records()
        .iter()
        .map(|record| record.phase)
        .collect();
    assert_eq!(
        phases,
        vec![QueryPhase::Parse, QueryPhase::Elaborate, QueryPhase::Lower]
    );
    let elaborate = &session.query_records()[1];
    assert!(
        elaborate
            .dependencies
            .iter()
            .any(|dependency| matches!(dependency, QueryDependency::Query(_)))
    );
    let lower = &session.query_records()[2];
    assert!(
        lower
            .dependencies
            .iter()
            .any(|dependency| matches!(dependency, QueryDependency::Query(_)))
    );
}

#[test]
fn language_metadata_is_loaded_from_the_definition_source() {
    let descriptor = klf_services::Definition::language(&CalcDefinition);
    assert_eq!(descriptor.name, "calc");
    assert_eq!(descriptor.extensions, vec!["calc"]);
    assert_eq!(descriptor.driver, "calc");
}
