use super::*;

#[test]
fn inserts_destroy_of_final_unique_move_result() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  %first = move %object\n  %second = move %first\n  return\n}\n";
    let canonical = canonicalize_text(text).expect("moved unique receives a destroy");
    assert!(
        canonical.contains("destroy %v2"),
        "final owned value must be destroyed: {canonical}"
    );
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("canonical text is stable")
    );
}

#[test]
fn cannot_move_owned_unique_twice() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  %first = move %object\n  %second = move %object\n  return\n}\n";
    assert!(
        canonicalize_text(text).is_err(),
        "a second move from consumed storage must fail"
    );
}

#[test]
fn owned_unique_is_destroyed_after_balanced_borrow() {
    let text = "module @Smoke tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Smoke.drop(%object: owned uniq i32) -> unit engine(native) {\nbb0(%object: owned uniq i32):\n  %loan = borrow.begin %object\n  borrow.end %loan\n  return\n}\n";
    let canonical = canonicalize_text(text).expect("borrow scope and final destroy canonicalize");
    assert!(canonical.contains("borrow.end %v1"));
    assert!(canonical.contains("destroy %arg0"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("canonical text is stable")
    );
}
