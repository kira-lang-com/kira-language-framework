use klf_diag::Diagnostic;
use klf_kir::parse_module;
use klf_kir_verify::verify_module;

fn verify(source: &str) -> Vec<Diagnostic> {
    verify_module(&parse_module(source).unwrap())
}

#[test]
fn normative_geometry_example_round_trips_and_verifies() {
    let source = include_str!("fixtures/geometry-example.kir");
    let module = parse_module(source).unwrap();
    assert_eq!(verify_module(&module), [] as [klf_diag::Diagnostic; 0]);
    let reparsed = parse_module(&module.to_string()).unwrap();
    assert_eq!(module, reparsed);
}

#[test]
fn section_12_constants_round_trip_and_verify() {
    let source = include_str!("fixtures/constants-positive.kir");
    let module = parse_module(source).unwrap();
    assert_eq!(verify_module(&module), [] as [klf_diag::Diagnostic; 0]);
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn section_12_calls_round_trip_and_verify() {
    let source = include_str!("fixtures/call-positive.kir");
    let module = parse_module(source).unwrap();
    assert_eq!(verify_module(&module), [] as [klf_diag::Diagnostic; 0]);
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn call_rejects_wrong_argument_count() {
    assert_ne!(
        verify(include_str!("fixtures/call-negative.kir")),
        [] as [klf_diag::Diagnostic; 0]
    );
}

#[test]
fn section_12_conversions_round_trip_and_verify() {
    let source = include_str!("fixtures/convert-positive.kir");
    let module = parse_module(source).unwrap();
    assert_eq!(verify_module(&module), [] as [klf_diag::Diagnostic; 0]);
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn conversion_rejects_mismatched_operand() {
    assert_ne!(
        verify(include_str!("fixtures/convert-negative.kir")),
        [] as [klf_diag::Diagnostic; 0]
    );
}

#[test]
fn section_12_memory_round_trips_and_verifies() {
    let source = include_str!("fixtures/memory-positive.kir");
    let module = parse_module(source).unwrap();
    assert_eq!(verify_module(&module), [] as [klf_diag::Diagnostic; 0]);
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn store_rejects_mismatched_pointee() {
    assert_ne!(
        verify(include_str!("fixtures/memory-negative.kir")),
        [] as [klf_diag::Diagnostic; 0]
    );
}

#[test]
fn section_12_switch_round_trips_and_verifies() {
    let source = include_str!("fixtures/switch-positive.kir");
    let module = parse_module(source).unwrap();
    assert_eq!(verify_module(&module), [] as [klf_diag::Diagnostic; 0]);
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v1_accepts_dominating_values_and_matching_block_arguments() {
    assert_eq!(
        verify(include_str!("fixtures/v1-positive.kir")),
        [] as [klf_diag::Diagnostic; 0]
    );
}

#[test]
fn v1_rejects_non_dominating_use() {
    let diagnostics = verify(include_str!("fixtures/v1-negative.kir"));
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "KLF-KIR-1001")
    );
}

#[test]
fn v2_accepts_well_typed_instructions() {
    assert_eq!(
        verify(include_str!("fixtures/v2-positive.kir")),
        [] as [klf_diag::Diagnostic; 0]
    );
}

#[test]
fn v2_rejects_mismatched_operand_types() {
    let diagnostics = verify(include_str!("fixtures/v2-negative.kir"));
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "KLF-KIR-1002")
    );
}

#[test]
fn v2_rejects_out_of_range_struct_field() {
    let diagnostics = verify(include_str!("fixtures/struct-extract-negative.kir"));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "KLF-KIR-1002" && diagnostic.message.contains("struct.extract")
    }));
}

#[test]
fn v3_accepts_one_final_terminator() {
    assert_eq!(
        verify(include_str!("fixtures/v3-positive.kir")),
        [] as [klf_diag::Diagnostic; 0]
    );
}

#[test]
fn v3_rejects_unterminated_block() {
    let diagnostics = verify(include_str!("fixtures/v3-negative.kir"));
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "KLF-KIR-1003")
    );
}

fn has_code(diagnostics: &[Diagnostic], code: &str) -> bool {
    diagnostics.iter().any(|diagnostic| diagnostic.code == code)
}

