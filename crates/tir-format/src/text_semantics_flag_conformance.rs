use crate::{BinaryModule, assemble_text_module, parse_text_module};

fn module(default_mode: &str, functions: &str) -> String {
    format!(
        "module @Modes tir 26.11.1 profile app semantics {default_mode}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{functions}\n"
    )
}

fn is_defined_only(source: &str) -> bool {
    let parsed = parse_text_module(source).expect("mode test parses");
    let bytes = assemble_text_module(&parsed).expect("mode test assembles");
    let binary = BinaryModule::decode(&bytes).expect("canonical binary decodes");
    binary.header.flags & 1 != 0
}

#[test]
fn defined_only_header_flag_matches_function_effective_modes() {
    let default = "func module @Modes.run() -> unit engine(native) {\nbb0:\n  return\n}";
    assert!(is_defined_only(&module("defined", default)));
    assert!(!is_defined_only(&module("permissive", default)));

    let overridden =
        "func module @Modes.run() -> unit engine(native) semantics(defined) {\nbb0:\n  return\n}";
    assert!(is_defined_only(&module("permissive", overridden)));

    let permissive = "func module @Modes.run() -> unit engine(native) semantics(permissive) {\nbb0:\n  return\n}";
    assert!(!is_defined_only(&module("defined", permissive)));
}

#[test]
fn mixed_function_modes_clear_defined_only_flag() {
    let functions = "func module @Modes.defined() -> unit engine(native) {\nbb0:\n  return\n}\n\nfunc module @Modes.permissive() -> unit engine(native) semantics(permissive) {\nbb0:\n  %value = const.poison i32\n  return\n}";
    assert!(!is_defined_only(&module("defined", functions)));
}
