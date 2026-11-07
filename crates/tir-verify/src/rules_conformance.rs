//! Independent T1 verifier-rule regression cases. These intentionally use
//! readable text instead of relying on an assembler accepting invalid IR.

use tir_format::parse_text_module;

use super::verify_text_module;
use super::{ControlFlowOptions, verify_text_body};

fn module(body: &str, profile: &str) -> String {
    format!(
        "module @Smoke tir 26.11.1 profile {profile} semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn codes(text: &str) -> Vec<String> {
    let parsed = parse_text_module(text).expect("TIR module text parses");
    verify_text_module(&parsed, Default::default())
        .into_iter()
        .map(|diagnostic| diagnostic.code.to_string())
        .collect()
}

fn assert_rule(text: &str, expected: &str) {
    let actual = codes(text);
    assert!(
        actual.iter().any(|code| code == expected),
        "expected {expected}: {actual:?}"
    );
}

#[test]
fn v10_invoke_is_forbidden_in_nounwind_function() {
    let text = module(
        "func module @Smoke.caller() -> unit engine(native) nounwind {\nbb0:\n  invoke @Smoke.callee() to bb1() unwind bb2()\nbb1:\n  return\nbb2:\n  return\n}",
        "app",
    );
    assert_rule(&text, "TIR-VFY-0010");
}

#[test]
fn v10_try_call_requires_failing_callee_type() {
    let text = module(
        "func module @Smoke.callee() -> unit engine(native) {\nbb0:\n  return\n}\n\nfunc module @Smoke.caller() -> unit engine(native) {\nbb0:\n  try.call @Smoke.callee() normal bb1(), error bb2()\nbb1:\n  return\nbb2:\n  return\n}",
        "app",
    );
    assert_rule(&text, "TIR-VFY-0010");
}

#[test]
fn v12_export_requires_nounwind_and_representable_signature() {
    let text = module(
        "func exported @Smoke.bad(%value: svec<i32>) -> unit engine(native) {\nbb0(%value: svec<i32>):\n  return\n}",
        "app",
    );
    assert_rule(&text, "TIR-VFY-0012");
}

#[test]
fn v12_exported_ridl_function_with_nounwind_and_simple_values_is_accepted() {
    let text = module(
        "func exported @Smoke.ok(%value: i32) -> i32 engine(native) nounwind {\nbb0(%value: i32):\n  return %value\n}",
        "app",
    );
    let actual = codes(&text);
    assert!(
        !actual.iter().any(|code| code == "TIR-VFY-0012"),
        "valid RIDL export: {actual:?}"
    );
}

#[test]
fn v13_kernel_functions_must_be_nounwind() {
    let text = module(
        "func module @Smoke.kernel() -> unit engine(native) {\nbb0:\n  return\n}",
        "kernel",
    );
    assert_rule(&text, "TIR-VFY-0013");
}

#[test]
fn v13_kernel_disallows_runtime_engine() {
    let text = module(
        "func module @Smoke.kernel() -> unit engine(runtime) nounwind {\nbb0:\n  return\n}",
        "kernel",
    );
    assert_rule(&text, "TIR-VFY-0013");
}

fn structured_codes(body: &str) -> Vec<String> {
    let parsed = tir_format::parse_function_body(body)
        .expect("structured function parses")
        .expect("structured function has a body");
    verify_text_body(
        &parsed,
        ControlFlowOptions {
            structured: true,
            gpu: false,
        },
    )
    .into_iter()
    .map(|diagnostic| diagnostic.code.to_string())
    .collect()
}

#[test]
fn s5_rejects_shared_merge_for_multiple_selections() {
    let function = "func @Smoke.structured(%cond: i1) -> unit {\nbb0(%cond: i1):\n  cond.br %cond, bb1(), bb2() merge bb4\nbb1:\n  cond.br %cond, bb3(), bb4() merge bb4\nbb3:\n  br bb4()\nbb2:\n  br bb4()\nbb4:\n  return\n}";
    let actual = structured_codes(function);
    assert!(
        actual.contains(&"TIR-CF-0005".to_owned()),
        "S5 shared merge: {actual:?}"
    );
}

#[test]
fn s6_rejects_edge_entering_selection_from_merge() {
    let function = "func @Smoke.structured(%cond: i1) -> unit {\nbb0(%cond: i1):\n  cond.br %cond, bb1(), bb4() merge bb4\nbb1:\n  br bb2()\nbb2:\n  br bb4()\nbb4:\n  br bb2()\n}";
    let actual = structured_codes(function);
    assert!(
        actual.contains(&"TIR-CF-0006".to_owned()),
        "S6 illegal construct entry: {actual:?}"
    );
}

#[test]
fn v2_scalar_float_arithmetic_and_comparison_verify_in_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.float(%a: f32, %b: f32) -> f32 engine(native) {\nbb0(%a: f32, %b: f32):\n  %sum = fadd %a, %b\n  %less = fcmp olt %sum, %b\n  %result = select %less, %sum, %b\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("float text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text verification failed: {text_errors:?}"
    );
    let encoded = assemble_text_module(&parsed).expect("float CODE assembles");
    let decoded = BinaryModule::decode(&encoded).expect("binary decodes");
    let binary_errors = super::verify_binary_module_records(&decoded, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary verification failed: {binary_errors:?}"
    );
}

#[test]
fn v2_float_vector_mask_shape_verifies_in_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.float(%a: vec<4 x f32>, %b: vec<4 x f32>) -> vec<4 x f32> engine(native) {\nbb0(%a: vec<4 x f32>, %b: vec<4 x f32>):\n  %sum = fmul %a, %b\n  %mask = fcmp olt %sum, %b\n  %result = select %mask, %sum, %b\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("text parses");
    let bytes = assemble_text_module(&parsed).expect("vector float CODE assembles");
    let binary = BinaryModule::decode(&bytes).expect("CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "vector floating-point V2 verification: {errors:?}"
    );
}

#[test]
fn v2_integer_vector_mask_shape_verifies_in_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.vector(%a: vec<4 x i32>, %b: vec<4 x i32>) -> vec<4 x i32> engine(native) {\nbb0(%a: vec<4 x i32>, %b: vec<4 x i32>):\n  %sum = add %a, %b\n  %mask = icmp ult %sum, %b\n  %result = select %mask, %sum, %a\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("text parses");
    let encoded = assemble_text_module(&parsed).expect("integer-vector CODE assembles");
    let binary = BinaryModule::decode(&encoded).expect("binary decodes");
    let diagnostics = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(diagnostics.is_empty(), "integer vector V2: {diagnostics:?}");
}

#[test]
fn v2_vector_splat_extract_reverse_and_step_verify_in_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    for (body, params, result) in [
        (
            "  %vec = vec.splat %a to vec<4 x i32>\n  %rev = vec.reverse %vec\n  %elt = vec.extract %rev, %a\n  return %elt",
            "%a: i32",
            "i32",
        ),
        (
            "  %vec = vec.step vec<4 x i32>\n  %back = vec.reverse %vec\n  return %back",
            "",
            "vec<4 x i32>",
        ),
        (
            "  %vec = vec.splat %a to vec<4 x f32>\n  %rev = vec.reverse %vec\n  return %rev",
            "%a: f32",
            "vec<4 x f32>",
        ),
    ] {
        let text = module(
            &format!(
                "func module @Smoke.vector({params}) -> {result} engine(native) {{\nbb0({params}):\n{body}\n}}"
            ),
            "app",
        );
        let parsed = parse_text_module(&text).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("vector CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let errors = super::verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "vector V2 rejected valid operations: {errors:?}"
        );
    }
}

