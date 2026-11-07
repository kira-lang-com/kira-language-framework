//! V21: module imports and instructions agree with TIR section 32.3.
use tir_diag::Severity;
use tir_format::{BinaryModule, assemble_text_module, parse_text_module};

use super::{verify_binary_module_records, verify_text_module};

fn source(platform: &str, items: &str) -> String {
    format!(
        "module @PlatformSmoke tir 26.11.1 profile app platform {platform} semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{items}\n"
    )
}

fn errors(source: &str) -> Vec<String> {
    let parsed = parse_text_module(source).expect("valid TIR text");
    verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

fn binary_errors(source: &str) -> Vec<String> {
    let parsed = parse_text_module(source).expect("valid TIR text");
    let bytes = assemble_text_module(&parsed).expect("TIR platform module assembles");
    let module = BinaryModule::decode(&bytes).expect("canonical binary decodes");
    verify_binary_module_records(&module, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

#[test]
fn v21_portable_modules_reject_platform_imports_in_text_and_binary() {
    let text = source("portable", "import platform \"libc.so.6\"");
    assert!(errors(&text).contains(&"TIR-PLAT-0001".to_owned()));
    assert!(binary_errors(&text).contains(&"TIR-PLAT-0001".to_owned()));
}

#[test]
fn v21_platform_library_names_match_the_selected_target() {
    let valid = [
        ("linux", "libc.so.6"),
        ("macos", "/usr/lib/libSystem.B.dylib"),
        ("windows", "kernel32.dll"),
        ("android", "liblog.so"),
        ("wasi", "wasi:filesystem/types@0.2.0"),
    ];
    for (platform, library) in valid {
        let text = source(platform, &format!("import platform \"{library}\""));
        assert!(
            !errors(&text).contains(&"TIR-PLAT-0001".to_owned()),
            "valid {platform} platform import rejected"
        );
        assert!(
            !binary_errors(&text).contains(&"TIR-PLAT-0001".to_owned()),
            "valid {platform} binary platform import rejected"
        );
    }

    for (platform, library) in [
        ("linux", "kernel32.dll"),
        ("windows", "libc.so.6"),
        ("macos", "libc.so.6"),
        ("wasi", "libc.so.6"),
    ] {
        let text = source(platform, &format!("import platform \"{library}\""));
        assert!(
            errors(&text).contains(&"TIR-PLAT-0001".to_owned()),
            "invalid {platform} platform import accepted"
        );
        assert!(
            binary_errors(&text).contains(&"TIR-PLAT-0001".to_owned()),
            "invalid {platform} binary import accepted"
        );
    }
}

#[test]
fn v21_portable_modules_reject_foreign_instructions() {
    let text = source(
        "portable",
        "func module @PlatformSmoke.foreign() -> unit engine(native) {\nbb0:\n  foreign.addr @Foreign.symbol\n  return\n}",
    );
    assert!(errors(&text).contains(&"TIR-PLAT-0001".to_owned()));
}

#[test]
fn v21_device_io_requires_tessera_platform() {
    let text = source(
        "linux",
        "func module @PlatformSmoke.io() -> unit engine(native) {\nbb0:\n  fence.io\n  return\n}",
    );
    assert!(errors(&text).contains(&"TIR-PLAT-0001".to_owned()));
}

#[test]
fn mmio_code_is_rejected_on_non_tessera_platforms_in_binary() {
    let text = source(
        "linux",
        "func module @PlatformSmoke.io(%p: ptr, %value: i32) -> unit engine(native) {\nbb0(%p: ptr, %value: i32):\n  mmio.store %value, %p [order release] [align 4]\n  fence.io [full]\n  return\n}",
    );
    assert!(errors(&text).contains(&"TIR-PLAT-0001".to_owned()));
    assert!(binary_errors(&text).contains(&"TIR-PLAT-0001".to_owned()));
}
