use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(mode: &str, body: &str) -> String {
    format!(
        "module @Terminal tir 26.11.1 profile app semantics {mode}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(text: &str) -> String {
    let parsed = parse_text_module(text).expect("terminator text parses");
    let encoded = assemble_text_module(&parsed).expect("terminator CODE assembles");
    let decoded = BinaryModule::decode(&encoded).expect("terminator binary decodes");
    let canonical =
        disassemble_text_module(&decoded, u64::MAX).expect("terminator CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical CODE reassembles");
    assert_eq!(encoded, rebuilt);
    canonical
}

#[test]
fn unreachable_and_trap_are_valid_nonreturning_blocks() {
    for (terminal, mode) in [
        ("unreachable", "defined"),
        ("unreachable [trap]", "defined"),
        ("unreachable [ub]", "permissive"),
        ("trap 0", "defined"),
        ("trap 65535", "defined"),
        ("trap 0x400", "defined"),
    ] {
        let body = format!(
            "func module @Terminal.halt() -> i32 engine(native) {{\nbb0:\n  {terminal}\n}}"
        );
        let canonical = round_trip(&module(mode, &body));
        let expected = match terminal {
            "unreachable [trap]" => "unreachable\n",
            "trap 0x400" => "trap 1024",
            other => other,
        };
        assert!(canonical.contains(expected));
    }
}

#[test]
fn mixed_returning_and_trapping_cfg_round_trips() {
    let body = "func module @Terminal.mixed(%cond: i1, %value: i32) -> i32 engine(native) {\nbb0(%cond: i1, %value: i32):\n  cond.br %cond, bb1(), bb2(%value)\nbb1:\n  %number = const.int i32, 17\n  trap 1024\nbb2(%incoming: i32):\n  return %incoming\n}";
    let canonical = round_trip(&module("defined", body));
    assert!(canonical.contains("trap 1024"));
    assert!(canonical.contains("return %"));
}

#[test]
fn unreachable_ub_in_branch_round_trips_with_permissive_semantics() {
    let body = "func module @Terminal.choose(%condition: i1) -> unit engine(native) {\nbb0(%condition: i1):\n  cond.br %condition, bb1(), bb2()\nbb1:\n  unreachable [ub]\nbb2:\n  return\n}";
    assert!(round_trip(&module("permissive", body)).contains("unreachable [ub]"));
}

#[test]
fn malformed_trap_codes_and_unreachable_modes_are_rejected() {
    for terminal in [
        "trap -1",
        "trap 65536",
        "trap 0x10000",
        "unreachable [unknown]",
        "unreachable [ub] extra",
    ] {
        let body = format!(
            "func module @Terminal.invalid() -> i32 engine(native) {{\nbb0:\n  {terminal}\n}}"
        );
        let source = module("permissive", &body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid terminator assembled: {terminal}"
            );
        }
    }
}

#[test]
fn longjmp_is_zero_result_terminator_and_round_trips() {
    let source = module(
        "defined",
        "func module @Terminal.jump(%buf: ptr, %state: i32) -> i64 engine(native) abi(c) {\nbb0(%buf: ptr, %state: i32):\n  longjmp %buf, %state\n}",
    );
    assert!(round_trip(&source).contains("longjmp %"));
}

#[test]
fn longjmp_in_control_flow_successor_round_trips() {
    let source = module(
        "defined",
        "func module @Terminal.jump(%buf: ptr, %state: i32) -> unit engine(native) abi(c) {\nbb0(%buf: ptr, %state: i32):\n  br bb1(%buf, %state)\nbb1(%target: ptr, %result: i32):\n  longjmp %target, %result\n}",
    );
    assert!(round_trip(&source).contains("longjmp %"));
}

#[test]
fn longjmp_rejects_invalid_buffer_value_and_terminator_result() {
    for (parameters, instruction) in [
        ("%p: ptr<device>, %v: i32", "longjmp %p, %v"),
        ("%p: ptr, %v: i64", "longjmp %p, %v"),
        ("%p: ptr, %v: i32", "longjmp %p, %unknown"),
        ("%p: ptr, %v: i32", "%out = longjmp %p, %v"),
    ] {
        let source = module(
            "defined",
            &format!(
                "func module @Terminal.bad({parameters}) -> unit engine(native) abi(c) {{\nbb0({parameters}):\n  {instruction}\n}}"
            ),
        );
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "accepted invalid longjmp: {instruction}"
            );
        }
    }
}

#[test]
fn fail_terminator_round_trips_without_a_result() {
    let source = module(
        "defined",
        "func module @Terminal.fail(%err: i32) -> i64 fails i32 engine(native) {\nbb0(%err: i32):\n  fail %err\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("fail %"));
}

#[test]
fn fail_in_a_later_block_round_trips() {
    let source = module(
        "defined",
        "func module @Terminal.try(%err: i32) -> i64 fails i32 engine(native) {\nbb0(%err: i32):\n  br bb1(%err)\nbb1(%forwarded: i32):\n  fail %forwarded\n}",
    );
    assert!(round_trip(&source).contains("fail %"));
}

#[test]
fn fail_requires_error_declared_on_function_and_matching_type() {
    for body in [
        "func module @Terminal.bad(%x: i32) -> i32 engine(native) {\nbb0(%x: i32):\n  fail %x\n}",
        "func module @Terminal.bad(%x: i64) -> i32 fails i32 engine(native) {\nbb0(%x: i64):\n  fail %x\n}",
        "func module @Terminal.bad(%x: i32) -> i32 fails i32 engine(native) {\nbb0(%x: i32):\n  fail %missing\n}",
        "func module @Terminal.bad(%x: i32) -> i32 fails i32 engine(native) {\nbb0(%x: i32):\n  %y = fail %x\n}",
    ] {
        let source = module("defined", body);
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "accepted invalid fail: {body}"
            );
        }
    }
}