#[test]
fn v4_balanced_ownership_verifies() {
    let source = include_str!("fixtures/v4-positive.kir");
    let module = parse_module(source).unwrap();
    assert_eq!(verify_module(&module), [] as [klf_diag::Diagnostic; 0]);
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v4_rejects_leaked_owned_value() {
    assert!(has_code(
        &verify(include_str!("fixtures/v4-negative-leak.kir")),
        "KLF-KIR-1004"
    ));
}

#[test]
fn v4_rejects_double_consume() {
    assert!(has_code(
        &verify(include_str!("fixtures/v4-negative-double.kir")),
        "KLF-KIR-1004"
    ));
}

#[test]
fn v4_rejects_consume_on_some_paths_only() {
    assert!(has_code(
        &verify(include_str!("fixtures/v4-negative-conditional.kir")),
        "KLF-KIR-1004"
    ));
}

#[test]
fn v5_v6_borrow_scope_verifies() {
    let source = include_str!("fixtures/borrow-positive.kir");
    let module = parse_module(source).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v5_rejects_consuming_a_borrow() {
    assert!(has_code(
        &verify(include_str!("fixtures/v5-negative.kir")),
        "KLF-KIR-1005"
    ));
}

#[test]
fn v6_rejects_unclosed_borrow_scope() {
    assert!(has_code(
        &verify(include_str!("fixtures/v6-negative.kir")),
        "KLF-KIR-1006"
    ));
}

#[test]
fn v12_exported_representable_signature_verifies() {
    let source = include_str!("fixtures/v12-positive.kir");
    let module = parse_module(source).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v12_rejects_non_representable_exported_type() {
    assert!(has_code(
        &verify(include_str!("fixtures/v12-negative.kir")),
        "KLF-KIR-1012"
    ));
}

#[test]
fn v11_generic_parameter_in_generic_function_verifies() {
    let source = include_str!("fixtures/v11-positive.kir");
    let module = parse_module(source).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v11_rejects_undeclared_generic_parameter() {
    assert!(has_code(
        &verify(include_str!("fixtures/v11-negative.kir")),
        "KLF-KIR-1011"
    ));
}

#[test]
fn v9_native_asm_verifies_and_round_trips() {
    let source = include_str!("fixtures/v9-positive.kir");
    let module = parse_module(source).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v9_rejects_asm_in_runtime_function() {
    assert!(has_code(
        &verify(include_str!("fixtures/v9-negative.kir")),
        "KLF-KIR-1009"
    ));
}

#[test]
fn v7_non_conflicting_static_accesses_verify() {
    let source = include_str!("fixtures/v7-positive.kir");
    let module = parse_module(source).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v7_rejects_conflicting_static_accesses() {
    assert!(has_code(
        &verify(include_str!("fixtures/v7-negative.kir")),
        "KLF-KIR-1007"
    ));
}

#[test]
fn borrow_check_rejects_consume_while_borrowed() {
    assert!(has_code(
        &verify(include_str!("fixtures/consume-while-borrowed.kir")),
        "KLF-KIR-1102"
    ));
}

#[test]
fn v10_try_call_of_fallible_verifies_and_round_trips() {
    let source = include_str!("fixtures/v10-positive.kir");
    let module = parse_module(source).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v10_rejects_try_call_of_infallible() {
    assert!(has_code(
        &verify(include_str!("fixtures/v10-negative-trycall.kir")),
        "KLF-KIR-1010"
    ));
}

#[test]
fn v10_rejects_plain_call_of_fallible() {
    assert!(has_code(
        &verify(include_str!("fixtures/v10-negative-call.kir")),
        "KLF-KIR-1010"
    ));
}

#[test]
fn v8_address_only_by_reference_verifies() {
    let source = include_str!("fixtures/v8-positive.kir");
    let module = parse_module(source).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn v8_rejects_address_only_by_value() {
    assert!(has_code(
        &verify(include_str!("fixtures/v8-negative.kir")),
        "KLF-KIR-1008"
    ));
}

const FOREIGN_POSITIVE: &str = r#"module @Ffi lang "c" target "aarch64-apple-darwin" stage canonical

func private foreign @abs(%x: i32) -> i32 engine(native)

func module @main() -> i64 engine(native) {
entry():
  %n = const.int i32, -5
  %r = call.foreign @abs(%n)
  %w = sext %r to i64
  return %w
}
"#;

const FOREIGN_UNDECLARED: &str = r#"module @Ffi lang "c" target "aarch64-apple-darwin" stage canonical

func module @main() -> i64 engine(native) {
entry():
  %n = const.int i32, -5
  %r = call.foreign @missing(%n)
  %w = sext %r to i64
  return %w
}
"#;

#[test]
fn section_26_foreign_call_verifies_and_round_trips() {
    let module = parse_module(FOREIGN_POSITIVE).unwrap();
    assert!(
        verify_module(&module).is_empty(),
        "{:?}",
        verify_module(&module)
    );
    assert_eq!(module, parse_module(&module.to_string()).unwrap());
}

#[test]
fn section_26_rejects_call_to_undeclared_foreign_symbol() {
    assert!(has_code(&verify(FOREIGN_UNDECLARED), "KLF-KIR-1002"));
}