#[test]
fn v2_numeric_conversions_verify_in_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    for (params, result, body) in [
        (
            "%a: i64",
            "i32",
            "  %result = trunc %a to i32\n  return %result",
        ),
        (
            "%a: f64",
            "i32",
            "  %result = fptosi [sat] %a to i32\n  return %result",
        ),
        (
            "%a: f32",
            "f16",
            "  %result = fcvt %a to f16 [rne] [satfinite]\n  return %result",
        ),
        (
            "%a: vec<4 x i64>",
            "vec<4 x i32>",
            "  %result = trunc %a to vec<4 x i32>\n  return %result",
        ),
    ] {
        let text = module(
            &format!(
                "func module @Smoke.convert({params}) -> {result} engine(native) {{\nbb0({params}):\n{body}\n}}"
            ),
            "app",
        );
        let parsed = parse_text_module(&text).expect("text parses");
        let bytes = assemble_text_module(&parsed).expect("conversion CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let errors = super::verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "conversion V2 rejected valid operations: {errors:?}"
        );
    }
}

#[test]
fn v4_owned_unique_branch_parameter_transfer_is_valid() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.transfer(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  br bb1(%object)\nbb1(%next: owned uniq i32):\n  destroy %next\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("owned unique forwarding parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text owned branch transfer verification: {text_errors:?}"
    );
    let bytes = assemble_text_module(&parsed).expect("owned branch transfer assembles");
    let decoded = BinaryModule::decode(&bytes).expect("binary decodes");
    let binary_errors = super::verify_binary_module_records(&decoded, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary owned branch transfer verification: {binary_errors:?}"
    );
}

