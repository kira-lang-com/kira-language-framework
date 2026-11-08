use super::canonicalize_text;

fn module(body: &str) -> String {
    format!(
        "module @Unique tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\nfunc module @Unique.drop(%object: owned uniq i32, %condition: i1) -> unit engine(native) {{\n{body}\n}}\n"
    )
}

#[test]
fn distinct_branch_local_moves_receive_distinct_destroys() {
    let raw = module(
        "bb0(%object: owned uniq i32, %condition: i1):\n  cond.br %condition, bb1(), bb2()\nbb2:\n  %right = move %object\n  return\nbb1:\n  %left = move %object\n  return",
    );
    let canonical =
        canonicalize_text(&raw).expect("two independent owned moves on disjoint exit paths");
    assert_eq!(canonical.matches(" = move ").count(), 2);
    assert_eq!(canonical.matches("destroy %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("exit move canonicalization is stable")
    );
}

#[test]
fn branch_local_borrow_and_move_can_consume_on_separate_exits() {
    let raw = module(
        "bb0(%object: owned uniq i32, %condition: i1):\n  cond.br %condition, bb1(), bb2()\nbb2:\n  %loan = borrow.begin %object\n  borrow.end %loan\n  return\nbb1:\n  %moved = move %object\n  return",
    );
    let canonical = canonicalize_text(&raw).expect("disjoint branch-local borrow and move");
    assert!(canonical.contains("borrow.end"));
    assert_eq!(canonical.matches("destroy %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("exit borrowing remains canonical")
    );
}

#[test]
fn invalid_branch_owned_consumption_is_rejected() {
    for body in [
        "bb0(%object: owned uniq i32, %condition: i1):\n  cond.br %condition, bb1(), bb2()\nbb2:\n  %first = move %object\n  %second = move %object\n  return\nbb1:\n  return",
        "bb0(%object: owned uniq i32, %condition: i1):\n  %moved = move %object\n  cond.br %condition, bb1(), bb2()\nbb2:\n  return\nbb1:\n  return",
        "bb0(%object: owned uniq i32, %condition: i1):\n  cond.br %condition, bb1(), bb2()\nbb2:\n  %loan = borrow.begin %object\n  return\nbb1:\n  return",
    ] {
        assert!(
            canonicalize_text(&module(body)).is_err(),
            "unsupported ownership flow canonicalized: {body}"
        );
    }
}
