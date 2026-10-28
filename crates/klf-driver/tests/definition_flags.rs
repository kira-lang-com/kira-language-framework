use klf_driver::{Backend, LanguageDefinition, build_frontend, run, toolchain_available};
use klf_services::{DefinitionPackage, DefinitionSource, DriverFlagValue, Session};
use klf_target::Target;
use std::path::Path;
use std::process::Command;

const LANGUAGE: &str = r#"
construct Flags() extends Language<Tree, Program> {
    let name: String = "flags"
    let driver: String = "flags"
    let version: String = "0.1.0"
    let extensions: [String] = ["flag", "other"]
    let memory: MemoryModel = .Counted
    let exclusivity: Exclusivity = .Dynamic
    let convention: DefaultConvention = .Ridl
    let stdRoot: String = "std"
    let fileModes: [FileMode] = [FileMode {
        name = "flagged",
        pragma = "flags flagged",
        flag = "-fklf-flagged",
        extensions = ["flag"],
        memory = .Some(.Affine),
        convention = .Some(.Platform)
    }]
    let flags: [DriverFlag] = [
        DriverFlag { name = "warnings", default_value = .Boolean(false) },
        DriverFlag { name = "jobs", default_value = .Integer(3) },
        DriverFlag { name = "sysroot", default_value = .String("std") }
    ]

    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        var result: [Unit] = []
        for file in inputs {
            if valid_file_defaults(session, file) {
                result.append(definition_unit(session, file))
            }
        }
        return result
    }

    function parse(session: Session, unit: Unit) -> Tree {
        return definition_tree(session, unit, 0)
    }

    function elaborate(session: Session, unit: Unit, tree: Tree) -> Program {
        return definition_program(session, unit, 0)
    }

    function lower(session: Session, unit: Unit, program: Program) -> KirModule {
        let value = flag_metric(session, "warnings") + flag_metric(session, "jobs") + flag_metric(session, "sysroot")
        let module = kir_module_new(session, unit, program)
        let record = kir_type_struct_new(module, 7, 10)
        kir_type_struct_field_i64(record, 11, 15)
        let word = kir_type_integer(64)
        let signature = kir_signature_new(module, 0, name_end(), word)
        let generated = kir_signature_define(signature)
        let result = kir_value_integer(generated, word, value)
        kir_function_result(generated, result)
        return kir_module_finish(module)
    }
}

function valid_file_defaults(session: Session, file: SourceFile) -> Bool {
    match session.file_memory(file) {
        Unmanaged -> { return false }
        Counted -> {
            if session.file_mode(file) == -1 {
                match session.file_convention(file) {
                    Ridl -> { return true }
                    Platform -> { return false }
                }
            } else {
                return false
            }
        }
        Affine -> {
            if session.file_mode(file) == 0 {
                match session.file_convention(file) {
                    Ridl -> { return false }
                    Platform -> { return true }
                }
            } else {
                return false
            }
        }
    }
}

function flag_metric(session: Session, name: String) -> Int {
    match session.flag(name) {
        Boolean(value) -> {
            if value {
                return 100
            } else {
                return 0
            }
        }
        Integer(value) -> { return value * 10 }
        String(value) -> { return utf8Count(value) }
    }
}

function name_end() -> Int {
    var index = 0
    while index < source_len() {
        if source_byte(index) == (32 as U8) {
            return index
        }
        index = index + 1
    }
    return index
}
"#;

fn target() -> Target {
    "aarch64-apple-darwin".parse().expect("valid target")
}

fn session() -> Session {
    Session::new(target())
}

fn override_flags(session: &mut Session, flags: &[klf_services::DriverFlag]) {
    session
        .initialize_flags(flags)
        .expect("flag defaults initialize");
    session
        .set_flag("warnings", DriverFlagValue::Boolean(true))
        .expect("boolean override applies");
    session
        .set_flag("jobs", DriverFlagValue::Integer(13))
        .expect("integer override applies");
    session
        .set_flag("sysroot", DriverFlagValue::String("custom".to_owned()))
        .expect("string override applies");
}

