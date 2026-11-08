use super::canonicalize_text;

fn module(semantics: &str, body: &str) -> String {
    format!(
        "module @Canonical tir 26.11.1 profile app semantics {semantics}\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn float_constants_in_arithmetic_canonicalize_idempotently() {
    let source = module(
        "defined",
        "func module @Canonical.float() -> f32 engine(native) {\nbb0:\n  %first = const.float f32, 1.5\n  %second = const.float f32, 2.25\n  %total = fadd %first, %second\n  return %total\n}",
    );
    let canonical = canonicalize_text(&source).expect("float constants canonicalize");
    assert_eq!(canonical.matches("const.float f32").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("canonical float module is stable")
    );
}

#[test]
fn byte_constant_used_by_arithmetic_canonicalizes() {
    let source = module(
        "defined",
        r#"func module @Canonical.bytes(%value: i32) -> i32 engine(native) {
bb0(%value: i32):
  %one = const.bytes i32, b"\x01\x00\x00\x00"
  %sum = add %one, %value
  return %sum
}"#,
    );
    let canonical = canonicalize_text(&source).expect("byte constant canonicalizes");
    assert!(canonical.contains("const.bytes i32"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("canonical byte module is stable")
    );
}

#[test]
fn pointer_null_and_permissive_poison_canonicalize() {
    let null = module(
        "defined",
        "func module @Canonical.null() -> i1 engine(native) {\nbb0:\n  %first = const.null device\n  %second = const.null device\n  %same = icmp eq %first, %second\n  return %same\n}",
    );
    let canonical = canonicalize_text(&null).expect("pointer null constants canonicalize");
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("canonical null module is stable")
    );

    let poison = module(
        "permissive",
        "func module @Canonical.poison(%value: i32) -> i32 engine(native) {\nbb0(%value: i32):\n  %undefined = const.poison i32\n  %sum = add %undefined, %value\n  return %sum\n}",
    );
    let canonical = canonicalize_text(&poison).expect("permissive poison canonicalizes");
    assert!(canonical.contains("const.poison i32"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("canonical poison module is stable")
    );
}

#[test]
fn poison_in_defined_semantics_is_rejected() {
    let source = module(
        "defined",
        "func module @Canonical.poison(%value: i32) -> i32 engine(native) {\nbb0(%value: i32):\n  %undefined = const.poison i32\n  %sum = add %undefined, %value\n  return %sum\n}",
    );
    assert!(canonicalize_text(&source).is_err());
}

#[test]
fn freezing_poison_before_use_is_canonical() {
    let source = module(
        "permissive",
        "func module @Canonical.safe(%v: f32) -> f32 engine(native) {\nbb0(%v: f32):\n  %poison = const.poison f32\n  %frozen = freeze %poison\n  %sum = fadd %v, %frozen\n  return %sum\n}",
    );
    let canonical = canonicalize_text(&source).expect("freeze of poison canonicalizes");
    assert!(canonical.contains("freeze %"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("frozen value is canonical")
    );
}

#[test]
fn string_value_and_tuple_selection_canonicalize_idempotently() {
    let source = module(
        "defined",
        "func module @Canonical.string(%condition: i1) -> (ptr, i64) engine(native) {\nbb0(%condition: i1):\n  %yes = const.string \"yes\"\n  %no = const.string \"no\"\n  %chosen = select %condition, %yes, %no\n  return %chosen\n}",
    );
    let canonical = canonicalize_text(&source).expect("trivial string tuples are canonical");
    assert_eq!(canonical.matches("const.string").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("string selection remains canonical")
    );
}

#[test]
fn trivial_tuple_aggregate_constant_canonicalizes_idempotently() {
    let source = module(
        "defined",
        "func module @Canonical.aggregate() -> (i32, f32, ptr) engine(native) {\nbb0:\n  %literal = const.aggregate (i32, f32, ptr)(42, 1.5, null)\n  return %literal\n}",
    );
    let canonical = canonicalize_text(&source).expect("trivial tuple aggregate canonicalizes");
    assert!(canonical.contains("const.aggregate (i32, f32, ptr)"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("canonical trivial aggregate is stable"),
    );
}
