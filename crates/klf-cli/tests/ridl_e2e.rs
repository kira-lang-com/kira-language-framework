//! M5 exit test: a Rust client calls a KLF library through bindgen.
//!
//! A KLF module exports C-convention functions (Cranelift object) behind the
//! `ridlcc` entries `klf_ridl::llvm::entries` builds for a matching `.ridl`.
//! The client side is what `klf ridl adapters` and `klf ridl bindgen` emit for
//! the same file. A Rust client links all of it and calls the functions,
//! proving the RIDL symbols, the `ridlcc` classification on both sides and the
//! generated wrapper agree.
//!
//! Skips cleanly when `rustc`, the bundled clang or a C linker is unavailable.

use std::path::PathBuf;
use std::process::Command;

use klf_core::FileId;
use klf_kir::parse_module;
use klf_ridl::bindgen::{caller_symbol, rust_bindings};
use klf_ridl::parse as parse_ridl;
use std::collections::BTreeMap;

const KIR: &str = r#"module @Mathlib lang "kira" target "aarch64-apple-darwin" stage raw

func private @add(%a: i64, %b: i64) -> i64 engine(native) {
bb0(%a: i64, %b: i64):
  %s = add %a, %b
  return %s
}

func private @mul(%a: i64, %b: i64) -> i64 engine(native) {
bb0(%a: i64, %b: i64):
  %p = mul %a, %b
  return %p
}
"#;

const RIDL: &str = r"
function add(a: I64, b: I64) -> I64
function mul(a: I64, b: I64) -> I64
";

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn have(cmd: &str, arg: &str) -> bool {
    Command::new(cmd)
        .arg(arg)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The newest bundled clang, as the Kira driver finds it.
fn clang() -> Option<PathBuf> {
    klf_target::llvm_tool("clang")
}

/// Compiles LLVM IR `source` to the object `object`.
fn compile_ir(clang: &PathBuf, source: &str, object: &PathBuf) {
    let ir = object.with_extension("ll");
    std::fs::write(&ir, source).expect("write IR");
    let status = Command::new(clang)
        .args(["-c", "-O2", "-Wno-override-module"])
        .arg(&ir)
        .arg("-o")
        .arg(object)
        .status()
        .expect("clang runs");
    assert!(status.success(), "clang failed on {}", ir.display());
}

#[test]
fn rust_client_calls_a_klf_library_through_bindgen() {
    if !have("rustc", "--version") {
        eprintln!("rustc unavailable; skipping end-to-end bindgen link test");
        return;
    }
    let Some(clang) = clang() else {
        eprintln!("no bundled clang; skipping end-to-end bindgen link test");
        return;
    };
    let target = klf_target::host_triple();
    let dir = std::env::temp_dir().join(format!("klf-ridl-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    // 1. Compile the KLF library to a native object exporting add and mul in
    //    the C convention, and the `ridlcc` entries in front of them.
    let mut kir = parse_module(KIR).expect("KIR parses");
    kir.target = target.parse().expect("host triple");
    let object =
        klf_cranelift::emit_library_object(&kir, &[("add", "Mathlib.add"), ("mul", "Mathlib.mul")])
            .expect("library object emits");
    let ridl = parse_ridl(FileId::from_u32(0), "Mathlib", RIDL).expect(".ridl parses");
    let entry_symbols: BTreeMap<String, String> = ["add", "mul"]
        .iter()
        .map(|name| ((*name).to_owned(), format!("Mathlib.{name}")))
        .collect();
    let entries =
        klf_ridl::llvm::entries(&ridl, &entry_symbols, &BTreeMap::new(), target, &|name| {
            name.to_owned()
        })
        .expect("entries build");
    let entries_object = dir.join("entries.o");
    compile_ir(&clang, &entries, &entries_object);

    // 2. Emit the client's adapters and Rust bindings from the same .ridl.
    let adapter_symbols: BTreeMap<String, String> = ["add", "mul"]
        .iter()
        .map(|name| ((*name).to_owned(), caller_symbol("Mathlib", name)))
        .collect();
    let adapters = klf_ridl::llvm::adapters(
        &ridl,
        &adapter_symbols,
        target,
        &|name| name.to_owned(),
        false,
    )
    .expect("adapters build");
    let adapters_object = dir.join("adapters.o");
    compile_ir(&clang, &adapters, &adapters_object);
    let bindings = rust_bindings(&ridl);
    assert!(bindings.contains("fn Mathlib_add(a: i64, b: i64) -> i64;"));

    // 3. Build the KLF runtime archive the object may reference.
    let build = Command::new(env!("CARGO"))
        .args(["build", "-p", "klf-native-rt"])
        .current_dir(workspace_root())
        .status()
        .expect("cargo build runs");
    assert!(build.success(), "building klf-native-rt failed");
    let runtime = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace_root().join("target"), std::path::PathBuf::from)
        .join("debug/libklf_native_rt.a");

    // 4. Write object, bindings + client, and compile the client with rustc,
    //    linking the KLF object, entries, adapters and runtime.
    let obj = dir.join("mathlib.o");
    std::fs::write(&obj, &object).expect("write object");

    let mut program = bindings;
    program.push_str(
        "\nfn main() {\n    \
         let s = add(20, 22);\n    \
         let p = mul(6, 7);\n    \
         std::process::exit(if s == 42 && p == 42 { 0 } else { 1 });\n}\n",
    );
    let src = dir.join("client.rs");
    std::fs::write(&src, &program).expect("write client");
    let bin = dir.join("client");

    let compile = Command::new("rustc")
        .args(["--edition", "2024", "-O"])
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .arg(format!("-Clink-arg={}", adapters_object.display()))
        .arg(format!("-Clink-arg={}", entries_object.display()))
        .arg(format!("-Clink-arg={}", obj.display()))
        .arg(format!("-Clink-arg={}", runtime.display()))
        .output()
        .expect("rustc runs");
    if !compile.status.success() {
        let stderr = String::from_utf8_lossy(&compile.stderr);
        if stderr.contains("linker") && stderr.contains("not found") {
            eprintln!("no C linker available; skipping end-to-end link test");
            return;
        }
        panic!("client failed to build/link:\n{stderr}");
    }

    // 5. Run the client; it exits 0 only if the KLF functions returned 42.
    let run = Command::new(&bin).status().expect("client runs");
    assert!(
        run.success(),
        "Rust client got the wrong result from the KLF library"
    );
}
