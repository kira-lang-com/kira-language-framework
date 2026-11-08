use super::canonicalize_text;

fn module(body: &str) -> String {
    format!(
        "module @CanonicalHeap tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn unused_counted_and_unique_allocations_receive_explicit_destroys() {
    let source = module(
        "func module @CanonicalHeap.allocate() -> unit engine(native) {\nbb0:\n  %shared = heap.alloc.counted i32\n  %unique = heap.alloc.unique i64\n  return\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("unused scalar heap allocations are destroyed");
    assert!(canonical.contains("heap.alloc.counted i32"));
    assert!(canonical.contains("heap.alloc.unique i64"));
    assert_eq!(canonical.matches("destroy %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("heap cleanup is idempotent")
    );
}

#[test]
fn existing_explicit_destroys_are_preserved_without_duplication() {
    let source = module(
        "func module @CanonicalHeap.allocate() -> unit engine(native) {\nbb0:\n  %shared = heap.alloc.counted f32\n  %unique = heap.alloc.unique i32\n  destroy %unique\n  destroy %shared\n  return\n}",
    );
    let canonical = canonicalize_text(&source).expect("explicit destruction remains canonical");
    assert_eq!(canonical.matches("destroy %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("explicit destruction stays stable")
    );
}

#[test]
fn multiple_functions_can_canonicalize_independent_heap_lifetimes() {
    let source = module(
        "func module @CanonicalHeap.first() -> unit engine(native) {\nbb0:\n  %a = heap.alloc.counted i16\n  return\n}\n\nfunc module @CanonicalHeap.second() -> unit engine(native) {\nbb0:\n  %b = heap.alloc.unique i128\n  return\n}",
    );
    let canonical = canonicalize_text(&source).expect("disjoint function ownership is independent");
    assert_eq!(canonical.matches("destroy %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("multi-function cleanup is stable")
    );
}

#[test]
fn aliased_double_consumed_or_scoped_heap_values_are_refused() {
    for body in [
        "func module @CanonicalHeap.bad() -> unit engine(native) {\nbb0:\n  %value = heap.alloc.counted i32\n  %addr = ref.addr %value\n  return\n}",
        "func module @CanonicalHeap.bad() -> unit engine(native) {\nbb0:\n  %value = heap.alloc.unique i32\n  destroy %value\n  destroy %value\n  return\n}",
        "func module @CanonicalHeap.bad() -> unit engine(native) {\nbb0:\n  destroy %value\n  %value = heap.alloc.counted i32\n  return\n}",
        "func module @CanonicalHeap.bad() -> unit engine(native) {\nbb0:\n  %value = heap.alloc.counted (i32, i32)\n  return\n}",
        "func module @CanonicalHeap.bad() -> uniq i32 engine(native) {\nbb0:\n  %value = heap.alloc.unique i32\n  %moved = move %value\n  return %moved\n}",
        "func module @CanonicalHeap.bad(%flag: i1) -> unit engine(native) {\nbb0(%flag: i1):\n  %value = heap.alloc.unique i32\n  cond.br %flag, bb1(), bb2()\nbb2:\n  br bb3()\nbb1:\n  br bb3()\nbb3:\n  return\n}",
    ] {
        let source = module(body);
        assert!(
            canonicalize_text(&source).is_err(),
            "unsupported heap lifetime accepted: {body}"
        );
    }
}

#[test]
fn two_branch_exits_each_consume_allocated_owner() {
    let source = module(
        "func module @CanonicalHeap.branch(%flag: i1) -> unit engine(native) {\nbb0(%flag: i1):\n  %shared = heap.alloc.counted i32\n  %unique = heap.alloc.unique i64\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return\nbb1:\n  return\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("disjoint paths each consume their allocations");
    assert_eq!(canonical.matches("heap.alloc.").count(), 2);
    assert_eq!(canonical.matches("destroy %").count(), 4);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("conditional cleanup stays canonical")
    );
}

#[test]
fn branch_destroys_are_completed_only_on_missing_exit_paths() {
    let source = module(
        "func module @CanonicalHeap.branch(%flag: i1) -> unit engine(native) {\nbb0(%flag: i1):\n  %owner = heap.alloc.unique i32\n  cond.br %flag, bb1(), bb2()\nbb2:\n  destroy %owner\n  return\nbb1:\n  return\n}",
    );
    let canonical = canonicalize_text(&source).expect("missing branch destructor is inserted");
    assert_eq!(canonical.matches("destroy %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("branch cleanup is stable")
    );
}

#[test]
fn branch_heap_escapes_and_unsupported_cfg_are_refused() {
    for body in [
        "func module @CanonicalHeap.bad(%flag: i1) -> unit engine(native) {\nbb0(%flag: i1):\n  %owner = heap.alloc.counted i32\n  %pointer = ref.addr %owner\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return\nbb1:\n  return\n}",
        "func module @CanonicalHeap.bad(%flag: i1) -> unit engine(native) {\nbb0(%flag: i1):\n  %owner = heap.alloc.unique i32\n  br bb1()\nbb1:\n  return\nbb2:\n  return\n}",
        "func module @CanonicalHeap.bad(%flag: i1) -> unit engine(native) {\nbb0(%flag: i1):\n  %owner = heap.alloc.unique i32\n  cond.br %flag, bb1(), bb2()\nbb2:\n  destroy %owner\n  destroy %owner\n  return\nbb1:\n  return\n}",
        "func module @CanonicalHeap.bad(%flag: i1) -> unit engine(native) {\nbb0(%flag: i1):\n  %owner = heap.alloc.unique i32\n  cond.br %flag, bb1(%owner), bb2()\nbb2:\n  return\nbb1(%owner2: owned uniq i32):\n  return\n}",
    ] {
        assert!(
            canonicalize_text(&module(body)).is_err(),
            "unsafe managed CFG canonicalized: {body}"
        );
    }
}

#[test]
fn weak_new_from_a_borrowed_reference_is_destroyed() {
    let source = module(
        "func module @CanonicalHeap.weak(%strong: borrow ref i32) -> unit engine(native) {\nbb0(%strong: borrow ref i32):\n  %weak = weak.new %strong\n  return\n}",
    );
    let canonical = canonicalize_text(&source).expect("new weak reference is cleaned up");
    assert!(canonical.contains("weak.new %"));
    assert_eq!(canonical.matches("destroy %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("weak cleanup is idempotent")
    );
}

#[test]
fn weak_new_is_destroyed_on_each_conditional_exit() {
    let source = module(
        "func module @CanonicalHeap.weak(%strong: borrow ref i32, %flag: i1) -> unit engine(native) {\nbb0(%strong: borrow ref i32, %flag: i1):\n  %weak = weak.new %strong\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return\nbb1:\n  return\n}",
    );
    let canonical = canonicalize_text(&source).expect("weak value is destroyed on either exit");
    assert_eq!(canonical.matches("destroy %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("weak CFG cleanup remains stable")
    );
}

#[test]
fn weak_new_canonicalization_refuses_unborrowed_sources_and_escapes() {
    for body in [
        "func module @CanonicalHeap.bad(%strong: owned ref i32) -> unit engine(native) {\nbb0(%strong: owned ref i32):\n  %weak = weak.new %strong\n  return\n}",
        "func module @CanonicalHeap.bad(%raw: ptr) -> unit engine(native) {\nbb0(%raw: ptr):\n  %weak = weak.new %raw\n  return\n}",
        "func module @CanonicalHeap.bad(%strong: borrow ref i32) -> unit engine(native) {\nbb0(%strong: borrow ref i32):\n  %weak = weak.new %strong\n  %second = move %weak\n  return\n}",
        "func module @CanonicalHeap.bad(%strong: borrow ref i32) -> weak i32 engine(native) {\nbb0(%strong: borrow ref i32):\n  %weak = weak.new %strong\n  %moved = move %weak\n  return %moved\n}",
    ] {
        assert!(
            canonicalize_text(&module(body)).is_err(),
            "unsupported weak ownership accepted: {body}"
        );
    }
}

#[test]
fn returning_new_counted_and_unique_owners_transfers_without_destroy() {
    let source = module(
        "func module @CanonicalHeap.shared() -> ref i32 engine(native) {\nbb0:\n  %owner = heap.alloc.counted i32\n  return %owner\n}\n\nfunc module @CanonicalHeap.exclusive() -> uniq i64 engine(native) {\nbb0:\n  %owner = heap.alloc.unique i64\n  return %owner\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("directly returned heap owners transfer to caller");
    assert_eq!(canonical.matches("destroy %").count(), 0);
    assert!(canonical.contains("heap.alloc.counted i32"));
    assert!(canonical.contains("heap.alloc.unique i64"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("heap owner transfers are stable")
    );
}

#[test]
fn returning_a_new_weak_reference_transfers_to_caller() {
    let source = module(
        "func module @CanonicalHeap.weak(%strong: borrow ref i32) -> weak i32 engine(native) {\nbb0(%strong: borrow ref i32):\n  %weak = weak.new %strong\n  return %weak\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("weak reference ownership transfers to caller");
    assert!(canonical.contains("weak.new %"));
    assert_eq!(canonical.matches("destroy %").count(), 0);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("returned weak value is stable")
    );
}

#[test]
fn unused_allocation_is_destroyed_while_other_owner_is_returned() {
    let source = module(
        "func module @CanonicalHeap.choose() -> uniq i64 engine(native) {\nbb0:\n  %unused = heap.alloc.counted i32\n  %result = heap.alloc.unique i64\n  return %result\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("dead heap owner is destroyed; live owner transferred");
    assert_eq!(canonical.matches("destroy %").count(), 1);
    assert!(canonical.contains("heap.alloc.unique i64"));
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("mixed transfer/cleanup is stable")
    );
}

#[test]
fn heap_return_type_mismatch_and_destroy_before_return_are_rejected() {
    for body in [
        "func module @CanonicalHeap.invalid() -> uniq i32 engine(native) {\nbb0:\n  %value = heap.alloc.counted i32\n  return %value\n}",
        "func module @CanonicalHeap.invalid() -> ref i64 engine(native) {\nbb0:\n  %value = heap.alloc.counted i32\n  return %value\n}",
        "func module @CanonicalHeap.invalid() -> uniq i32 engine(native) {\nbb0:\n  %value = heap.alloc.unique i32\n  destroy %value\n  return %value\n}",
        "func module @CanonicalHeap.invalid(%input: owned uniq i32, %wrong: i32) -> uniq i32 engine(native) {\nbb0(%input: owned uniq i32, %wrong: i32):\n  return %wrong\n}",
    ] {
        assert!(
            canonicalize_text(&module(body)).is_err(),
            "invalid managed transfer accepted: {body}"
        );
    }
}
