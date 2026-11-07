//! V2 address projection and enum address CODE conformance.
use super::verify_text_module;
use tir_format::parse_text_module;

fn module(body: &str, profile: &str) -> String {
    format!(
        "module @Smoke tir 26.11.1 profile {profile} semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn v2_field_and_element_addresses_preserve_projected_types() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Pair = struct ridl { left: i32, right: i64 } policy default unmanaged\n\nfunc module @Smoke.field(%base: *%Pair) -> *i64 engine(native) {\nbb0(%base: *%Pair):\n  %field = field.addr %base, 1\n  return %field\n}\n\nfunc module @Smoke.element(%base: *[4 x i32], %index: i32) -> *i32 engine(native) {\nbb0(%base: *[4 x i32], %index: i32):\n  %element = elem.addr %base, %index\n  return %element\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("address projection source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects typed address projection: {text_errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("address projection CODE assembles"),
    )
    .expect("address projection CODE decodes");
    let binary_errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        binary_errors.is_empty(),
        "binary V2 rejects typed address projection: {binary_errors:?}"
    );
}

#[test]
fn v2_unchecked_elem_address_is_typed_in_permissive_semantics() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = "module @Smoke tir 26.11.1 profile app semantics permissive\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.element(%base: *[4 x i64], %index: i64) -> *i64 engine(native) {\nbb0(%base: *[4 x i64], %index: i64):\n  %element = elem.addr %base, %index [unchecked]\n  return %element\n}\n";
    let parsed = parse_text_module(source).expect("permissive elem.addr parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "text V2 rejects permissive elem.addr: {errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("unchecked CODE assembles"))
            .expect("unchecked CODE decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2 rejects permissive elem.addr: {errors:?}"
    );
}

#[test]
fn v2_enum_payload_address_and_payloadless_inject_verify_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "type %Choice = enum ridl { None, Number(i32), Pair(i32, i64) } policy default unmanaged\n\nfunc module @Smoke.payload(%base: *%Choice) -> *i32 engine(native) {\nbb0(%base: *%Choice):\n  %field = enum.payload.addr %base, Number\n  return %field\n}\n\nfunc module @Smoke.inject(%base: *%Choice) -> unit engine(native) {\nbb0(%base: *%Choice):\n  enum.inject %base, None\n  return\n}\n\nfunc module @Smoke.tuple(%base: *%Choice) -> *(i32, i64) engine(native) {\nbb0(%base: *%Choice):\n  %field = enum.payload.addr %base, Pair\n  return %field\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("enum address source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects enum address: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("enum address CODE assembles"))
            .expect("enum address binary decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2 rejects enum address: {errors:?}"
    );
}

#[test]
fn v2_enum_address_rejects_unknown_variant_names() {
    use tir_diag::Severity;
    let source = module(
        "type %Choice = enum ridl { None, Number(i32) } policy default unmanaged\n\nfunc module @Smoke.invalid(%base: *%Choice) -> unit engine(native) {\nbb0(%base: *%Choice):\n  %field = enum.payload.addr %base, Unknown\n  return\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("invalid enum address source parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(!errors.is_empty(), "V2 accepted nonexistent enum variant");
}

#[test]
fn v2_v20_address_pointer_conversions_are_typed_and_classified_unmanaged_on_cpu() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.to_ptr(%address: *i32) -> ptr engine(native) {\nbb0(%address: *i32):\n  %ptr = addr.toptr %address\n  return %ptr\n}\n\nfunc module @Smoke.to_addr(%ptr: ptr) -> *i64 engine(native) {\nbb0(%ptr: ptr):\n  %address = ptr.toaddr %ptr as *i64\n  return %address\n}\n\nfunc module @Smoke.device(%ptr: ptr<device>) -> *i32<device> engine(native) {\nbb0(%ptr: ptr<device>):\n  %address = ptr.toaddr %ptr as *i32<device>\n  return %address\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("pointer-address source parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "text V2 rejects pointer-address conversion: {errors:?}"
    );
    let binary = BinaryModule::decode(
        &assemble_text_module(&parsed).expect("pointer-address CODE assembles"),
    )
    .expect("pointer-address CODE decodes");
    let analysis = binary
        .analysis_summary(u64::MAX)
        .expect("ANALYSIS section decodes");
    assert_eq!(analysis.functions.len(), 3);
    for record in &analysis.functions {
        assert_eq!(
            record.trust_class, 2,
            "CPU raw pointer casts must be unmanaged"
        );
        assert_eq!(record.reason_mask & (1 << 1), 1 << 1);
    }
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2/V20 rejects pointer-address conversion: {errors:?}"
    );
}

#[test]
fn v2_v20_pointer_cast_bad_address_space_is_rejected() {
    use tir_diag::Severity;
    let source = module(
        "func module @Smoke.bad(%p: ptr) -> *i32<device> engine(native) {\nbb0(%p: ptr):\n  %result = ptr.toaddr %p as *i32<device>\n  return %result\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("mismatched address space source parses");
    let errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        !errors.is_empty() || tir_format::assemble_text_module(&parsed).is_err(),
        "both the text verifier and assembler accepted a pointer/address space mismatch"
    );
}

#[test]
fn v2_address_cast_preserves_space_and_layout_in_text_and_binary() {
    use tir_diag::Severity;
    use tir_format::{BinaryModule, assemble_text_module};

    let source = module(
        "func module @Smoke.scalar(%base: *i32) -> *f32 engine(native) {\nbb0(%base: *i32):\n  %view = addr.cast %base to *f32\n  return %view\n}\n\nfunc module @Smoke.device(%base: *f64<device>) -> *i64<device> engine(native) {\nbb0(%base: *f64<device>):\n  %view = addr.cast %base to *i64<device>\n  return %view\n}",
        "app",
    );
    let parsed = parse_text_module(&source).expect("address cast source parses");
    let text_errors = verify_text_module(&parsed, Default::default())
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        text_errors.is_empty(),
        "text V2 rejects layout-compatible cast: {text_errors:?}"
    );
    let binary =
        BinaryModule::decode(&assemble_text_module(&parsed).expect("address cast CODE assembles"))
            .expect("address cast TIRB decodes");
    let errors = super::verify_binary_module_records(&binary, u64::MAX)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "binary V2 rejects layout-compatible cast: {errors:?}"
    );
}
