use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Calls tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(source: &str) -> String {
    let parsed = parse_text_module(source).expect("direct call text parses");
    let bytes = assemble_text_module(&parsed).expect("direct call CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("direct call CODE decodes");
    let canonical =
        disassemble_text_module(&binary, u64::MAX).expect("direct call CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical direct call CODE reassembles");
    assert_eq!(bytes, rebuilt);
    canonical
}

#[test]
fn resultful_direct_call_uses_forward_signature_and_ssa_result() {
    let source = module(
        "func module @Calls.caller(%arg: i32) -> i32 engine(native) {\nbb0(%arg: i32):\n  %result = call @Calls.double(%arg)\n  return %result\n}\n\nfunc module @Calls.double(%number: i32) -> i32 engine(native) {\nbb0(%number: i32):\n  %twice = add %number, %number\n  return %twice\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains(" = call @Calls.double(%"));
}

#[test]
fn unit_direct_call_and_multiple_arguments_round_trip() {
    let source = module(
        "func module @Calls.caller(%a: i32, %b: i32) -> unit engine(native) {\nbb0(%a: i32, %b: i32):\n  call @Calls.sink(%a, %b)\n  return\n}\n\nfunc module @Calls.sink(%a: i32, %b: i32) -> unit engine(native) {\nbb0(%a: i32, %b: i32):\n  return\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("call @Calls.sink(%"));
}

#[test]
fn direct_calls_are_supported_across_basic_blocks() {
    let source = module(
        "func module @Calls.caller(%input: i64) -> i64 engine(native) {\nbb0(%input: i64):\n  br bb1(%input)\nbb1(%value: i64):\n  %returned = call @Calls.callee(%value)\n  return %returned\n}\n\nfunc module @Calls.callee(%value: i64) -> i64 engine(native) {\nbb0(%value: i64):\n  return %value\n}",
    );
    assert!(round_trip(&source).contains(" = call @Calls.callee(%"));
}

#[test]
fn direct_calls_reject_mismatched_types_results_and_unknown_callees() {
    for (callee, caller) in [
        (
            "func module @Calls.f(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  return %x\n}",
            "func module @Calls.main(%x: i64) -> i32 engine(native) {\nbb0(%x: i64):\n  %r = call @Calls.f(%x)\n  return %r\n}",
        ),
        (
            "func module @Calls.f(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  return %x\n}",
            "func module @Calls.main() -> unit engine(native) {\nbb0:\n  call @Calls.f()\n  return\n}",
        ),
        (
            "func module @Calls.f() -> unit engine(native) {\nbb0:\n  return\n}",
            "func module @Calls.main() -> unit engine(native) {\nbb0:\n  %r = call @Calls.f()\n  return\n}",
        ),
        (
            "",
            "func module @Calls.main() -> unit engine(native) {\nbb0:\n  call @Calls.missing()\n  return\n}",
        ),
        (
            "func module @Calls.f() -> unit engine(native) {\nbb0:\n  return\n}",
            "func module @Calls.main() -> unit engine(native) {\nbb0:\n  call @Calls.f(%missing)\n  return\n}",
        ),
    ] {
        let source = module(&format!("{caller}\n\n{callee}"));
        let parsed = parse_text_module(&source).expect("invalid direct call syntax parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "mismatched direct call assembled: {caller}"
        );
    }
}

#[test]
fn tail_call_is_zero_result_terminator_even_for_nonunit_returns() {
    let source = module(
        "func module @Calls.caller(%arg: i32) -> i32 engine(native) {\nbb0(%arg: i32):\n  tail.call @Calls.callee(%arg)\n}\n\nfunc module @Calls.callee(%number: i32) -> i32 engine(native) {\nbb0(%number: i32):\n  return %number\n}",
    );
    let text = round_trip(&source);
    assert!(text.contains("tail.call @Calls.callee(%"));
}

#[test]
fn tail_call_in_branch_successor_round_trips() {
    let source = module(
        "func module @Calls.caller(%arg: i32) -> i32 engine(native) {\nbb0(%arg: i32):\n  br bb1(%arg)\nbb1(%passed: i32):\n  tail.call @Calls.callee(%passed)\n}\n\nfunc module @Calls.callee(%number: i32) -> i32 engine(native) {\nbb0(%number: i32):\n  return %number\n}",
    );
    assert!(round_trip(&source).contains("tail.call @Calls.callee(%"));
}

#[test]
fn tail_call_rejects_result_abi_and_argument_mismatch() {
    for (callee, caller) in [
        (
            "func module @Calls.callee() -> unit engine(native) {\nbb0:\n  return\n}",
            "func module @Calls.caller() -> i32 engine(native) {\nbb0:\n  tail.call @Calls.callee()\n}",
        ),
        (
            "func module @Calls.callee(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  return %x\n}",
            "func module @Calls.caller(%x: i64) -> i32 engine(native) {\nbb0(%x: i64):\n  tail.call @Calls.callee(%x)\n}",
        ),
        (
            "func module @Calls.callee() -> unit engine(native) abi(c) {\nbb0:\n  return\n}",
            "func module @Calls.caller() -> unit engine(native) {\nbb0:\n  tail.call @Calls.callee()\n}",
        ),
        (
            "func module @Calls.callee() -> unit engine(native) {\nbb0:\n  return\n}",
            "func module @Calls.caller() -> unit engine(native) {\nbb0:\n  %result = tail.call @Calls.callee()\n}",
        ),
    ] {
        let source = module(&format!("{caller}\n\n{callee}"));
        let parsed = parse_text_module(&source).expect("invalid tail.call source parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid tail.call accepted: {caller}"
        );
    }
}

#[test]
fn direct_call_site_attributes_canonicalize_by_bit_order() {
    let source = module(
        "func module @Calls.caller(%arg: i32) -> i32 engine(native) {\nbb0(%arg: i32):\n  %result = call [noinline] [cold] [nounwind] @Calls.callee(%arg)\n  return %result\n}\n\nfunc module @Calls.callee(%value: i32) -> i32 engine(native) nounwind {\nbb0(%value: i32):\n  return %value\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("call [cold] [nounwind] [noinline] @Calls.callee"));
}

#[test]
fn tail_call_site_attributes_round_trip_without_creating_results() {
    let source = module(
        "func module @Calls.start(%arg: i64) -> i64 engine(native) {\nbb0(%arg: i64):\n  tail.call [nounwind] [cold] @Calls.end(%arg)\n}\n\nfunc module @Calls.end(%value: i64) -> i64 engine(native) nounwind {\nbb0(%value: i64):\n  return %value\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("tail.call [cold] [nounwind] @Calls.end"));
}

#[test]
fn invalid_or_unrepresentable_call_attributes_are_rejected() {
    for prefix in [
        "call [inline] [noinline]",
        "call [cold] [cold]",
        "call [builtin]",
        "call [unknown]",
        "call [nounwind",
    ] {
        let source = module(&format!(
            "func module @Calls.caller() -> unit engine(native) {{\nbb0:\n  {prefix} @Calls.callee()\n  return\n}}\n\nfunc module @Calls.callee() -> unit engine(native) {{\nbb0:\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid call attributes accepted: {prefix}"
            );
        }
    }
}
