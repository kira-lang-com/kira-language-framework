use tir_diag::Severity;
use tir_format::{BinaryModule, assemble_text_module, parse_text_module};

use crate::{verify_binary_module_records, verify_text_module};

fn source(body: &str) -> String {
    format!(
        "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.chain(%a: i32, %b: i32, %choose: i1) -> i32 engine(native) {{\nbb0(%a: i32, %b: i32, %choose: i1):\n{body}\n}}\n"
    )
}

#[test]
fn assembled_integer_chain_is_valid_text_and_binary() {
    let text = source(
        "  %sum = add %a, %b\n  %product = mul %sum, %b\n  %result = select %choose, %product, %a\n  return %result",
    );
    let parsed = parse_text_module(&text).expect("text parses");
    let diagnostics = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(diagnostics.is_empty(), "text verifier: {diagnostics:?}");
    let binary = assemble_text_module(&parsed).expect("text assembles");
    let decoded = BinaryModule::decode(&binary).expect("binary decodes");
    let diagnostics = verify_binary_module_records(&decoded, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(diagnostics.is_empty(), "binary verifier: {diagnostics:?}");
}

#[test]
fn malformed_chain_is_rejected_before_emission() {
    let text = source(
        "  %sum = add %a, %b\n  %product = mul %sum, %b\n  %result = select %product, %a, %b\n  return %result",
    );
    let parsed = parse_text_module(&text).expect("text parses");
    assert!(assemble_text_module(&parsed).is_err());
    assert!(
        verify_text_module(&parsed, Default::default())
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    );
}

#[test]
fn v1_duplicate_definitions_are_rejected() {
    let module = parse_text_module(&source(
        "  %twice = add %a, %b\n  %twice = mul %a, %b\n  return %twice",
    ))
    .expect("text parses");
    let diagnostics = verify_text_module(&module, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.to_string() == "TIR-VFY-0001"),
        "expected V1 diagnostic: {diagnostics:?}"
    );
}

#[test]
fn v3_missing_terminator_is_rejected() {
    let module = parse_text_module(&source("  %twice = add %a, %b")).expect("text parses");
    let diagnostics = verify_text_module(&module, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.to_string() == "TIR-VFY-0003"),
        "expected V3 diagnostic: {diagnostics:?}"
    );
}

#[test]
fn v1_use_before_definition_is_rejected() {
    let module = parse_text_module(&source(
        "  %first = add %a, %later\n  %later = add %a, %b\n  return %first",
    ))
    .expect("text parses");
    let diagnostics = verify_text_module(&module, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.to_string() == "TIR-VFY-0001"),
        "expected V1 diagnostic: {diagnostics:?}"
    );
}

#[test]
fn mixed_constants_and_intermediate_comparison_verify() {
    let text = source(
        "  %five = const.int i32, 5\n  %sum = add %a, %five\n  %eq = icmp eq %sum, %b\n  %flag = const.bool true\n  %ready = and %eq, %flag\n  %final = select %ready, %sum, %b\n  return %final",
    );
    let parsed = parse_text_module(&text).expect("text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text verifier errors: {text_errors:?}"
    );
    let binary = assemble_text_module(&parsed).expect("text assembles");
    let decoded = BinaryModule::decode(&binary).expect("binary decodes");
    let binary_errors = verify_binary_module_records(&decoded, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier errors: {binary_errors:?}"
    );
}

#[test]
fn typed_multi_block_arguments_verify_as_text_and_binary() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.route(%a: i32, %cond: i1) -> i32 engine(native) {\nbb0(%a: i32, %cond: i1):\n  cond.br %cond, bb1(%a), bb2(%a)\nbb1(%left: i32):\n  br bb2(%left)\nbb2(%right: i32):\n  return %right\n}\n";
    let parsed = parse_text_module(text).expect("text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(text_errors.is_empty(), "text verifier: {text_errors:?}");
    let bytes = assemble_text_module(&parsed).expect("text assembles");
    let module = BinaryModule::decode(&bytes).expect("binary decodes");
    let binary_errors = verify_binary_module_records(&module, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier: {binary_errors:?}"
    );
}

#[test]
fn multi_block_local_operations_verify() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.route(%a: i32, %cond: i1) -> i32 engine(native) {\nbb0(%a: i32, %cond: i1):\n  %twice = add %a, %a\n  cond.br %cond, bb1(%twice), bb2(%a)\nbb1(%left: i32):\n  %four = const.int i32, 4\n  %sum = add %left, %four\n  br bb2(%sum)\nbb2(%right: i32):\n  %zero = const.int i32, 0\n  %result = add %right, %zero\n  return %result\n}\n";
    let parsed = parse_text_module(text).expect("text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(text_errors.is_empty(), "text verifier: {text_errors:?}");
    let bytes = assemble_text_module(&parsed).expect("text assembles");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let binary_errors = verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier: {binary_errors:?}"
    );
}

