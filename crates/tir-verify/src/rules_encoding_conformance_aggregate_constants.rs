//! V2 typed aggregate CONSTANTS records and CODE references.
use super::verify_text_module;
use tir_format::parse_text_module;

#[test]
fn v2_tuple_and_fixed_array_const_aggregate_check_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = "module @Aggregates tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Aggregates.tuple() -> (i32, f64, ptr) engine(native) {\nbb0:\n  %aggregate = const.aggregate (i32, f64, ptr)(42, 1.5, null)\n  return %aggregate\n}\n\nfunc module @Aggregates.array() -> [4 x i32] engine(native) {\nbb0:\n  %aggregate = const.aggregate [4 x i32](1, -2, 3, 4)\n  return %aggregate\n}\n";
    let parsed = parse_text_module(source).expect("aggregate constant source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejected aggregate constants: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("aggregate constant CODE assembles"),
    )
    .expect("aggregate constant TIRB decodes");
    let constants = binary
        .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
        .expect("aggregate CONSTANTS decodes");
    assert_eq!(constants.records.len(), 2);
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejected aggregate constants: {binary_errors:?}"
    );
}

#[test]
fn v2_mixed_float_vector_and_aggregate_constant_index_validation() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = "module @Aggregates tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nglobal module @Aggregates.counter: i32 = 7\n\nfunc module @Aggregates.mixed() -> (i64, f32) engine(native) {\nbb0:\n  %vector = const.vec vec<8 x i32>, [1, 2, 3, 4, 5, 6, 7, 8]\n  %float = const.float f64, 2.0\n  %aggregate = const.aggregate (i64, f32)(-1, 1.5)\n  return %aggregate\n}\n";
    let parsed = parse_text_module(source).expect("mixed constants source parses");
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("mixed constants CODE assembles"),
    )
    .expect("mixed constants TIRB decodes");
    assert_eq!(
        binary
            .constant_table(u64::MAX, &binary.type_table(u64::MAX).unwrap())
            .expect("mixed CONSTANTS decodes")
            .records
            .len(),
        4
    );
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejected mixed CONSTANTS indexes: {binary_errors:?}"
    );
}

#[test]
fn v2_nested_tuple_and_array_constants_are_typed_recursively() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = "module @Aggregates tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Aggregates.nested() -> ((i8, f32), [2 x i32]) engine(native) {\nbb0:\n  %literal = const.aggregate ((i8, f32), [2 x i32])((7, 1.5), (-2, 48))\n  return %literal\n}\n";
    let parsed = parse_text_module(source).expect("nested aggregate source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "nested aggregate text V2 rejected: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("nested aggregate CODE assembles"),
    )
    .expect("nested aggregate TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "nested aggregate binary V2 rejected: {binary_errors:?}"
    );
}

#[test]
fn v2_trivial_nominal_struct_aggregate_constants_verify() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = "module @Aggregates tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\ntype %Record = struct ridl { count: i32, weight: f64 } policy default unmanaged\n\nfunc module @Aggregates.record() -> %Record engine(native) {\nbb0:\n  %literal = const.aggregate %Record(7, 1.5)\n  return %literal\n}\n\nfunc module @Aggregates.nested() -> (%Record, i32) engine(native) {\nbb0:\n  %literal = const.aggregate (%Record, i32)((7, 1.5), 3)\n  return %literal\n}\n";
    let parsed = parse_text_module(source).expect("nominal const.aggregate source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "nominal aggregate text V2 rejected: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("nominal aggregate CODE assembles"),
    )
    .expect("nominal aggregate TIRB decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "nominal aggregate binary V2 rejected: {binary_errors:?}"
    );
}

#[test]
fn v2_explicit_union_active_member_aggregate_constants_verify() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};
    let source = "module @Aggregates tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\ntype %Choice = explicit explicit(size 8, align 8) { bits: i64 @0, fraction: f64 @0 } policy default unmanaged\n\nfunc module @Aggregates.union() -> %Choice engine(native) {\nbb0:\n  %literal = const.aggregate %Choice(1, 1.5)\n  return %literal\n}\n";
    let parsed = parse_text_module(source).expect("explicit union source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "V2 text rejected explicit union constant: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("explicit union CODE assembles"),
    )
    .expect("explicit union TIRB decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2 binary rejected explicit union constant: {errors:?}"
    );
}

#[test]
fn v2_explicit_union_nested_member_types_verify_binary_and_text() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};
    let source = "module @Aggregates tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\ntype %Packet = explicit explicit(size 16, align 8) { pair: (i64, i32) @0, words: [4 x i32] @0 } policy default unmanaged\n\nfunc module @Aggregates.union() -> %Packet engine(native) {\nbb0:\n  %literal = const.aggregate %Packet(1, (1, 2, 3, 4))\n  return %literal\n}\n";
    let parsed = parse_text_module(source).expect("explicit union with aggregate member parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2 text rejects explicit union aggregate member: {errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("explicit union aggregate member CODE assembles"),
    )
    .expect("explicit union TIRB decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "V2 binary rejects explicit union aggregate member: {errors:?}"
    );
}

#[test]
fn v2_explicit_layout_rejects_malformed_size_align_delimiters() {
    use tir_diag::Severity;

    for layout in [
        "explicit(size 8 align 8)",
        "explicit(size 8, align 8]",
        "explicit(8, align 8)",
        "explicit(size 8, alignment 8)",
    ] {
        let source = format!(
            "module @Aggregates tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\ntype %Choice = explicit {layout} {{ bits: i64 @0 }} policy default unmanaged\n"
        );
        if let Ok(module) = parse_text_module(&source) {
            let errors = verify_text_module(&module, Default::default())
                .into_iter()
                .filter(|issue| issue.severity == Severity::Error)
                .collect::<Vec<_>>();
            assert!(
                !errors.is_empty(),
                "V2 accepted invalid explicit layout {layout}"
            );
        }
    }
}
