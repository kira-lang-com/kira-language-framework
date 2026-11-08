use super::canonicalize_text;

fn module(body: &str) -> String {
    format!(
        "module @StackCleanup tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

#[test]
fn single_stack_slot_is_deallocated_before_return() {
    let source = module(
        "func module @StackCleanup.read(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  %read = load [trivial] %slot\n  return %read\n}",
    );
    let canonical = canonicalize_text(&source).expect("stack lifetime ends on return");
    assert!(canonical.contains("stack.dealloc %"));
    assert_eq!(canonical.matches("stack.dealloc %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("stack lifetime is stable")
    );
}

#[test]
fn multiple_stack_slots_deallocate_in_reverse_order() {
    let source = module(
        "func module @StackCleanup.read(%a: i32, %b: i32) -> i32 engine(native) {\nbb0(%a: i32, %b: i32):\n  %first = stack.alloc i32\n  %second = stack.alloc i32\n  store [init] %a to %first\n  store [init] %b to %second\n  %read = load [trivial] %first\n  return %read\n}",
    );
    let canonical = canonicalize_text(&source).expect("stack slots deallocate LIFO");
    let first = canonical
        .find("stack.dealloc %v3")
        .expect("second slot is freed");
    let second = canonical
        .find("stack.dealloc %v2")
        .expect("first slot is freed");
    assert!(
        first < second,
        "stack dealloc must follow reverse allocation order"
    );
    assert_eq!(canonical.matches("stack.dealloc %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("stack LIFO cleanup is stable")
    );
}

#[test]
fn preexisting_lifo_prefix_only_needs_missing_deallocation() {
    let source = module(
        "func module @StackCleanup.read(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %first = stack.alloc i32\n  %second = stack.alloc i32\n  store [init] %v to %first\n  stack.dealloc %second\n  return %v\n}",
    );
    let canonical = canonicalize_text(&source).expect("missing LIFO suffix is appended");
    assert_eq!(canonical.matches("stack.dealloc %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("partial LIFO cleanup stays stable")
    );
}

#[test]
fn uninitialized_read_receives_zero_and_deallocation() {
    let source = module(
        "func module @StackCleanup.read() -> i32 engine(native) {\nbb0:\n  %slot = stack.alloc i32\n  %read = load [trivial] %slot\n  return %read\n}",
    );
    let canonical = canonicalize_text(&source).expect("zero and cleanup are inserted");
    assert!(canonical.contains("const.int i32, 0"));
    assert!(canonical.contains("store [init]"));
    assert_eq!(canonical.matches("stack.dealloc %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("zero and cleanup are stable")
    );
}

#[test]
fn two_way_branch_cleans_stack_on_both_return_paths() {
    let source = module(
        "func module @StackCleanup.branch(%v: i32, %flag: i1) -> i32 engine(native) {\nbb0(%v: i32, %flag: i1):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return %v\nbb1:\n  %read = load [trivial] %slot\n  return %read\n}",
    );
    let canonical = canonicalize_text(&source).expect("stack is deallocated on both exits");
    assert_eq!(canonical.matches("stack.dealloc %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("branch stack cleanup is stable")
    );
}

#[test]
fn cfg_stack_zero_initialization_runs_after_deallocation_insertion() {
    let source = module(
        "func module @StackCleanup.branch(%v: i32, %flag: i1) -> i32 engine(native) {\nbb0(%v: i32, %flag: i1):\n  %slot = stack.alloc i32\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return %v\nbb1:\n  %read = load [trivial] %slot\n  return %read\n}",
    );
    let canonical = canonicalize_text(&source).expect("multi-block zero and cleanup are inserted");
    assert!(canonical.contains("const.int i32, 0"));
    assert_eq!(canonical.matches("stack.dealloc %").count(), 2);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("CFG stack zero/cleanup stable")
    );
}

#[test]
fn loop_back_edge_deallocates_entry_stack_slot_on_exit() {
    let source = module(
        "func module @StackCleanup.loop(%v: i32, %again: i1) -> i32 engine(native) {\nbb0(%v: i32, %again: i1):\n  %slot = stack.alloc i32\n  store [init] %v to %slot\n  br bb1()\nbb1:\n  %read = load [trivial] %slot\n  cond.br %again, bb1(), bb2()\nbb2:\n  return %read\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("entry allocation survives loop and cleans up on exit");
    assert_eq!(canonical.matches("stack.dealloc %").count(), 1);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("loop lifetime stays canonical")
    );
}

#[test]
fn cfg_partial_lifo_suffix_is_completed_per_return_edge() {
    let source = module(
        "func module @StackCleanup.branch(%v: i32, %flag: i1) -> i32 engine(native) {\nbb0(%v: i32, %flag: i1):\n  %first = stack.alloc i32\n  %second = stack.alloc i32\n  store [init] %v to %first\n  cond.br %flag, bb1(), bb2()\nbb2:\n  stack.dealloc %second\n  return %v\nbb1:\n  stack.dealloc %second\n  stack.dealloc %first\n  return %v\n}",
    );
    let canonical =
        canonicalize_text(&source).expect("missing deallocation is added on only one exit");
    assert_eq!(canonical.matches("stack.dealloc %").count(), 4);
    assert_eq!(
        canonical,
        canonicalize_text(&canonical).expect("partial CFG cleanup stays canonical")
    );
}

#[test]
fn stack_cleanup_rejects_non_lifo_early_dealloc_alias_and_unclosed_scope() {
    for body in [
        "func module @StackCleanup.bad(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %first = stack.alloc i32\n  %second = stack.alloc i32\n  stack.dealloc %first\n  return %v\n}",
        "func module @StackCleanup.bad(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %slot = stack.alloc i32\n  stack.dealloc %slot\n  %read = load [trivial] %slot\n  return %read\n}",
        "func module @StackCleanup.bad(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %slot = stack.alloc i32\n  %alias = copy %slot\n  return %v\n}",
        "func module @StackCleanup.bad(%v: i32) -> i32 engine(native) {\nbb0(%v: i32):\n  %slot = stack.alloc i32\n  %access = access.begin [read] [static] %slot\n  return %v\n}",
        "func module @StackCleanup.bad(%v: i32, %flag: i1) -> i32 engine(native) {\nbb0(%v: i32, %flag: i1):\n  %slot = stack.alloc i32\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return %v\nbb1:\n  stack.dealloc %slot\n  br bb2()\n}",
        "func module @StackCleanup.bad(%v: i32, %flag: i1) -> i32 engine(native) {\nbb0(%v: i32, %flag: i1):\n  %slot = stack.alloc i32\n  cond.br %flag, bb1(), bb2()\nbb2:\n  return %v\nbb1:\n  %other = stack.alloc i32\n  return %v\n}",
    ] {
        assert!(
            canonicalize_text(&module(body)).is_err(),
            "unsupported stack lifetime accepted: {body}"
        );
    }
}