#[test]
fn loop_back_edge_with_block_arguments_verifies() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.route(%a: i32, %cond: i1) -> i32 engine(native) {\nbb0(%a: i32, %cond: i1):\n  br bb1(%a)\nbb1(%i: i32):\n  %one = const.int i32, 1\n  %next = add %i, %one\n  cond.br %cond, bb1(%next), bb2(%next)\nbb2(%result: i32):\n  return %result\n}\n";
    let parsed = parse_text_module(text).expect("text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(text_errors.is_empty(), "text verifier: {text_errors:?}");
    let bytes = assemble_text_module(&parsed).expect("text assembles");
    let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
    let binary_errors = verify_binary_module_records(&decoded, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier: {binary_errors:?}"
    );
}

#[test]
fn dominating_cross_block_ssa_value_verifies() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.route(%a: i32, %cond: i1) -> i32 engine(native) {\nbb0(%a: i32, %cond: i1):\n  %shared = add %a, %a\n  cond.br %cond, bb1(%shared), bb2(%a)\nbb1(%left: i32):\n  %sum = add %left, %shared\n  br bb2(%sum)\nbb2(%right: i32):\n  %answer = add %right, %shared\n  return %answer\n}\n";
    let parsed = parse_text_module(text).expect("text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text verifier errors: {text_errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("text assembles");
    let module = BinaryModule::decode(&bytes).expect("binary decodes");
    let binary_errors = verify_binary_module_records(&module, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier errors: {binary_errors:?}"
    );
}

#[test]
fn non_dominating_cross_block_ssa_value_is_v1() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.route(%a: i32, %cond: i1) -> i32 engine(native) {\nbb0(%a: i32, %cond: i1):\n  cond.br %cond, bb1(%a), bb2(%a)\nbb1(%left: i32):\n  %local = add %left, %left\n  br bb2(%local)\nbb2(%right: i32):\n  %invalid = add %right, %local\n  return %invalid\n}\n";
    let parsed = parse_text_module(text).expect("text parses");
    let diagnostics = verify_text_module(&parsed, Default::default());
    assert!(
        diagnostics.iter().any(
            |d| d.code.to_string() == "TIR-VFY-0001" && d.message.contains("does not dominate")
        ),
        "expected V1 cross-block dominance error: {diagnostics:?}"
    );
    assert!(
        assemble_text_module(&parsed).is_err(),
        "assembler must also reject non-dominating references"
    );
}

#[test]
fn module_verifier_reports_definite_initialization_codes() {
    let cases = [
        (
            "  %slot = stack.alloc i32\n  %value = load [trivial] %slot\n  return",
            "TIR-OWN-0014",
        ),
        (
            "  %slot = stack.alloc i32\n  store [init] %v to %slot\n  store [init] %v to %slot\n  return",
            "TIR-OWN-0015",
        ),
    ];
    for (body, expected_code) in cases {
        let source = format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.test(%v: i32) -> unit engine(native) {{\nbb0(%v: i32):\n{body}\n}}\n"
        );
        let module = parse_text_module(&source).expect("text parses");
        let diagnostics = verify_text_module(&module, Default::default());
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code.to_string() == expected_code),
            "expected {expected_code} from module verifier: {diagnostics:?}"
        );
    }
}

#[test]
fn v1_wrong_typed_block_edge_is_rejected() {
    let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.edge(%a: i32, %condition: i1) -> i32 engine(native) {\nbb0(%a: i32, %condition: i1):\n  br bb1(%condition)\nbb1(%value: i32):\n  return %value\n}\n";
    let parsed = parse_text_module(source).expect("text parses");
    let diagnostics = verify_text_module(&parsed, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.to_string() == "TIR-VFY-0001" && d.message.contains("type")),
        "wrong typed edge must produce V1 diagnostic: {diagnostics:?}"
    );
}

#[test]
fn structured_control_flow_requires_loop_annotation_s1() {
    let source = "func @test(%condition: i1) -> unit {\nbb0(%condition: i1):\n br bb1()\nbb1:\n cond.br %condition, bb1(), bb2() merge bb2\nbb2:\n return\n}";
    let body = tir_format::parse_function_body(source)
        .expect("text parses")
        .expect("function body");
    let diagnostics = crate::verify_text_body(
        &body,
        crate::ControlFlowOptions {
            structured: true,
            gpu: false,
        },
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.to_string() == "TIR-CF-0001"),
        "expected S1 for unannotated loop: {diagnostics:?}"
    );
}

