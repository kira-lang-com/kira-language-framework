//! `klf build <package> --out <dir>` (sections 5.3 and 5.4): the generated
//! driver compiles the language's std into a static library and bytecode, the
//! sysroot records what it was built from, and a program the installed driver
//! links calls into the std.

use klf_driver::{build_sysroot, test_package, toolchain_available};
use klf_services::Session;
use klf_target::Target;
use std::path::Path;
use std::process::Command;

const LANGUAGE: &str = r#"
construct Square() extends Language<Tree, Program> {
    let name: String = "sq"
    let driver: String = "sq"
    let version: String = "0.1.0"
    let extensions: [String] = ["sq"]
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
        if source_len() == 13 {
            let offset = kir_global_new(module, 7, 13, word, 1)
            kir_global_integer(offset, 5)
            let signature = kir_signature_new(module, 0, 6, word)
            kir_signature_parameter(signature, word)
            let square = kir_signature_define(signature)
            let x = kir_function_parameter(square, 0)
            let product = kir_value_binary(square, 2, 0, 1, x, x)
            let stored = kir_value_load(square, kir_value_global_address(square, offset))
            let biased = kir_value_binary(square, 0, 0, 1, product, stored)
            kir_function_result(square, kir_value_binary(square, 1, 0, 1, biased, kir_value_integer(square, word, 5)))
        } else {
            let declared = kir_signature_new(module, 5, 11, word)
            kir_signature_parameter(declared, word)
            let square = kir_signature_declare(declared)
            let main = kir_signature_define(kir_signature_new(module, 0, 4, word))
            kir_value_argument(main, kir_value_integer(main, word, 7))
            kir_function_result(main, kir_value_call(main, square, word))
        }
        return kir_module_finish(module)
    }
}
"#;

#[test]
fn the_sysroot_holds_a_std_the_installed_driver_links() {
    if !cfg!(target_os = "macos") {
        eprintln!("skipped: the sysroot target matrix needs a macOS host");
        return;
    }
    if !toolchain_available() {
        eprintln!("skipped: clang toolchain unavailable");
        return;
    }
    if ["KLF_SYSROOT_AARCH64_UNKNOWN_LINUX_GNU", "KLF_SYSROOT"]
        .iter()
        .all(|name| std::env::var_os(name).is_none())
    {
        eprintln!("skipped: the Linux std needs a sysroot holding its C library");
        return;
    }
    let directory = std::env::temp_dir().join(format!("klf-sysroot-{}", std::process::id()));
    let package = directory.join("package");
    std::fs::create_dir_all(package.join("std")).expect("package directory");
    std::fs::write(package.join("language.klfd"), LANGUAGE).expect("definition written");
    std::fs::write(package.join("std/square.sq"), "square offset").expect("std source written");
    std::fs::create_dir_all(package.join("std/abi")).expect("abi directory");
    std::fs::write(package.join("std/abi/0001.ridlsig"), "record\n").expect("record written");
    let out = directory.join("toolchain");
    let target: Target = "aarch64-apple-darwin".parse().expect("valid target");
    let linux: Target = "aarch64-unknown-linux-gnu".parse().expect("valid target");
    let intel: Target = "x86_64-apple-darwin".parse().expect("valid target");
    let targets = [target.clone(), linux, intel];

    let built = build_sysroot(
        &mut Session::new(target.clone()),
        &package,
        &out,
        &targets,
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("sysroot builds: {error}"));
    assert_eq!(built.driver, out.join("bin/sq"));
    for file in [
        "bin/sq",
        "bin/sq-run",
        "lib/klf/aarch64-apple-darwin/libklfrt.a",
        "lib/klf/aarch64-apple-darwin/libklfvm.a",
        "lib/klf/aarch64-unknown-linux-gnu/libklfrt.a",
        "lib/sq/aarch64-apple-darwin/libstd.a",
        "lib/sq/aarch64-apple-darwin/libstd.dylib",
        "lib/sq/aarch64-unknown-linux-gnu/libstd.a",
        "lib/sq/aarch64-unknown-linux-gnu/libstd.so",
        "lib/sq/bytecode/64/std.klbc",
    ] {
        assert!(out.join(file).exists(), "{file} is in the sysroot");
    }
    assert_std_surface(&out);

    let program = directory.join("app.sq");
    std::fs::write(&program, "main square").expect("program written");
    let binary = directory.join("app");
    let result = Command::new(&built.driver)
        .arg(&program)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("installed driver runs");
    assert!(
        result.status.success(),
        "linking against the std failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let run = Command::new(&binary).output().expect("program runs");
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("RET 49"),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );

    check_cross_targets(&built.driver, &program, &directory);

    let bytecode = directory.join("app.klbc");
    let result = Command::new(&built.driver)
        .arg(&program)
        .args(["--emit", "klbc", "-o"])
        .arg(&bytecode)
        .output()
        .expect("installed driver runs");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let run = Command::new(&built.runner)
        .arg(&bytecode)
        .output()
        .expect("runner runs");
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "RET 49\n",
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );

    build_sysroot(
        &mut Session::new(target.clone()),
        &package,
        &out,
        std::slice::from_ref(&target),
        &[],
        klf_driver::Profile::Debug,
    )
    .unwrap_or_else(|error| panic!("an existing sysroot is replaced: {error}"));
    std::fs::remove_dir_all(&directory).ok();
}

