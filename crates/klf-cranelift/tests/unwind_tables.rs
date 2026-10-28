//! The unwind tables of a frontend object (`KLF-EDITOR.md` section 8.6): the
//! system unwinder steps out of Cranelift code into its C caller, on aarch64
//! and, under Rosetta, on `x86_64` Mach-O.

use std::path::PathBuf;
use std::process::Command;

/// A definition whose `units` phase calls C, which walks the stack, while
/// it owns a counted value, so the call has a cleanup pad.
const DEFINITION: &str = r#"module @Walk lang "test" target "TRIPLE" stage raw

func private foreign @walk() -> i64 engine(native)

func private @units() -> i64 engine(runtime) {
bb0():
  %cell = heap.alloc.counted i64
  %r = call.foreign @walk()
  destroy %cell
  return %r
}

func private @parse() -> i64 engine(runtime) {
bb0():
  %r = const.int i64, 0
  return %r
}

func private @elaborate() -> i64 engine(runtime) {
bb0():
  %r = const.int i64, 0
  return %r
}

func private @lower() -> i64 engine(runtime) {
bb0():
  %r = const.int i64, 0
  return %r
}
"#;

/// Calls phase 0 from `run` and answers, through the exit status, whether
/// the unwinder walked from `walk` back into `run` and found the landing
/// table of `units`, whose count is its one pad.
const HARNESS: &str = r"
#include <stdint.h>
#include <unwind.h>

int64_t klffe_phase_0(int64_t, int64_t, int64_t, int64_t);
__attribute__((noinline)) int64_t run(void);

static int reached;
static int landing;

static _Unwind_Reason_Code step(struct _Unwind_Context *context, void *unused) {
    uintptr_t ip = _Unwind_GetIP(context);
    uintptr_t start = (uintptr_t)&run;
    if (ip > start && ip < start + 256) {
        reached = 1;
    }
    const uint32_t *table = (const uint32_t *)_Unwind_GetLanguageSpecificData(context);
    if (table && *table == 1) {
        landing = 1;
    }
    return _URC_NO_REASON;
}

int64_t walk(void) {
    _Unwind_Backtrace(step, 0);
    return reached && landing;
}

__attribute__((noinline)) int64_t run(void) {
    return klffe_phase_0(0, 0, 0, 0) + 1;
}

int main(void) {
    return run() == 2 ? 0 : 1;
}
";

fn clang() -> Option<PathBuf> {
    klf_target::llvm_tool("clang")
}

/// The kernel runtime archive for `triple`, built with Cargo.
fn runtime(triple: &str) -> PathBuf {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let (mut command, directory) =
        klf_target::cargo_build(&workspace, env!("CARGO").as_ref(), "klf-native-rt", triple);
    let status = command
        .current_dir(&workspace)
        .status()
        .expect("cargo runs");
    assert!(
        status.success(),
        "building klf-native-rt for {triple} failed"
    );
    directory.join("libklf_native_rt.a")
}

fn walks_out_of_cranelift_code(triple: &str, launcher: &[&str]) {
    let Some(clang) = clang() else {
        eprintln!("skipped: no provisioned clang");
        return;
    };
    let module =
        klf_kir::parse_module(&DEFINITION.replace("TRIPLE", triple)).expect("module parses");
    let object =
        klf_cranelift::emit_frontend_object(&module, "", 0).expect("frontend object emits");
    let directory =
        std::env::temp_dir().join(format!("klf-unwind-{triple}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("scratch directory");
    std::fs::write(directory.join("walk.o"), object).expect("object written");
    std::fs::write(directory.join("harness.c"), HARNESS).expect("harness written");
    let sdk = Command::new("xcrun")
        .args(["--show-sdk-path"])
        .output()
        .expect("xcrun runs");
    let binary = directory.join("walk");
    let link = Command::new(&clang)
        .arg(format!("--target={triple}"))
        .arg("-isysroot")
        .arg(String::from_utf8_lossy(&sdk.stdout).trim())
        .arg("-fuse-ld=lld")
        .arg(directory.join("harness.c"))
        .arg(directory.join("walk.o"))
        .arg(runtime(triple))
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("clang runs");
    assert!(
        link.status.success(),
        "{triple}: {}",
        String::from_utf8_lossy(&link.stderr)
    );
    let mut command = Command::new(launcher.first().map_or(binary.as_os_str(), |l| l.as_ref()));
    command.args(launcher.iter().skip(1));
    if !launcher.is_empty() {
        command.arg(&binary);
    }
    let status = command.status().expect("program runs");
    let _ = std::fs::remove_dir_all(&directory);
    assert_eq!(
        status.code(),
        Some(0),
        "{triple}: the unwinder stopped in Cranelift code"
    );
}

#[test]
fn the_unwinder_leaves_aarch64_mach_o_cranelift_code() {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        eprintln!("skipped: needs an Apple silicon host");
        return;
    }
    walks_out_of_cranelift_code("aarch64-apple-darwin", &[]);
}

#[test]
fn the_unwinder_leaves_x86_64_mach_o_cranelift_code() {
    if !cfg!(target_os = "macos") {
        eprintln!("skipped: needs a macOS host");
        return;
    }
    if cfg!(target_arch = "x86_64") {
        walks_out_of_cranelift_code("x86_64-apple-darwin", &[]);
    } else if Command::new("arch")
        .args(["-x86_64", "/usr/bin/true"])
        .status()
        .is_ok_and(|status| status.success())
    {
        walks_out_of_cranelift_code("x86_64-apple-darwin", &["arch", "-x86_64"]);
    } else {
        eprintln!("skipped: Rosetta is not installed");
    }
}