#[test]
fn structured_selection_requires_merge_s3() {
    let source = "func @test(%condition: i1) -> unit {\nbb0(%condition: i1):\n cond.br %condition, bb1(), bb2()\nbb1:\n return\nbb2:\n return\n}";
    let body = tir_format::parse_function_body(source)
        .expect("text parses")
        .expect("function body");
    let diagnostics = crate::verify_text_body(
        &body,
        crate::ControlFlowOptions {
            structured: true,
            gpu: false,
        },
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.to_string() == "TIR-CF-0003"),
        "expected S3 for missing selection merge: {diagnostics:?}"
    );
}

#[test]
fn structured_flow_forbids_indirect_branch_s7() {
    let source = "func @test(%condition: i1) -> unit {\nbb0(%condition: i1):\n br.indirect %condition [bb1()]\nbb1:\n return\n}";
    let body = tir_format::parse_function_body(source)
        .expect("text parses")
        .expect("function body");
    let diagnostics = crate::verify_text_body(
        &body,
        crate::ControlFlowOptions {
            structured: true,
            gpu: false,
        },
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.to_string() == "TIR-CF-0007"),
        "expected S7 for indirect branch: {diagnostics:?}"
    );
}

#[test]
fn simple_structured_function_without_violations_is_accepted() {
    let source = "func @test(%condition: i1) -> unit {\nbb0(%condition: i1):\n return\n}";
    let body = tir_format::parse_function_body(source)
        .expect("text parses")
        .expect("function body");
    let diagnostics = crate::verify_text_body(
        &body,
        crate::ControlFlowOptions {
            structured: true,
            gpu: false,
        },
    );
    assert!(
        diagnostics.is_empty(),
        "simple structured return must be valid: {diagnostics:?}"
    );
}

#[test]
fn owned_unique_value_requires_consumption_v4() {
    let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.leak(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  return\n}\n";
    let parsed = parse_text_module(source).expect("text parses");
    let diagnostics = verify_text_module(&parsed, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.to_string() == "TIR-VFY-0004"),
        "leaking owned unique value should produce V4: {diagnostics:?}"
    );
}

#[test]
fn borrowed_unique_value_cannot_be_destroyed_v5() {
    let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.consume(%object: borrow uniq i32) -> unit engine(native) {\nbb0(%object: borrow uniq i32):\n  destroy %object\n  return\n}\n";
    let parsed = parse_text_module(source).expect("text parses");
    let diagnostics = verify_text_module(&parsed, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.to_string() == "TIR-VFY-0005"),
        "destroy of borrowed unique should produce V5: {diagnostics:?}"
    );
}

#[test]
fn balanced_borrow_scope_verifies_as_text_and_binary() {
    let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.scope(%a: i32) -> unit engine(native) {\nbb0(%a: i32):\n  %loan = borrow.begin %a\n  borrow.end %loan\n  return\n}\n";
    let parsed = parse_text_module(source).expect("text parses");
    let diagnostics = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(diagnostics.is_empty(), "text verifier: {diagnostics:?}");
    let bytes = assemble_text_module(&parsed).expect("text assembles");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let diagnostics = verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(diagnostics.is_empty(), "binary verifier: {diagnostics:?}");
}

#[test]
fn v6_rejects_unclosed_and_out_of_order_borrows() {
    for body in [
        "  %loan = borrow.begin %a\n  return",
        "  %first = borrow.begin %a\n  %second = borrow.begin %a\n  borrow.end %first\n  borrow.end %second\n  return",
        "  borrow.end %a\n  return",
    ] {
        let text = format!(
            "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.scope(%a: i32) -> unit engine(native) {{\nbb0(%a: i32):\n{body}\n}}\n"
        );
        let parsed = parse_text_module(&text).expect("text parses");
        let diagnostics = verify_text_module(&parsed, Default::default());
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code.to_string() == "TIR-VFY-0006"),
            "borrow scope error requires V6: {diagnostics:?}"
        );
    }
}

#[test]
fn scalar_memory_round_trip_is_valid_text_and_binary() {
    let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.memory(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %loaded = load [take] %slot\n  stack.dealloc %slot\n  return %loaded\n}\n";
    let parsed = parse_text_module(source).expect("text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(text_errors.is_empty(), "text verifier: {text_errors:?}");
    let bytes = assemble_text_module(&parsed).expect("memory CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("binary decodes");
    let binary_errors = verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verifier: {binary_errors:?}"
    );
}

#[test]
fn scalar_memory_take_then_read_is_use_before_init() {
    let source = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.memory(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %taken = load [take] %slot\n  %invalid = load [trivial] %slot\n  stack.dealloc %slot\n  return %taken\n}\n";
    let parsed = parse_text_module(source).expect("text parses");
    let diagnostics = verify_text_module(&parsed, Default::default());
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code.to_string() == "TIR-OWN-0014"),
        "load after take must be rejected: {diagnostics:?}"
    );
}