fn native_compile(
    binary: &Path,
    input: &Path,
    output: &Path,
    select_mode: bool,
) -> std::process::Output {
    let mut command = Command::new(binary);
    command
        .arg(input)
        .arg("-o")
        .arg(output)
        .arg("--warnings")
        .arg("--jobs=13")
        .arg("--sysroot=custom");
    if select_mode {
        command.arg("-fklf-flagged=**/*.other");
    }
    command.output().expect("native frontend runs")
}

#[test]
fn session_flags_and_effective_file_defaults_reach_vm_and_native_frontends() {
    let package = DefinitionPackage::single("language.klfd", LANGUAGE);
    let mut hosted_session = session();
    let language = LanguageDefinition::from_package(&mut hosted_session, &package, Backend::Vm)
        .unwrap_or_else(|| panic!("VM definition builds: {:?}", hosted_session.diagnostics()));
    override_flags(&mut hosted_session, &language.metadata().descriptor.flags);
    let hosted = language
        .compile_sources(
            &mut hosted_session,
            &[DefinitionSource::new("input.flag", "answer Foo item")],
        )
        .unwrap_or_else(|| {
            panic!(
                "VM phase reads Session values: {:?}",
                hosted_session.diagnostics()
            )
        });
    assert_eq!(
        run(&hosted, "answer", &[], Backend::Vm).expect("hosted KIR runs"),
        klf_vm::Outcome::Return(Some(236))
    );
    assert!(hosted.to_string().contains(
        "policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: dynamic, pairing: none }"
    ));
    let hosted_default = language
        .compile_sources(
            &mut hosted_session,
            &[DefinitionSource::new("input.other", "answer Foo item")],
        )
        .unwrap_or_else(|| {
            panic!(
                "VM phase reads package file defaults: {:?}",
                hosted_session.diagnostics()
            )
        });
    assert!(hosted_default.to_string().contains(
        "policy { copy: trivial, destroy: trivial, relocate: bitwise, exclusivity: dynamic, pairing: none }"
    ));
    assert!(!hosted.to_string().contains("abi(ridl)"));
    assert!(hosted_default.to_string().contains("abi(ridl)"));

    if !toolchain_available() {
        eprintln!("native semantic half skipped: clang toolchain unavailable");
        return;
    }
    let directory = std::env::temp_dir().join(format!("klf-session-flag-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("test package directory");
    let source_path = directory.join("language.klfd");
    std::fs::write(&source_path, LANGUAGE).expect("language source written");
    let binary = directory.join("flags");
    let mut build_session = session();
    build_frontend(
        &mut build_session,
        &source_path,
        Some(&binary),
        &target(),
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("native frontend builds: {error}"));

    let input = directory.join("input.other");
    std::fs::write(&input, "answer Foo item").expect("input written");
    let output = directory.join("input.kir");
    let result = native_compile(&binary, &input, &output, true);
    assert!(
        result.status.success(),
        "native frontend failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let emitted = std::fs::read_to_string(output).expect("native KIR emitted");
    let module = klf_kir::parse_module(&emitted).expect("native KIR parses");
    assert_eq!(emitted, hosted.to_string(), "native and VM KIR agree");
    assert_eq!(
        run(&module, "answer", &[], Backend::Vm).expect("native KIR runs"),
        klf_vm::Outcome::Return(Some(236))
    );

    let default_output = directory.join("default.kir");
    let result = native_compile(&binary, &input, &default_output, false);
    assert!(
        result.status.success(),
        "native frontend default-mode run failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let emitted = std::fs::read_to_string(default_output).expect("default native KIR emitted");
    assert_eq!(
        emitted,
        hosted_default.to_string(),
        "native and VM default KIR agree"
    );
}