const CORPUS: &str = r#"
construct Smoke() extends Corpus {
    let compare: [String] = ["accept", "stdout"]
    let cases: [CorpusCase] = [
        CorpusCase { name = "square", sources = ["app.sq"], stdout = "RET 49\n" },
        CorpusCase { name = "wrong", sources = ["app.sq"], stdout = "RET 1\n" }
    ]
}
"#;

const ORACLE: &str = r#"
construct AgainstOracle() extends Corpus {
    let oracle: [String] = ["sh", "oracle.sh"]
    let cases: [CorpusCase] = [CorpusCase { name = "square", sources = ["app.sq"] }]
}
"#;

#[test]
fn klf_test_runs_recorded_and_oracle_corpora() {
    if !toolchain_available() {
        eprintln!("skipped: clang toolchain unavailable");
        return;
    }
    let package = std::env::temp_dir().join(format!("klf-corpus-{}", std::process::id()));
    std::fs::create_dir_all(package.join("std")).expect("package directory");
    std::fs::create_dir_all(package.join("corpus")).expect("corpus directory");
    std::fs::write(package.join("language.klfd"), LANGUAGE).expect("definition written");
    std::fs::write(package.join("std/square.sq"), "square offset").expect("std source written");
    std::fs::write(package.join("corpus/app.sq"), "main square").expect("case source written");
    std::fs::write(package.join("corpus/smoke.klfd"), CORPUS).expect("manifest written");
    std::fs::write(package.join("corpus/oracle.klfd"), ORACLE).expect("manifest written");
    std::fs::write(
        package.join("corpus/oracle.sh"),
        "printf '#!/bin/sh\\necho RET 49\\n' > \"$3\"\nchmod +x \"$3\"\n",
    )
    .expect("oracle written");
    let target: Target = klf_driver::host_triple().parse().expect("valid target");

    let results = test_package(
        &mut Session::new(target.clone()),
        &package,
        None,
        &target,
        &[],
    )
    .unwrap_or_else(|error| panic!("corpus runs: {error}"));
    let verdicts: Vec<(&str, bool)> = results
        .iter()
        .map(|result| (result.name.as_str(), result.mismatches.is_empty()))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("AgainstOracle::square", true),
            ("Smoke::square", true),
            ("Smoke::wrong", false),
        ],
        "{results:?}"
    );
    assert!(results[2].mismatches[0].starts_with("stdout: expected \"RET 1\\n\""));

    let only = test_package(
        &mut Session::new(target.clone()),
        &package,
        Some("smoke"),
        &target,
        &[],
    )
    .expect("one corpus runs");
    assert_eq!(only.len(), 2);
    std::fs::remove_dir_all(&package).ok();
}

