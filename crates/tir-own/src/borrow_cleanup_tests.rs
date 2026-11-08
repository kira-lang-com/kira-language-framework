use super::canonicalize_text;

fn module(body: &str) -> String {
    format!(
        "module @BorrowCleanup tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn trivial_borrow_scope_is_closed_before_return() {
    let source = module(
        "func module @BorrowCleanup.read(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %loan = borrow.begin %a\n  %sum = add %loan, %b\n  return %sum\n}",
    );
    let canonical = canonicalize_text(&source).expect("single-block borrowed value closes at exit");
    assert_eq!(canonical.matches("borrow.end %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("borrow closure is stable")
    );
}

#[test]
fn nested_scopes_are_ended_in_reverse_order() {
    let source = module(
        "func module @BorrowCleanup.read(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %outer = borrow.begin %a\n  %inner = borrow.begin %b\n  %sum = add %outer, %inner\n  return %sum\n}",
    );
    let canonical = canonicalize_text(&source).expect("nested scopes end LIFO");
    assert_eq!(canonical.matches("borrow.end %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("nested scope closure is stable")
    );
}

#[test]
fn partial_scope_nesting_is_completed_without_redundant_ends() {
    let source = module(
        "func module @BorrowCleanup.read(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %outer = borrow.begin %a\n  %inner = borrow.begin %b\n  borrow.end %inner\n  return %a\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("outer borrow is closed after explicit inner end");
    assert_eq!(canonical.matches("borrow.end %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("partial borrow closure stays stable")
    );
}

#[test]
fn owned_unique_is_destroyed_after_synthesized_borrow_end() {
    let source = module(
        "func module @BorrowCleanup.release(%owner: owned uniq i32) -> unit engine(native) {\nbb0(%owner: owned uniq i32):\n  %loan = borrow.begin %owner\n  return\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("borrow closes before unique owner is destroyed");
    let end = canonical.find("borrow.end %").expect("borrow ends");
    let destroy = canonical.find("destroy %").expect("unique owner destroyed");
    assert!(end < destroy);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("borrow+destroy stays stable")
    );
}

#[test]
fn load_borrow_closes_before_automatic_stack_deallocation() {
    let source = module(
        "func module @BorrowCleanup.stack(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %loan = load.borrow %slot\n  %result = add %loan, %v\n  return %result\n}",
    );
    let canonical = canonicalize_text(&source).expect("in-memory borrow ends before stack cleanup");
    let end = canonical.find("borrow.end %").expect("loan ended");
    let dealloc = canonical.find("stack.dealloc %").expect("slot deallocated");
    assert!(end < dealloc);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("borrowed stack lifetime is stable")
    );
}

#[test]
fn invalid_scope_nesting_or_escapes_remain_errors() {
    for body in [
        "func module @BorrowCleanup.bad(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %outer = borrow.begin %a\n  %inner = borrow.begin %b\n  borrow.end %outer\n  return %b\n}",
        "func module @BorrowCleanup.bad(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %loan = borrow.begin %a\n  return %loan\n}",
        "func module @BorrowCleanup.bad(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  borrow.end %missing\n  return %a\n}",
        "func module @BorrowCleanup.bad(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %loan = borrow.begin %a\n  borrow.end %loan\n  borrow.end %loan\n  return %a\n}",
        "func module @BorrowCleanup.bad(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %loan = borrow.begin %a\n  br bb1()\nbb1:\n  return %a\n}",
        "func module @BorrowCleanup.bad(%a: i32) -> i32 engine(native) {\nbb0(%a: i32):\n  %slot = stack.alloc i32\n  store [init] %a to %slot\n  %loan = load.borrow %slot\n  stack.dealloc %slot\n  return %a\n}",
    ] {
        assert!(
            canonicalize_text(&module(body)).is_err(),
            "unsafe borrow was accepted: {body}"
        );
    }
}
