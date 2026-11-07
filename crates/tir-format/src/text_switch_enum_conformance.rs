use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(definition: &str, body: &str) -> String {
    format!(
        "module @Enum tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{definition}\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("enum switch text parses");
    let bytes = assemble_text_module(&parsed).expect("enum switch CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("enum switch binary decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("enum CODE disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&text).expect("canonical text parses"))
        .expect("enum CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

const CHOICE: &str = "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged";

#[test]
fn exhaustive_switch_enum_passes_payload_to_variant_block() {
    let source = module(
        CHOICE,
        "func module @Enum.inspect(%value: %Choice) -> i32 engine(native) {\nbb0(%value: %Choice):\n  switch.enum %value, [Number: bb1], [None: bb2]\nbb1(%payload: i32):\n  return %payload\nbb2:\n  %zero = const.int i32, 0\n  return %zero\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("switch.enum %"));
    assert!(canonical.contains("[None: bb2], [Number: bb1]"));
}

#[test]
fn partial_enum_switch_must_have_parameterless_default() {
    let source = module(
        CHOICE,
        "func module @Enum.inspect(%value: %Choice) -> i32 engine(native) {\nbb0(%value: %Choice):\n  switch.enum %value, [None: bb2], [default bb1]\nbb1:\n  %one = const.int i32, 1\n  return %one\nbb2:\n  %zero = const.int i32, 0\n  return %zero\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("[None: bb2], [default bb1]"));
}

#[test]
fn switch_enum_rejects_unknown_variants_and_payload_mismatch() {
    for cases in [
        "[Missing: bb1], [default bb2]",
        "[Number: bb1], [Number: bb1], [default bb2]",
        "[None: bb2]",
        "[None: bb1], [Number: bb2]",
    ] {
        let source = module(
            CHOICE,
            &format!(
                "func module @Enum.bad(%value: %Choice) -> i32 engine(native) {{\nbb0(%value: %Choice):\n  switch.enum %value, {cases}\nbb1:\n  %one = const.int i32, 1\n  return %one\nbb2:\n  %two = const.int i32, 2\n  return %two\n}}"
            ),
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "bad enum switch assembled: {cases}"
            );
        }
    }
}
