//! Harness: emit a native object with Cranelift, link it against the shared
//! `klf-native-rt` runtime, run it, and read the outcome in the VM's shape.

use klf_kir::parse_module;
use klf_vm::Outcome;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

struct Tools {
    clang: PathBuf,
    sysroot: Option<String>,
    runtime: PathBuf,
}

fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| Tools {
        clang: find_clang().expect("no RIDL LLVM clang under ~/.klf/llvm-ridl"),
        sysroot: macos_sysroot(),
        runtime: build_runtime(),
    })
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn find_clang() -> Option<PathBuf> {
    klf_target::llvm_tool("clang")
}

fn macos_sysroot() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = Command::new("xcrun")
        .args(["--show-sdk-path"])
        .output()
        .expect("xcrun runs");
    Some(
        String::from_utf8(output.stdout)
            .expect("sdk path is utf8")
            .trim()
            .to_owned(),
    )
}

/// The program runs on this machine, so it is built for the host whatever
/// triple the fixture declares.
fn for_host(mut module: klf_kir::Module) -> klf_kir::Module {
    module.target = klf_target::host_triple().parse().expect("host triple");
    module
}

fn build_runtime() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "-p", "klf-native-rt"])
        .current_dir(workspace_root())
        .status()
        .expect("cargo build runs");
    assert!(status.success(), "building klf-native-rt failed");
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace_root().join("target"), PathBuf::from)
        .join("release/libklf_native_rt.a")
}

/// Lower `source`'s `entry` to a native object with Cranelift, link and run it.
pub fn cranelift_run(source: &str, entry: &str, arguments: &[u64]) -> Outcome {
    parse_outcome(&cranelift_stdout(source, entry, arguments))
}

/// Link and run the program, returning its full stdout (so a test can see kernel
/// service output as well as the final `RET`/`TRAP` line).
pub fn cranelift_stdout(source: &str, entry: &str, arguments: &[u64]) -> String {
    let module = for_host(klf_kir_passes::monomorphize(
        &parse_module(source).expect("module parses"),
    ));
    let object = klf_cranelift::emit_object(&module, entry, arguments).expect("module emits");

    let tools = tools();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(source, &mut hasher);
    let tag = std::hash::Hasher::finish(&hasher);
    let dir = std::env::temp_dir().join(format!("klf-clif-{entry}-{}-{tag:x}", arguments.len()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let obj = dir.join("program.o");
    let bin = dir.join("program.out");
    std::fs::write(&obj, object).expect("write object");

    let mut command = Command::new(&tools.clang);
    if let Some(sysroot) = &tools.sysroot {
        command.args(["-isysroot", sysroot]);
    }
    if !cfg!(target_os = "macos") && tools.clang.with_file_name("ld.lld").exists() {
        command.arg("-fuse-ld=lld");
    }
    let status = command
        .arg(&obj)
        .arg(&tools.runtime)
        .arg("-lm")
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("clang runs");
    assert!(status.success(), "clang link failed for `{entry}`");

    let output = Command::new(&bin).output().expect("native program runs");
    String::from_utf8(output.stdout).expect("stdout is utf8")
}

fn parse_outcome(stdout: &str) -> Outcome {
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    if let Some(code) = line.strip_prefix("TRAP ") {
        return Outcome::Trap(code.to_owned());
    }
    if let Some(value) = line.strip_prefix("FAIL ") {
        return Outcome::Fail(value.trim().parse().expect("fail value"));
    }
    if let Some(rest) = line.strip_prefix("RET ") {
        let rest = rest.trim();
        if rest == "void" {
            return Outcome::Return(None);
        }
        return Outcome::Return(Some(rest.parse().expect("return value")));
    }
    panic!("unrecognized native output: {stdout:?}");
}
