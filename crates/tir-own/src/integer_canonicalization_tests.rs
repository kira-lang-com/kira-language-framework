use super::canonicalize_text;

#[test]
fn integer_division_funnel_shift_and_bit_count_are_canonical() {
    let source = "module @Integer tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Integer.compute(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %quotient = sdiv %a, %b\n  %shifted = fshl %quotient, %a, %b\n  %bits = ctpop %shifted\n  %result = bswap %bits\n  return %result\n}\n";
    let canonical = canonicalize_text(source).expect("integer operations are pure SSA");
    assert!(canonical.contains("fshl %"));
    assert!(canonical.contains("bswap %"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("integer canonicalization is idempotent")
    );
}

#[test]
fn integer_canonicalization_rejects_invalid_element_width() {
    let source = "module @Integer tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Integer.invalid(%a: i8) -> i8 engine(native) {\nbb0(%a: i8):\n  %bad = bswap %a\n  return %bad\n}\n";
    assert!(
        canonicalize_text(source).is_err(),
        "canonicalization must not emit invalid bswap CODE"
    );
}

#[test]
fn permissive_integer_flags_remain_canonical() {
    let source = "module @Integer tir 26.11.1 profile app semantics permissive\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Integer.flags(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %divided = udiv [exact] %a, %b\n  %merged = or [disjoint] %divided, %a\n  %result = ctlz [poison] %merged\n  return %result\n}\n";
    let canonical = canonicalize_text(source).expect("permissive integer flags canonicalize");
    assert!(canonical.contains("udiv [exact]"));
    assert!(canonical.contains("or [disjoint]"));
    assert!(canonical.contains("ctlz [poison]"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("integer flags remain canonical")
    );
}
