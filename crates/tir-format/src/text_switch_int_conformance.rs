use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Switch tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("switch.int source parses");
    let bytes = assemble_text_module(&parsed).expect("switch.int CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("switch.int binary decodes");
    let text = disassemble_text_module(&binary, u64::MAX).expect("switch.int CODE disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&text).expect("canonical text parses"))
        .expect("canonical CODE reassembles");
    assert_eq!(bytes, rebuilt);
    text
}

#[test]
fn switch_int_negative_zero_and_default_edges_round_trip() {
    let body = "func module @Switch.route(%selector: i32, %value: i32) -> i32 engine(native) {\nbb0(%selector: i32, %value: i32):\n  switch.int %selector, [-1: bb1(%value)], [0: bb2(%value)], default bb3(%value) [merge bb4]\nbb1(%negative: i32):\n  br bb4(%negative)\nbb2(%zero: i32):\n  br bb4(%zero)\nbb3(%otherwise: i32):\n  br bb4(%otherwise)\nbb4(%result: i32):\n  return %result\n}";
    let canonical = round_trip(&module(body));
    for expected in [
        "switch.int %",
        "[-1: bb1(",
        "[0: bb2(",
        "default bb3(",
        "[merge bb4]",
    ] {
        assert!(canonical.contains(expected), "missing {expected}");
    }
}

#[test]
fn switch_int_default_only_round_trips() {
    let body = "func module @Switch.default(%selector: i64, %value: i64) -> i64 engine(native) {\nbb0(%selector: i64, %value: i64):\n  switch.int %selector, default bb1(%value)\nbb1(%result: i64):\n  return %result\n}";
    let canonical = round_trip(&module(body));
    assert!(canonical.contains("switch.int %arg0, default bb1(%"));
}

#[test]
fn switch_int_full_i128_case_range_round_trips() {
    let body = "func module @Switch.wide(%selector: i128, %value: i32) -> i32 engine(native) {\nbb0(%selector: i128, %value: i32):\n  switch.int %selector, [-170141183460469231731687303715884105728: bb1(%value)], [340282366920938463463374607431768211455: bb2(%value)], default bb3(%value)\nbb1(%a: i32):\n  return %a\nbb2(%b: i32):\n  return %b\nbb3(%c: i32):\n  return %c\n}";
    let canonical = round_trip(&module(body));
    assert!(canonical.contains("-170141183460469231731687303715884105728"));
    assert!(canonical.contains("340282366920938463463374607431768211455"));
}

#[test]
fn switch_int_rejects_duplicate_values_overflows_and_incompatible_targets() {
    for (parameters, switch) in [
        (
            "%k: i8, %v: i32",
            "switch.int %k, [0: bb1(%v)], [0: bb1(%v)], default bb1(%v)",
        ),
        (
            "%k: i8, %v: i32",
            "switch.int %k, [-1: bb1(%v)], [255: bb1(%v)], default bb1(%v)",
        ),
        (
            "%k: i8, %v: i32",
            "switch.int %k, [256: bb1(%v)], default bb1(%v)",
        ),
        (
            "%k: i8, %v: i32",
            "switch.int %k, [-129: bb1(%v)], default bb1(%v)",
        ),
        (
            "%k: i8, %v: i32",
            "switch.int %k, [-0: bb1(%v)], default bb1(%v)",
        ),
        (
            "%k: f32, %v: i32",
            "switch.int %k, [0: bb1(%v)], default bb1(%v)",
        ),
        (
            "%k: i8, %v: i32",
            "switch.int %k, [1: bb1(%k)], default bb1(%v)",
        ),
        (
            "%k: i8, %v: i32",
            "switch.int %k, [1: bb2(%v)], default bb1(%v)",
        ),
        (
            "%k: i8, %v: i32",
            "switch.int %k, [1: bb1(%v)], default bb1(%v) [merge bb3]",
        ),
    ] {
        let body = format!(
            "func module @Switch.invalid({parameters}) -> i32 engine(native) {{\nbb0({parameters}):\n  {switch}\nbb1(%result: i32):\n  return %result\n}}"
        );
        let source = module(&body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid switch assembled: {switch}"
            );
        }
    }
}
