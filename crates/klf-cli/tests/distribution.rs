//! `klf dist` (section 5.1): the distribution's own `klf` builds a toolchain
//! from the archives it ships, without Cargo.

use std::path::Path;
use std::process::Command;

const LANGUAGE: &str = r#"
construct Answer() extends Language<Tree, Program> {
    let name: String = "answer"
    let driver: String = "answer"
    let version: String = "0.1.0"
    let extensions: [String] = ["ans"]
    let memory: MemoryModel = .Unmanaged
    let exclusivity: Exclusivity = .None
    let convention: DefaultConvention = .Platform
    let stdRoot: String = "std"

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
        let main = kir_signature_define(kir_signature_new(module, 0, 4, word))
        kir_function_result(main, kir_value_integer(main, word, 42))
        return kir_module_finish(module)
    }
}
"#;

fn klf(distribution: &Path, arguments: &[&Path]) -> std::process::Output {
    Command::new(distribution.join("bin/klf"))
        .args(arguments)
        .env_remove("KLF_HOME")
        .output()
        .expect("klf runs")
}

#[test]
#[ignore = "builds a release distribution; run with `--run-ignored all`"]
fn a_distribution_builds_toolchains_from_its_own_archives() {
    if Command::new(env!("CARGO_BIN_EXE_klf"))
        .arg("--help")
        .output()
        .is_err()
    {
        return;
    }
    let directory = std::env::temp_dir().join(format!("klf-dist-{}", std::process::id()));
    let distribution = directory.join("klf");
    let made = Command::new(env!("CARGO_BIN_EXE_klf"))
        .arg("dist")
        .arg("--out")
        .arg(&distribution)
        .output()
        .expect("klf dist runs");
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let host = klf_driver::host_triple();
    for file in [
        "bin/klf".to_owned(),
        "kernel/services.manifest".to_owned(),
        format!("kernel/{host}/libklf_kernel.a"),
        format!("kernel/{host}/libklfrt.a"),
        format!("kernel/{host}/libklfvm.a"),
        "llvm/bin/clang".to_owned(),
    ] {
        assert!(
            distribution.join(&file).exists(),
            "{file} is in the distribution"
        );
    }

    let package = directory.join("package");
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(package.join("language.klfd"), LANGUAGE).expect("definition written");
    let sysroot = directory.join("toolchain");
    let built = klf(
        &distribution,
        &[Path::new("build"), &package, Path::new("--out"), &sysroot],
    );
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );

    let program = directory.join("main.ans");
    std::fs::write(&program, "main").expect("program written");
    let binary = directory.join("main");
    let linked = Command::new(sysroot.join("bin/answer"))
        .arg(&program)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("driver runs");
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let run = Command::new(&binary).output().expect("program runs");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "RET 42\n");

    std::fs::remove_file(distribution.join(format!("kernel/{host}/libklfvm.a")))
        .expect("archive removed");
    let missing = klf(
        &distribution,
        &[Path::new("build"), &package, Path::new("--out"), &sysroot],
    );
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("has no libklfvm.a"),
        "a distribution never falls back to Cargo: {}",
        String::from_utf8_lossy(&missing.stderr)
    );
    std::fs::remove_dir_all(&directory).ok();
}
