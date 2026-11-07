use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.route(%a: i32, %cond: i1) -> i32 engine(native) {{\n{body}\n}}\n"
    )
}

#[test]
fn multi_block_parameters_round_trip() {
    let source = module(
        "bb0(%a: i32, %cond: i1):\n  cond.br %cond, bb1(%a), bb2(%a)\nbb1(%left: i32):\n  br bb2(%left)\nbb2(%right: i32):\n  return %right",
    );
    let parsed = parse_text_module(&source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("block arguments assemble");
    let module = BinaryModule::decode(&bytes).expect("binary decodes");
    let text = disassemble_text_module(&module, u64::MAX).expect("binary disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&text).expect("text reparses"))
        .expect("canonical text reassembles");
    assert_eq!(bytes, rebuilt);
}

#[test]
fn arithmetic_inside_multiple_blocks_round_trips() {
    let source = module(
        "bb0(%a: i32, %cond: i1):\n  %twice = add %a, %a\n  cond.br %cond, bb1(%twice), bb2(%a)\nbb1(%left: i32):\n  %four = const.int i32, 4\n  %sum = add %left, %four\n  br bb2(%sum)\nbb2(%right: i32):\n  %zero = const.int i32, 0\n  %result = add %right, %zero\n  return %result",
    );
    let parsed = parse_text_module(&source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("multi-block operations assemble");
    let module = BinaryModule::decode(&bytes).expect("binary decodes");
    let text = disassemble_text_module(&module, u64::MAX).expect("binary disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&text).expect("text reparses"))
        .expect("canonical text reassembles");
    assert_eq!(bytes, rebuilt);
}

#[test]
fn loop_back_edge_preserves_ssa_numbering() {
    let source = module(
        "bb0(%a: i32, %cond: i1):\n  br bb1(%a)\nbb1(%i: i32):\n  %one = const.int i32, 1\n  %next = add %i, %one\n  cond.br %cond, bb1(%next), bb2(%next)\nbb2(%result: i32):\n  return %result",
    );
    let parsed = parse_text_module(&source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("loop assembles");
    let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
    let canonical_text = disassemble_text_module(&decoded, u64::MAX).expect("loop disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&canonical_text).expect("text reparses"))
        .expect("canonical text reassembles");
    assert_eq!(bytes, rebuilt);
}

#[test]
fn dominating_cross_block_value_round_trips() {
    let source = module(
        "bb0(%a: i32, %cond: i1):\n  %shared = add %a, %a\n  cond.br %cond, bb1(%shared), bb2(%a)\nbb1(%left: i32):\n  %one = const.int i32, 1\n  %sum = add %left, %shared\n  br bb2(%sum)\nbb2(%right: i32):\n  %answer = add %right, %shared\n  return %answer",
    );
    let parsed = parse_text_module(&source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("dominating definitions assemble");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("binary disassembles");
    let reconstructed =
        assemble_text_module(&parse_text_module(&canonical).expect("text reparses"))
            .expect("canonical text reassembles");
    assert_eq!(bytes, reconstructed);
}

#[test]
fn non_dominating_cross_block_value_is_rejected() {
    let source = module(
        "bb0(%a: i32, %cond: i1):\n  cond.br %cond, bb1(%a), bb2(%a)\nbb1(%left: i32):\n  %local = add %left, %left\n  br bb2(%local)\nbb2(%right: i32):\n  %invalid = add %right, %local\n  return %invalid",
    );
    let parsed = parse_text_module(&source).expect("text parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "a value from a non-dominating sibling block must not assemble"
    );
}

#[test]
fn entry_parameters_do_not_dominate_disconnected_components() {
    let source = module("bb0(%a: i32, %cond: i1):\n  return %a\nbb1:\n  return %a");
    let parsed = parse_text_module(&source).expect("text parses");
    assert!(
        assemble_text_module(&parsed).is_err(),
        "entry parameter cannot be referenced from a disconnected CFG component"
    );
}

#[test]
fn explicit_trivial_ownership_in_multiple_blocks_round_trips() {
    let source = module(
        "bb0(%a: i32, %cond: i1):\n  %duplicate = copy %a\n  cond.br %cond, bb1(%duplicate), bb2(%a)\nbb1(%left: i32):\n  %taken = move %left\n  br bb2(%taken)\nbb2(%right: i32):\n  destroy %a\n  return %right",
    );
    let parsed = parse_text_module(&source).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("explicit ownership assembles");
    let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
    let text = disassemble_text_module(&decoded, u64::MAX).expect("binary disassembles");
    let rebuilt = assemble_text_module(&parse_text_module(&text).expect("text reparses"))
        .expect("canonical text reassembles");
    assert_eq!(bytes, rebuilt);
}

#[test]
fn multi_block_argument_types_and_arity_are_checked() {
    let invalid = [
        "bb0(%a: i32, %cond: i1):\n  br bb1(%cond)\nbb1(%other: i32):\n  return %other",
        "bb0(%a: i32, %cond: i1):\n  br bb1()\nbb1(%other: i32):\n  return %other",
        "bb0(%a: i32, %cond: i1):\n  br bb1(%a)\nbb1(%other: i32):\n  return %undefined",
    ];
    for body in invalid {
        let parsed = parse_text_module(&module(body)).expect("text parses");
        assert!(
            assemble_text_module(&parsed).is_err(),
            "invalid branch assembled: {body}"
        );
    }
}
