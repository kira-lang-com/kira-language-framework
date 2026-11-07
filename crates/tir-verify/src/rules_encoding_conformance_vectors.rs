//! V2/V8 vector constant and scalable-mask CODE conformance.
use super::verify_text_module;
use tir_format::parse_text_module;

fn module(body: &str, profile: &str) -> String {
    format!(
        "module @Smoke tir 26.11.1 profile {profile} semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn v8_scalable_mask_active_raw_code_fails_closed_until_spec_conflict_resolved() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.mask(%start: i64, %end: i64) -> i64 engine(native) {\nbb0(%start: i64, %end: i64):\n  %mask = vec.mask.active %start, %end\n  %count = vec.mask.count %mask\n  %first = vec.mask.first %mask\n  %sum = add %count, %first\n  return %sum\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("scalable mask source parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("address-only")),
        "V8 must reject svec<i1> as an SSA value: {errors:?}"
    );
    let binary = BinaryModule::decode(&assemble_text_module(&parsed).expect("mask CODE assembles"))
        .expect("mask TIRB decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors
            .iter()
            .any(|issue| issue.message.contains("address-only")),
        "binary V8 must likewise reject scalable mask SSA values: {errors:?}"
    );
}

#[test]
fn v2_inline_integer_const_vec_has_typed_text_and_binary_code() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.vector() -> vec<4 x i32> engine(native) {\nbb0:\n  %v = const.vec vec<4 x i32>, [-1, 2, 3, 4]\n  return %v\n}\n\nfunc module @Smoke.lanes() -> vec<3 x i32> engine(native) {\nbb0:\n  %v = const.vec vec<3 x i32>, [1, 2, 3]\n  return %v\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("const.vec source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects inline const.vec: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("const.vec CODE assembles"))
            .expect("const.vec binary decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2 rejects inline const.vec: {errors:?}"
    );
}

#[test]
fn v2_large_const_vec_uses_constants_table_without_disturbing_float_indices() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.large() -> vec<8 x i32> engine(native) {\nbb0:\n  %v = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  return %v\n}\n\nfunc module @Smoke.float() -> f32 engine(native) {\nbb0:\n  %v = const.float f32, 1.5\n  return %v\n}\n\nfunc module @Smoke.other() -> vec<3 x i64> engine(native) {\nbb0:\n  %v = const.vec vec<3 x i64>, [1, -1, 2]\n  return %v\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("large const.vec with float source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2 text rejects large const.vec: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("large const.vec CODE assembles"),
    )
    .expect("large const.vec binary decodes");
    assert_eq!(
        binary
            .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
            .expect("CONSTANTS section decodes")
            .records
            .len(),
        3,
    );
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects large const.vec: {binary_errors:?}"
    );
}

#[test]
fn v2_mixed_global_float_and_large_vector_constant_sections_verify() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "global module @Smoke.seed: i32 = 7\n\nfunc module @Smoke.mixed() -> vec<8 x i32> engine(native) {\nbb0:\n  %float = const.float f32, 2.0\n  %large = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  return %large\n}\n\nfunc module @Smoke.other() -> vec<8 x i32> engine(native) {\nbb0:\n  %large = const.vec vec<8 x i32>, [8, 7, 6, 5, 4, 3, 2, 1]\n  return %large\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("mixed constants text parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "mixed constants text verification failed: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("mixed constants CODE assembles"),
    )
    .expect("mixed constants binary decodes");
    let constant_table = binary
        .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
        .expect("mixed constants section decodes");
    assert_eq!(constant_table.records.len(), 4);
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "mixed constants binary verification failed: {errors:?}"
    );
}

#[test]
fn v2_ieee_float_vec_inline_and_constants_backed_verify() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.inline() -> vec<4 x f32> engine(native) {\nbb0:\n  %v = const.vec vec<4 x f32>, [1.5, -2.0, 0.0, 4.0]\n  return %v\n}\n\nfunc module @Smoke.large() -> vec<3 x f64> engine(native) {\nbb0:\n  %v = const.vec vec<3 x f64>, [1.5, -2.0, 4.0]\n  return %v\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("float vector source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2 text rejects IEEE const.vec: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("IEEE const.vec CODE assembles"),
    )
    .expect("IEEE const.vec TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "V2 binary rejects IEEE const.vec: {binary_errors:?}"
    );
}

#[test]
fn v2_packed_i1_vector_mask_verifies_inline_and_constants_forms() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let bits = (0..32)
        .map(|lane| if lane % 4 == 0 { "1" } else { "0" })
        .collect::<Vec<_>>()
        .join(", ");
    let source = module(
        &format!(
            "func module @Smoke.smallmask() -> vec<4 x i1> engine(native) {{\nbb0:\n  %mask = const.vec vec<4 x i1>, [1, 0, 1, 0]\n  return %mask\n}}\n\nfunc module @Smoke.bigmask() -> vec<32 x i1> engine(native) {{\nbb0:\n  %mask = const.vec vec<32 x i1>, [{bits}]\n  return %mask\n}}"
        ),
        "app",
    );
    let parsed = parse_text_module(&source).expect("packed mask source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "packed i1 mask text V2 rejected: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("packed mask CODE assembles"))
            .expect("packed mask TIRB decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "packed i1 mask binary V2 rejected: {errors:?}"
    );
}
