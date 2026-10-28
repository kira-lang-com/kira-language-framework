use klf_driver::{Backend, LanguageDefinition};
use klf_services::{DefinitionPackage, Session};
use klf_target::Target;

const LANGUAGE: &str = r#"
construct Runner() extends Language<Tree, Program> {
    let name: String = "runner"
    let driver: String = "runner"
    let version: String = "0.1.0"
    let extensions: [String] = ["run"]
    let memory: MemoryModel = .Counted
    let exclusivity: Exclusivity = .None
    let convention: DefaultConvention = .Ridl
    let stdRoot: String = "std"

    function runProgram(command: [String], driverArguments: [String]) -> Int {
        let status = runCommand(command)
        if commandOutput(1) != "out" {
            return 90
        }
        if commandOutput(2) != "err" {
            return 91
        }
        var total = status
        for argument in driverArguments {
            total = total + 10
        }
        writeOutput("")
        return total
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
        return kir_module_finish(module)
    }
}
"#;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn run_program_runs_a_command_and_reads_what_it_wrote() {
    let package = DefinitionPackage::single("language.klfd", LANGUAGE);
    let target: Target = "aarch64-apple-darwin".parse().expect("valid target");
    let mut session = Session::new(target);
    let language = LanguageDefinition::from_package(&mut session, &package, Backend::Vm)
        .unwrap_or_else(|| panic!("definition builds: {:?}", session.diagnostics()));
    let command = strings(&["sh", "-c", "printf out; printf err >&2; exit 3"]);
    assert_eq!(
        language.run_program(&command, &strings(&["--emit", "run"])),
        Ok(Some(23))
    );
    assert_eq!(
        language.run_program(&strings(&["/nonexistent/program"]), &[]),
        Ok(Some(90))
    );
}
