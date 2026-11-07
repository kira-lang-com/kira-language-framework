use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(semantics: &str, body: &str) -> String {
    format!(
        "module @Constants tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("module parses");
    let bytes = assemble_text_module(&parsed).expect("module assembles");
    let decoded = BinaryModule::decode(&bytes).expect("canonical binary decodes");
    let canonical = disassemble_text_module(&decoded, u64::MAX).expect("binary disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical text assembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn pointer_null_constants_in_ssa_chains_round_trip() {
    for (space, suffix) in [
        ("ptr", ""),
        ("ptr<device>", " device"),
        ("ptr<thread>", " thread"),
        ("ptr<constant>", " constant"),
    ] {
        let body = format!(
            "func module @Constants.compare(%a: {space}) -> i1 engine(native) {{\nbb0(%a: {space}):\n  %null = const.null{suffix}\n  %also_null = const.null{suffix}\n  %same = icmp eq %null, %also_null\n  return %same\n}}"
        );
        let canonical = round_trip(&module("defined", &body));
        assert_eq!(canonical.matches("const.null").count(), 2);
    }
}

#[test]
fn pointer_null_constant_is_available_in_second_basic_block() {
    let body = "func module @Constants.check(%a: ptr) -> i1 engine(native) {\nbb0(%a: ptr):\n  br bb1(%a)\nbb1(%next: ptr):\n  %null = const.null\n  %is_null = icmp eq %null, %next\n  return %is_null\n}";
    let canonical = round_trip(&module("defined", body));
    assert!(canonical.contains("const.null"));
}

#[test]
fn poison_in_ssa_chain_requires_permissive_semantics() {
    let body = "func module @Constants.poison(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %poison = const.poison i32\n  %result = add %a, %poison\n  return %result\n}";
    let canonical = round_trip(&module("permissive", body));
    assert!(canonical.contains("const.poison i32"));
    let defined = module("defined", body);
    let parsed = parse_text_module(&defined).expect("defined module parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "defined semantics must reject poison"
    );
}

#[test]
fn poison_pointer_can_be_compared_in_permissive_mode() {
    let body = "func module @Constants.poison() -> i1 engine(native) {\nbb0:\n  %poison = const.poison ptr<device>\n  %null = const.null device\n  %result = icmp eq %poison, %null\n  return %result\n}";
    let canonical = round_trip(&module("permissive", body));
    assert!(canonical.contains("const.poison ptr<device>"));
}

#[test]
fn pointer_zero_in_ssa_chains_round_trips_with_address_spaces() {
    for space in ["ptr", "ptr<device>", "ptr<constant>", "ptr<thread>"] {
        let body = format!(
            "func module @Constants.check(%a: {space}) -> i1 engine(native) {{\nbb0(%a: {space}):\n  %zero = const.zero {space}\n  %again = const.zero {space}\n  %equal = icmp eq %zero, %again\n  return %equal\n}}"
        );
        let canonical = round_trip(&module("defined", &body));
        assert_eq!(canonical.matches(&format!("const.zero {space}")).count(), 2);
    }
}

#[test]
fn pointer_zero_in_second_block_is_canonical() {
    let body = "func module @Constants.check(%a: ptr<device>) -> i1 engine(native) {\nbb0(%a: ptr<device>):\n  br bb1(%a)\nbb1(%next: ptr<device>):\n  %zero = const.zero ptr<device>\n  %equal = icmp eq %zero, %next\n  return %equal\n}";
    let canonical = round_trip(&module("defined", body));
    assert!(canonical.contains("const.zero ptr<device>"));
}
