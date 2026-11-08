use super::canonicalize_text;

fn module(body: &str) -> String {
    format!(
        "module @Transfer tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn owned_unique_parameter_can_transfer_directly_to_return() {
    let source = module(
        "func module @Transfer.identity(%owner: owned uniq i32) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32):\n  return %owner\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("unique owner is transferred rather than destroyed");
    assert_eq!(canonical.matches("destroy %").count(), 0);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("unique return is idempotent")
    );
}

#[test]
fn linear_move_chain_can_transfer_final_unique_to_return() {
    let source = module(
        "func module @Transfer.move(%owner: owned uniq i32) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32):\n  %first = move %owner\n  %second = move %first\n  return %second\n}",
    );
    let canonical = canonicalize_text(&source).expect("last live unique move transfers to caller");
    assert_eq!(canonical.matches("destroy %").count(), 0);
    assert_eq!(canonical.matches(" = move ").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("move/return transfer stays stable")
    );
}

#[test]
fn nonreturned_unique_parameters_receive_destroys_before_transfer() {
    let source = module(
        "func module @Transfer.choose(%first: owned uniq i32, %unused: owned uniq i64) -> uniq i32 engine(native) {\nbb0(%first: owned uniq i32, %unused: owned uniq i64):\n  return %first\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("other live owner is destroyed before return");
    assert_eq!(canonical.matches("destroy %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("unique return cleanup is stable")
    );
}

#[test]
fn disjoint_branch_returns_each_transfer_the_same_owned_parameter() {
    let source = module(
        "func module @Transfer.branch(%owner: owned uniq i32, %flag: i1) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32, %flag: i1):\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return %owner\nbb1:\n  return %owner\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("unique owner is transferred along either path");
    assert_eq!(canonical.matches("destroy %").count(), 0);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("disjoint unique returns remain stable")
    );
}

#[test]
fn forwarded_unique_block_parameter_can_be_returned() {
    let source = module(
        "func module @Transfer.forward(%owner: owned uniq i32) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32):\n  br bb1(%owner)\nbb1(%passed: owned uniq i32):\n  return %passed\n}",
    );
    let canonical = canonicalize_text(&source).expect("forwarded unique ownership is transferred");
    assert_eq!(canonical.matches("destroy %").count(), 0);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("forwarded return remains stable")
    );
}

#[test]
fn unique_return_rejects_double_moves_borrow_leaks_and_wrong_return_type() {
    for body in [
        "func module @Transfer.bad(%owner: owned uniq i32) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32):\n  destroy %owner\n  return %owner\n}",
        "func module @Transfer.bad(%owner: owned uniq i32) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32):\n  %moved = move %owner\n  return %owner\n}",
        "func module @Transfer.bad(%owner: owned uniq i32) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32):\n  %borrow = borrow.begin %owner\n  destroy %owner\n  return %owner\n}",
        "func module @Transfer.bad(%owner: owned uniq i32) -> uniq i64 engine(native) {\nbb0(%owner: owned uniq i32):\n  return %owner\n}",
        "func module @Transfer.bad(%owner: owned uniq i32) -> uniq i32 engine(native) {\nbb0(%owner: owned uniq i32):\n  return %missing\n}",
    ] {
        assert!(
            canonicalize_text(&module(body)).is_err(),
            "invalid unique return accepted: {body}"
        );
    }
}
