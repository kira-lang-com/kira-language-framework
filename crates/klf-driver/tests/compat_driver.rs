use klf_driver::{Backend, LanguageDefinition, build_frontend, run, toolchain_available};
use klf_services::{DefinitionPackage, DriverRequest, Session};
use klf_target::Target;
use std::process::Command;

const LANGUAGE: &str = r#"
construct Compat() extends Language<Tree, Program> {
    let name: String = "compat"
    let driver: String = "compat"
    let version: String = "0.1.0"
    let extensions: [String] = ["cmp"]
    let memory: MemoryModel = .Counted
    let exclusivity: Exclusivity = .None
    let convention: DefaultConvention = .Ridl
    let stdRoot: String = "std"

    function compatDriver(arguments: [String]) -> Optional<DriverRequest> {
        var normalized: [String] = []
        var changed = false
        for argument in arguments {
            if argument == "-c" {
                changed = true
            } else if argument == "--compat-out" {
                normalized.append("-o")
            } else {
                normalized.append(argument)
            }
        }
        if changed {
            return .Some(DriverRequest { arguments = normalized })
        }
        return .None
    }

    function units(session: Session, inputs: [SourceFile]) -> [Unit] {
        var result: [Unit] = []
        for file in inputs {
            result.append(definition_unit(session, file))
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
        let module = kir_module_new(session, unit, program)
        let word = kir_type_integer(64)
        let signature = kir_signature_new(module, 0, source_len(), word)
        let generated = kir_signature_define(signature)
        let result = kir_value_integer(generated, word, 42)
        kir_function_result(generated, result)
        return kir_module_finish(module)
    }
}
"#;

fn target() -> Target {
    "aarch64-apple-darwin".parse().expect("valid target")
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn compat_driver_normalizes_arguments_on_vm_and_native_frontends() {
    let package = DefinitionPackage::single("language.klfd", LANGUAGE);
    let mut session = Session::new(target());
    let language = LanguageDefinition::from_package(&mut session, &package, Backend::Vm)
        .unwrap_or_else(|| panic!("definition builds: {:?}", session.diagnostics()));
    assert_eq!(
        language
            .compat_driver(&strings(&["-c", "input.cmp", "--compat-out", "app.kir"]))
            .expect("compatDriver runs on the VM"),
        Some(DriverRequest {
            arguments: strings(&["input.cmp", "-o", "app.kir"]),
        })
    );
    assert_eq!(
        language
            .compat_driver(&strings(&["input.cmp", "", "-o", "app.kir"]))
            .expect("compatDriver keeps uniform arguments"),
        None
    );

    if !toolchain_available() {
        eprintln!("native half skipped: clang toolchain unavailable");
        return;
    }
    let directory = std::env::temp_dir().join(format!("klf-compat-driver-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("test directory");
    let source = directory.join("language.klfd");
    std::fs::write(&source, LANGUAGE).expect("language written");
    let binary = directory.join("compat");
    build_frontend(
        &mut Session::new(target()),
        &source,
        Some(&binary),
        &target(),
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("native frontend builds: {error}"));
    let input = directory.join("answer");
    std::fs::write(&input, "answer").expect("input written");

    let compat_output = directory.join("compat.kir");
    let result = Command::new(&binary)
        .arg("-c")
        .arg(&input)
        .arg("--compat-out")
        .arg(&compat_output)
        .output()
        .expect("native frontend runs");
    assert!(
        result.status.success(),
        "compat invocation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let module = klf_kir::parse_module(
        &std::fs::read_to_string(&compat_output).expect("compat output written"),
    )
    .expect("KIR parses");
    assert_eq!(
        run(&module, "answer", &[], Backend::Vm).expect("KIR runs"),
        klf_vm::Outcome::Return(Some(42))
    );

    let uniform_output = directory.join("uniform.kir");
    let result = Command::new(&binary)
        .arg(&input)
        .arg("-o")
        .arg(&uniform_output)
        .output()
        .expect("native frontend runs");
    assert!(
        result.status.success(),
        "uniform invocation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(uniform_output.exists());
}
