use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn source(functions: &str) -> String {
    format!(
        "module @Strings tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{functions}\n"
    )
}

fn round_trip(input: &str) -> String {
    let parsed = parse_text_module(input).expect("string module parses");
    let bytes = assemble_text_module(&parsed).expect("string constants assemble");
    let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
    let canonical = disassemble_text_module(&decoded, u64::MAX).expect("string CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical string CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn multiple_strings_feed_select_in_same_block() {
    let input = source(
        "func module @Strings.choose(%cond: i1) -> (ptr, i64) engine(native) {\nbb0(%cond: i1):\n  %left = const.string \"hello\"\n  %right = const.string \"world\"\n  %chosen = select %cond, %left, %right\n  return %chosen\n}",
    );
    let canonical = round_trip(&input);
    assert!(canonical.contains("const.string \"hello\""));
    assert!(canonical.contains("const.string \"world\""));
    assert_eq!(canonical.matches("const.string").count(), 2);
}

#[test]
fn string_constants_in_second_block_and_multiple_functions_round_trip() {
    let input = source(
        "func module @Strings.first(%cond: i1) -> (ptr, i64) engine(native) {\nbb0(%cond: i1):\n  br bb1(%cond)\nbb1(%flag: i1):\n  %yes = const.string \"yes\"\n  %no = const.string \"no\"\n  %chosen = select %flag, %yes, %no\n  return %chosen\n}\n\nfunc module @Strings.second() -> (ptr, i64) engine(native) {\nbb0:\n  %answer = const.string \"other\"\n  return %answer\n}",
    );
    let canonical = round_trip(&input);
    assert_eq!(canonical.matches("const.string").count(), 3);
}

#[test]
fn string_constant_utf8_and_escaped_text_round_trip() {
    let input = source(
        "func module @Strings.utf8() -> (ptr, i64) engine(native) {\nbb0:\n  %message = const.string \"café \\n snow\"\n  return %message\n}",
    );
    let canonical = round_trip(&input);
    assert!(canonical.contains("const.string"));
}