#[test]
fn v2_freeze_accepts_scalar_float_pointer_and_fixed_vector() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let cases = [
        ("f32", "permissive"),
        ("ptr<device>", "defined"),
        ("vec<4 x i32>", "defined"),
    ];
    for (ty, semantics) in cases {
        let text = format!(
            "module @Smoke tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.freeze(%v: {ty}) -> {ty} engine(native) {{\nbb0(%v: {ty}):\n  %safe = freeze %v\n  return %safe\n}}\n"
        );
        let parsed = parse_text_module(&text).expect("TIR text parses");
        let bytes = assemble_text_module(&parsed).expect("freeze CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let errors = super::verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            errors.is_empty(),
            "V2 rejects valid freeze of {ty}: {errors:?}"
        );
    }
}

#[test]
fn v2_extended_integer_operations_verify_in_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let binary_ops = [
        "sdiv", "udiv", "srem", "urem", "shl", "lshr", "ashr", "rotl", "rotr", "smin", "smax",
        "umin", "umax", "clmul", "clmulh",
    ];
    for ty in ["i32", "i64", "vec<4 x i32>"] {
        for opcode in binary_ops {
            if ty.starts_with("vec<") && matches!(opcode, "clmul" | "clmulh") {
                continue;
            }
            let source = module(
                &format!(
                    "func module @Smoke.integer(%a: {ty}, %b: {ty}) -> {ty} engine(native) {{\nbb0(%a: {ty}, %b: {ty}):\n  %value = {opcode} %a, %b\n  return %value\n}}"
                ),
                "app",
            );
            let parsed = parse_text_module(&source).expect("integer text parses");
            let binary = assemble_text_module(&parsed).expect("integer CODE assembles");
            let decoded = BinaryModule::decode(&binary).expect("binary decodes");
            let errors = super::verify_binary_module_records(&decoded, u64::MAX)
                .into_iter()
                .filter(|diagnostic| diagnostic.severity == Severity::Error)
                .collect::<Vec<_>>();
            assert!(errors.is_empty(), "V2 {opcode} on {ty}: {errors:?}");
        }
    }

    for ty in ["i32", "vec<4 x i32>"] {
        for opcode in ["fshl", "fshr"] {
            let source = module(
                &format!(
                    "func module @Smoke.funnel(%a: {ty}, %b: {ty}) -> {ty} engine(native) {{\nbb0(%a: {ty}, %b: {ty}):\n  %value = {opcode} %a, %b, %a\n  return %value\n}}"
                ),
                "app",
            );
            let parsed = parse_text_module(&source).expect("funnel text parses");
            let bytes = assemble_text_module(&parsed).expect("funnel CODE assembles");
            let decoded = BinaryModule::decode(&bytes).expect("funnel binary decodes");
            let errors = super::verify_binary_module_records(&decoded, u64::MAX)
                .into_iter()
                .filter(|diagnostic| diagnostic.severity == Severity::Error)
                .collect::<Vec<_>>();
            assert!(errors.is_empty(), "V2 {opcode} on {ty}: {errors:?}");
        }
    }
}

#[test]
fn v14_integer_exact_disjoint_and_poison_require_permissive_semantics() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    for instruction in [
        "udiv [exact] %a, %b",
        "or [disjoint] %a, %b",
        "ctlz [poison] %a",
        "cttz [poison] %a",
    ] {
        let function = format!(
            "func module @Smoke.flags(%a: i32, %b: i32) -> i32 engine(native) {{\nbb0(%a: i32, %b: i32):\n  %result = {instruction}\n  return %result\n}}"
        );
        let defined = module(&function, "app");
        let parsed = parse_text_module(&defined).expect("defined text parses");
        assert!(
            verify_text_module(&parsed, Default::default())
                .iter()
                .any(|d| d.code.to_string() == "TIR-VFY-0014"),
            "text V14 missed {instruction}"
        );
        let bytes =
            assemble_text_module(&parsed).expect("defined CODE is valid for separate verification");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        assert!(
            super::verify_binary_module_records(&binary, u64::MAX)
                .iter()
                .any(|d| d.code.to_string() == "TIR-VFY-0014"),
            "binary V14 missed {instruction}"
        );

        let permissive = defined.replace("semantics defined", "semantics permissive");
        let parsed = parse_text_module(&permissive).expect("permissive text parses");
        let bytes = assemble_text_module(&parsed).expect("permissive CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("binary decodes");
        let diagnostics = super::verify_binary_module_records(&binary, u64::MAX)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .collect::<Vec<_>>();
        assert!(
            diagnostics.is_empty(),
            "permissive {instruction}: {diagnostics:?}"
        );
    }
}

