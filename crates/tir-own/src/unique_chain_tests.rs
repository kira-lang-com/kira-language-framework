use super::canonicalize_text;

fn source(body: &str) -> String {
    format!(
        "module @Chain tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Chain.forward(%object: owned uniq i32, %c: i1) -> unit engine(native) {{\n{body}\n}}\n"
    )
}

#[test]
fn forwarded_unique_is_destroyed_in_target_block() {
    let raw = source(
        "bb0(%object: owned uniq i32, %c: i1):\n  br bb1(%object, %c)\nbb1(%next: owned uniq i32, %flag: i1):\n  return",
    );
    let canonical = canonicalize_text(&raw).expect("unique forwarded once");
    assert_eq!(canonical.matches("destroy %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("stable forwarded ownership")
    );
}

#[test]
fn multiple_hops_and_linear_move_destroy_final_owner() {
    let raw = source(
        "bb0(%object: owned uniq i32, %c: i1):\n  br bb1(%object, %c)\nbb1(%next: owned uniq i32, %flag: i1):\n  br bb2(%next)\nbb2(%final: owned uniq i32):\n  %moved = move %final\n  return",
    );
    let canonical = canonicalize_text(&raw).expect("uniqueness forwards through two blocks");
    assert_eq!(canonical.matches("destroy %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("forwarded move is stable")
    );
}

#[test]
fn invalid_forwarding_is_rejected() {
    for body in [
        "bb0(%object: owned uniq i32, %c: i1):\n  br bb1(%object, %object)\nbb1(%next: owned uniq i32, %again: owned uniq i32):\n  return",
        "bb0(%object: owned uniq i32, %c: i1):\n  br bb1(%c)\nbb1(%next: owned uniq i32):\n  return",
        "bb0(%object: owned uniq i32, %c: i1):\n  cond.br %c, bb1(%object), bb2(%object)\nbb1(%next: owned uniq i32):\n  return\nbb2(%last: owned uniq i32):\n  return",
        "bb0(%object: owned uniq i32, %c: i1):\n  br bb1(%object)\nbb1(%next: owned uniq i32):\n  %copied = copy %next\n  return",
    ] {
        assert!(
            canonicalize_text(&source(body)).is_err(),
            "invalid handoff accepted: {body}"
        );
    }
}
