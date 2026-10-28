//! The runner resolves a program's std exports through the host's symbol
//! lookup, which sees an executable's symbols only when it exports them, so
//! `klfvm-run` exports its symbols. `klfvm_invoke` and `klfvm_attach` are
//! forced undefined until something needs them: a program that calls no
//! native library would otherwise leave them unreferenced and stripped.
//!
//! The flags follow the target, not the host the build script runs on: Apple
//! linkers name C symbols with a leading underscore and export with
//! `-export_dynamic`, MinGW's exports every symbol on request, and ELF
//! linkers take `-rdynamic`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let vendor = std::env::var("CARGO_CFG_TARGET_VENDOR").unwrap_or_default();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let environment = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let arguments: &[&str] = match (vendor.as_str(), os.as_str(), environment.as_str()) {
        ("apple", _, _) => &[
            "-Wl,-u,_klfvm_invoke",
            "-Wl,-u,_klfvm_attach",
            "-Wl,-export_dynamic",
        ],
        (_, "windows", "msvc") => &[
            "/INCLUDE:klfvm_invoke",
            "/INCLUDE:klfvm_attach",
            "/EXPORT:klfvm_invoke",
            "/EXPORT:klfvm_attach",
        ],
        (_, "windows", _) => &[
            "-Wl,-u,klfvm_invoke",
            "-Wl,-u,klfvm_attach",
            "-Wl,--export-all-symbols",
        ],
        _ => &["-Wl,-u,klfvm_invoke", "-Wl,-u,klfvm_attach", "-rdynamic"],
    };
    for argument in arguments {
        println!("cargo:rustc-link-arg-bins={argument}");
    }
}