#[test]
fn v14_assume_is_permissive_only_in_text_and_binary() {
    use tir_format::{BinaryModule, assemble_text_module};
    let function = "func module @Smoke.assume(%condition: i1) -> unit engine(native) {\nbb0(%condition: i1):\n  assume %condition\n  return\n}";
    for (mode, must_fail) in [("defined", true), ("permissive", false)] {
        let source = format!(
            "module @Smoke tir 26.11.1 profile app semantics {mode}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{function}\n"
        );
        let parsed = parse_text_module(&source).expect("assume module parses");
        let text_errors = verify_text_module(&parsed, Default::default());
        assert_eq!(
            text_errors
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0014"),
            must_fail,
            "V14 text mismatch for {mode}: {text_errors:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("assume CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("assume CODE decodes");
        let diagnostics = super::verify_binary_module_records(&binary, u64::MAX);
        assert_eq!(
            diagnostics
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0014"),
            must_fail,
            "V14 binary mismatch for {mode}: {diagnostics:?}"
        );
    }
}

#[test]
fn v2_target_queries_expect_and_portable_intrinsics_are_well_typed() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.hints(%input: i32, %condition: i1) -> i64 engine(native) {\nbb0(%input: i32, %condition: i1):\n  %hint = expect %input, -3 [probability 0.75]\n  %feature = target.has \"atomic128.lockfree\"\n  %pages = target.pagesize\n  intrinsic 11() -> unit\n  %counter = intrinsic 12() -> i64\n  intrinsic 14() -> unit\n  %result = add %counter, %pages\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("hints source parses");
    let bytes = assemble_text_module(&parsed).expect("hints CODE encodes");
    let module = BinaryModule::decode(&bytes).expect("hints CODE decodes");
    let diagnostics = super::verify_binary_module_records(&module, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        diagnostics.is_empty(),
        "V2 rejects valid hints and queries: {diagnostics:?}"
    );
}

#[test]
fn v2_trivial_tuple_construction_and_extraction_verify() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.tuple(%a: i32, %b: f32) -> i32 engine(native) {\nbb0(%a: i32, %b: f32):\n  %pair = tuple.new (%a, %b)\n  %result = tuple.extract %pair, 0\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("tuple text parses");
    let encoded = assemble_text_module(&parsed).expect("tuple CODE assembles");
    let binary = BinaryModule::decode(&encoded).expect("tuple binary decodes");
    let diagnostics = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        diagnostics.is_empty(),
        "V2 rejected trivial tuple CODE: {diagnostics:?}"
    );
}

#[test]
fn v2_trivial_fixed_array_construction_insertion_and_extraction_verify() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.array(%a: i32, %b: i32, %index: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32, %index: i32):\n  %array = array.new [3 x i32](%a, %b, %a)\n  %replaced = array.insert %array, %index, %b\n  %result = array.extract %replaced, %index\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("array text parses");
    let encoded = assemble_text_module(&parsed).expect("array CODE encodes");
    let decoded = BinaryModule::decode(&encoded).expect("array CODE decodes");
    let diagnostics = super::verify_binary_module_records(&decoded, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        diagnostics.is_empty(),
        "V2 rejected trivial array CODE: {diagnostics:?}"
    );
}

