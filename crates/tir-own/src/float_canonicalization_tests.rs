use super::canonicalize_text;

#[test]
fn float_arithmetic_and_comparison_canonicalization_is_idempotent() {
    let source = "module @Float tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Float.choose(%a: f32, %b: f32) -> f32 engine(native) {\nbb0(%a: f32, %b: f32):\n  %sum = fadd %a, %b\n  %equal = fcmp oeq %sum, %a\n  %final = select %equal, %sum, %b\n  return %final\n}\n";
    let canonical = canonicalize_text(source).expect("unmodified float SSA is canonicalizable");
    assert!(canonical.contains("fcmp oeq"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("float SSA is canonical on replay")
    );
}

#[test]
fn unsupported_float_flags_are_not_silently_discarded() {
    let source = "module @Float tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Float.choose(%a: f32, %b: f32) -> f32 engine(native) {\nbb0(%a: f32, %b: f32):\n  %sum = fadd [fast] %a, %b\n  return %sum\n}\n";
    assert!(
        canonicalize_text(source).is_err(),
        "fast-math flags require explicit lowering"
    );
}
