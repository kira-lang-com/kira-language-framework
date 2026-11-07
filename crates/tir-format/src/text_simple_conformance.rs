use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(semantics: &str, body: &str) -> String {
    format!(
        "module @Simple tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("simple text parses");
    let bytes = assemble_text_module(&parsed).expect("simple CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("CODE disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
        .expect("simple CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn simple_non_result_instructions_round_trip_without_changing_ssa_values() {
    let source = module(
        "permissive",
        "func module @Simple.run(%test: i1, %value: i32) -> i32 engine(native) {\nbb0(%test: i1, %value: i32):\n  nop\n  assume %test\n  prof.count 123\n  %sum = add %value, %value\n  return %sum\n}",
    );
    let text = round_trip(&source);
    for instruction in ["nop", "assume %", "prof.count 123", "add %"] {
        assert!(text.contains(instruction), "missing {instruction}");
    }
}

#[test]
fn target_page_size_can_feed_integer_arithmetic() {
    let source = module(
        "defined",
        "func module @Simple.pages() -> i64 engine(native) {\nbb0:\n  %pagesize = target.pagesize\n  %result = add %pagesize, %pagesize\n  return %result\n}",
    );
    assert!(round_trip(&source).contains("target.pagesize"));
}

#[test]
fn target_feature_query_can_select_an_integer() {
    let source = module(
        "defined",
        "func module @Simple.feature(%yes: i32, %no: i32) -> i32 engine(native) {\nbb0(%yes: i32, %no: i32):\n  %enabled = target.has \"atomic128.lockfree\"\n  %result = select %enabled, %yes, %no\n  return %result\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("target.has \"atomic128.lockfree\""));
}

#[test]
fn target_feature_queries_in_different_blocks_round_trip() {
    let source = module(
        "defined",
        "func module @Simple.features() -> i1 engine(native) {\nbb0:\n  %first = target.has \"sve2\"\n  br bb1(%first)\nbb1(%feature: i1):\n  %second = target.has \"atomic128.lockfree\"\n  %result = and %feature, %second\n  return %result\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("target.has \"sve2\""));
    assert!(text.contains("target.has \"atomic128.lockfree\""));
}

#[test]
fn integer_expect_hints_round_trip_with_optional_probability() {
    for expression in [
        "expect %input, 42",
        "expect %input, -3 [probability 0.75]",
        "expect %input, 0x100 [probability 0.125]",
    ] {
        let source = module(
            "defined",
            &format!(
                "func module @Simple.expected(%input: i32) -> i32 engine(native) {{\nbb0(%input: i32):\n  %hint = {expression}\n  %result = add %hint, %input\n  return %result\n}}"
            ),
        );
        let canonical = round_trip(&source);
        assert!(canonical.contains(" = expect %"));
    }
}

#[test]
fn invalid_expect_ranges_and_probabilities_are_rejected() {
    for (ty, expression) in [
        ("i8", "expect %input, 256"),
        ("i8", "expect %input, -129"),
        ("i32", "expect %input, 3 [probability -0.1]"),
        ("i32", "expect %input, 3 [probability 1.1]"),
        ("i32", "expect %input, 3 [probability NaN]"),
        ("f32", "expect %input, 1"),
    ] {
        let source = module(
            "defined",
            &format!(
                "func module @Simple.invalid(%input: {ty}) -> {ty} engine(native) {{\nbb0(%input: {ty}):\n  %hint = {expression}\n  return %hint\n}}"
            ),
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid expect assembled: {expression}"
            );
        }
    }
}

#[test]
fn zero_argument_portable_intrinsics_round_trip() {
    let source = module(
        "defined",
        "func module @Simple.clocks() -> i64 engine(native) {\nbb0:\n  intrinsic 11() -> unit\n  %start = intrinsic 12() -> i64\n  intrinsic 14() -> unit\n  %frequency = intrinsic 13() -> i64\n  %result = add %start, %frequency\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    for item in [
        "intrinsic 11() -> unit",
        "intrinsic 12() -> i64",
        "intrinsic 13() -> i64",
        "intrinsic 14() -> unit",
    ] {
        assert!(canonical.contains(item), "missing {item}");
    }
}

#[test]
fn intrinsic_results_preserve_ssa_types_across_blocks() {
    let source = module(
        "defined",
        "func module @Simple.clocks() -> i64 engine(native) {\nbb0:\n  %start = intrinsic 12() -> i64\n  br bb1(%start)\nbb1(%incoming: i64):\n  %rate = intrinsic 13() -> i64\n  %result = add %incoming, %rate\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("intrinsic 12() -> i64"));
    assert!(canonical.contains("intrinsic 13() -> i64"));
}

#[test]
fn unknown_and_mistyped_intrinsics_are_rejected() {
    for expression in [
        "intrinsic 12() -> unit",
        "intrinsic 14() -> i64",
        "intrinsic 999() -> unit",
        "intrinsic 11(%arg) -> unit",
    ] {
        let source = module(
            "defined",
            &format!(
                "func module @Simple.invalid(%arg: i32) -> unit engine(native) {{\nbb0(%arg: i32):\n  {expression}\n  return\n}}"
            ),
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid intrinsic assembled: {expression}"
            );
        }
    }
}

#[test]
fn isolated_zero_result_opcode_in_unit_function_round_trips() {
    let source = module(
        "defined",
        "func module @Simple.nop() -> unit engine(native) {\nbb0:\n  nop\n  return\n}",
    );
    assert!(round_trip(&source).contains("nop"));
}

#[test]
fn invalid_simple_instructions_are_rejected() {
    for (params, line) in [
        ("%value: i32", "assume %value"),
        ("%value: i1", "assume %unknown"),
        ("%value: i1", "prof.count -1"),
        ("%value: i1", "nop trailing"),
        ("%value: i1", "%x = target.pagesize %value"),
    ] {
        let source = module(
            "permissive",
            &format!(
                "func module @Simple.bad({params}) -> unit engine(native) {{\nbb0({params}):\n  {line}\n  return\n}}"
            ),
        );
        match parse_text_module(&source) {
            Ok(parsed) => assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid CODE accepted: {line}"
            ),
            Err(_) => {}
        }
    }
}