#[test]
fn v2_pointer_arithmetic_and_raw_memory_operations_are_verified() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let function = "func module @Smoke.raw(%dst: ptr, %src: ptr, %count: i64, %byte: i8) -> i64 engine(native) {\nbb0(%dst: ptr, %src: ptr, %count: i64, %byte: i8):\n  %offset = ptr.offset %dst, %count\n  %difference = ptr.diff %offset, %src\n  mem.copy %dst, %src, %count\n  mem.move %dst, %src, %count\n  mem.set %dst, %byte, %count\n  prefetch %dst [write] [locality 3] [instruction]\n  return %difference\n}";
    let source = module(function, "app");
    let parsed = parse_text_module(&source).expect("raw memory text parses");
    let bytes = assemble_text_module(&parsed).expect("raw CODE encodes");
    let binary = BinaryModule::decode(&bytes).expect("raw CODE decodes");
    let diagnostics = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        diagnostics.is_empty(),
        "V2 rejected valid pointer/memory CODE: {diagnostics:?}"
    );
}

#[test]
fn v14_raw_pointer_inbounds_and_nooverlap_are_permissive_only() {
    use tir_format::{BinaryModule, assemble_text_module};
    let function = "func module @Smoke.raw(%dst: ptr, %src: ptr, %count: i64) -> unit engine(native) {\nbb0(%dst: ptr, %src: ptr, %count: i64):\n  %offset = ptr.offset [inbounds] %dst, %count\n  mem.copy %offset, %src, %count [nooverlap]\n  return\n}";
    for (mode, expected_rejection) in [("defined", true), ("permissive", false)] {
        let source = format!(
            "module @Smoke tir 26.11.1 profile app semantics {mode}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{function}\n"
        );
        let parsed = parse_text_module(&source).expect("pointer semantics text parses");
        let text = verify_text_module(&parsed, Default::default());
        assert_eq!(
            text.iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0014"),
            expected_rejection,
            "text V14 mismatch for {mode}: {text:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("pointer CODE encodes");
        let binary = BinaryModule::decode(&bytes).expect("pointer CODE decodes");
        let issues = super::verify_binary_module_records(&binary, u64::MAX);
        assert_eq!(
            issues
                .iter()
                .any(|issue| issue.code.to_string() == "TIR-VFY-0014"),
            expected_rejection,
            "binary V14 mismatch for {mode}: {issues:?}"
        );
    }
}

#[test]
fn v2_v20_typed_raw_load_and_store_with_alignment_verify_in_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let text = module(
        "func module @Smoke.raw(%p: ptr, %value: i32) -> i32 engine(native) {\nbb0(%p: ptr, %value: i32):\n  raw.store %value, %p [align 4] [volatile] [nontemporal]\n  %loaded = raw.load i32, %p [align 4] [volatile] [nontemporal]\n  return %loaded\n}",
        "app",
    );
    let parsed = parse_text_module(&text).expect("raw access text parses");
    let encoded = assemble_text_module(&parsed).expect("raw access CODE assembles");
    let module = BinaryModule::decode(&encoded).expect("binary decodes");
    let analysis = module.analysis_summary(u64::MAX).expect("ANALYSIS decodes");
    assert_eq!(analysis.functions[0].trust_class, 2);
    assert_eq!(analysis.functions[0].reason_mask & 0x02, 0x02);
    let diagnostics = super::verify_binary_module_records(&module, u64::MAX)
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        diagnostics.is_empty(),
        "raw access has V2 or V20 errors: {diagnostics:?}"
    );
}

#[test]
fn v14_unreachable_ub_is_permissive_only_in_text_and_binary() {
    use tir_format::{BinaryModule, assemble_text_module};

    let body = "func module @Smoke.terminal() -> i32 engine(native) {\nbb0:\n  unreachable [ub]\n}";
    for (semantics, forbidden) in [("defined", true), ("permissive", false)] {
        let text = format!(
            "module @Smoke tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
        );
        let parsed = parse_text_module(&text).expect("unreachable text parses");
        let text_diagnostics = verify_text_module(&parsed, Default::default());
        assert_eq!(
            text_diagnostics
                .iter()
                .any(|d| d.code.to_string() == "TIR-VFY-0014"),
            forbidden,
            "V14 text mismatch for {semantics}: {text_diagnostics:?}"
        );
        let bytes = assemble_text_module(&parsed).expect("unreachable CODE assembles");
        let binary = BinaryModule::decode(&bytes).expect("unreachable CODE decodes");
        let diagnostics = super::verify_binary_module_records(&binary, u64::MAX);
        assert_eq!(
            diagnostics
                .iter()
                .any(|d| d.code.to_string() == "TIR-VFY-0014"),
            forbidden,
            "V14 binary mismatch for {semantics}: {diagnostics:?}"
        );
    }
}