fn assert_std_surface(out: &Path) {
    let surface = std::fs::read_to_string(out.join("share/sq/ridl/sq.ridl")).expect("surface");
    assert!(surface.contains("function square"), "{surface}");
    klf_ridl::parse::parse(klf_core::FileId::from_u32(0), "sq", &surface)
        .unwrap_or_else(|error| panic!("the std surface parses: {error:?}\n{surface}"));
    assert!(out.join("share/sq/abi/0001.ridlsig").exists());
    // The distribution ships only the tools a build runs, so the test reads
    // the archive with the provisioned LLVM's `llvm-nm`.
    let nm = provisioned_nm().expect("llvm-nm");
    let symbols = Command::new(nm)
        .arg(out.join("lib/sq/aarch64-apple-darwin/libstd.a"))
        .output()
        .expect("llvm-nm runs");
    let symbols = String::from_utf8_lossy(&symbols.stdout);
    assert!(symbols.contains("T _square"), "{symbols}");
    assert!(
        symbols.contains(&format!(
            "T _{}",
            klf_ridl::mangle::function_symbol("sq", "square", None)
        )),
        "{symbols}"
    );
    let elf = std::fs::read(out.join("lib/sq/aarch64-unknown-linux-gnu/libstd.so"))
        .expect("shared std reads");
    assert_eq!(&elf[..4], b"\x7fELF", "the Linux std is an ELF file");
    let lock = std::fs::read_to_string(out.join("definition.lock")).expect("lock written");
    assert!(lock.contains("target aarch64-apple-darwin"), "{lock}");
    assert!(lock.contains("target aarch64-unknown-linux-gnu"), "{lock}");
    assert!(lock.contains(" language.klfd\n"), "{lock}");
    assert!(lock.contains(" square.sq\n"), "{lock}");
}

fn check_cross_targets(driver: &Path, program: &Path, directory: &Path) {
    let intel_binary = directory.join("app-x86_64");
    let result = Command::new(driver)
        .arg(program)
        .args(["--target", "x86_64-apple-darwin", "-o"])
        .arg(&intel_binary)
        .output()
        .expect("installed driver runs");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let run = Command::new("arch")
        .arg("-x86_64")
        .arg(&intel_binary)
        .output()
        .expect("arch runs");
    if run.status.success() {
        assert_eq!(String::from_utf8_lossy(&run.stdout), "RET 49\n");
    }
    let sysroot = cross_sysroot("aarch64-unknown-linux-gnu");
    {
        let (target, file, magic) = ("aarch64-unknown-linux-gnu", "app-linux", &b"\x7fELF"[..]);
        let binary = directory.join(file);
        let result = Command::new(driver)
            .arg(program)
            .args(["--target", target, "-o"])
            .arg(&binary)
            .output()
            .expect("installed driver runs");
        let stderr = String::from_utf8_lossy(&result.stderr);
        if sysroot {
            assert!(result.status.success(), "{target}: {stderr}");
            let bytes = std::fs::read(&binary).expect("cross binary written");
            assert!(
                bytes.starts_with(magic),
                "{target} wrote no {magic:?} image"
            );
        } else {
            assert!(!result.status.success(), "{target} built without a sysroot");
            assert!(stderr.contains(NO_SYSROOT), "{target}: {stderr}");
        }
    }
    let uncarried = Command::new(driver)
        .arg(program)
        .args(["--target", "x86_64-unknown-linux-gnu", "-o"])
        .arg(directory.join("app-linux-x86_64"))
        .output()
        .expect("installed driver runs");
    let stderr = String::from_utf8_lossy(&uncarried.stderr);
    assert!(!uncarried.status.success(), "a target without a std linked");
    let expected = if cross_sysroot("x86_64-unknown-linux-gnu") {
        "carries no `sq` std for `x86_64-unknown-linux-gnu`"
    } else {
        NO_SYSROOT
    };
    assert!(stderr.contains(expected), "{stderr}");
}

/// What the framework says when a target this host is not has no sysroot
/// naming that target's C library.
const NO_SYSROOT: &str = "needs a sysroot holding that target's C library";

/// Whether the environment names a sysroot for `target`, which is what a target
/// this host is not compiles and links against.
fn cross_sysroot(target: &str) -> bool {
    let per_target = format!(
        "KLF_SYSROOT_{}",
        target.to_ascii_uppercase().replace('-', "_")
    );
    [per_target.as_str(), "KLF_SYSROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|path| !path.is_empty() && Path::new(&path).is_dir())
}

/// `llvm-nm` of the newest RIDL LLVM.
fn provisioned_nm() -> Option<std::path::PathBuf> {
    klf_target::llvm_tool("llvm-nm")
}
