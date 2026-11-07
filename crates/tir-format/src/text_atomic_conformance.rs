use crate::{BinaryModule, assemble_text_module, disassemble_text_module, parse_text_module};

fn module(body: &str) -> String {
    format!(
        "module @Atomic tir 26.11.1 profile app semantics defined\n  producer \"klf\" \"26.11.1\" languages [kira]\n\n{body}\n"
    )
}

fn round_trip(input: &str) -> String {
    let parsed = parse_text_module(input).expect("atomic text parses");
    let encoded = assemble_text_module(&parsed).expect("atomic CODE assembles");
    let binary = BinaryModule::decode(&encoded).expect("atomic binary decodes");
    let canonical = disassemble_text_module(&binary, u64::MAX).expect("atomic CODE disassembles");
    let rebuilt =
        assemble_text_module(&parse_text_module(&canonical).expect("canonical text parses"))
            .expect("canonical atomic CODE reassembles");
    assert_eq!(encoded, rebuilt);
    canonical
}

#[test]
fn atomic_load_store_and_fence_round_trip_with_explicit_scopes() {
    let source = module(
        "func module @Atomic.run(%p: ptr, %v: i32) -> i32 engine(native) {\nbb0(%p: ptr, %v: i32):\n  atomic.store [order=release] [scope=system] %v, %p\n  %read = atomic.load [order=acquire] [scope=thread] i32, %p\n  fence [order=acq_rel] [scope=system]\n  return %read\n}",
    );
    let canonical = round_trip(&source);
    for clause in [
        "atomic.store [order=release] [scope=system]",
        "atomic.load [order=acquire] [scope=thread]",
        "fence [order=acq_rel] [scope=system]",
    ] {
        assert!(canonical.contains(clause), "missing {clause}");
    }
}

#[test]
fn default_and_explicit_sequential_consistency_are_distinct() {
    let source = module(
        "func module @Atomic.orders(%p: ptr, %v: i64) -> i64 engine(native) {\nbb0(%p: ptr, %v: i64):\n  atomic.store %v, %p\n  atomic.store [order=seq_cst] %v, %p\n  %original = atomic.load i64, %p\n  %explicit = atomic.load [order=seq_cst] i64, %p\n  fence\n  fence [order=seq_cst]\n  return %explicit\n}",
    );
    let canonical = round_trip(&source);
    assert_eq!(canonical.matches("[order=seq_cst]").count(), 3);
    assert!(canonical.contains("atomic.load i64"));
}

#[test]
fn atomic_float_and_wide_integer_types_round_trip() {
    let source = module(
        "func module @Atomic.float(%p: ptr, %v: f32) -> f32 engine(native) {\nbb0(%p: ptr, %v: f32):\n  atomic.store [order=relaxed] %v, %p\n  %read = atomic.load [order=relaxed] f32, %p\n  return %read\n}\n\nfunc module @Atomic.wide(%p: ptr, %v: i128) -> i128 engine(native) {\nbb0(%p: ptr, %v: i128):\n  atomic.store %v, %p\n  %read = atomic.load i128, %p\n  return %read\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("atomic.load [order=relaxed] f32"));
    assert!(canonical.contains("atomic.load i128"));
}

#[test]
fn atomic_load_on_successor_block_round_trips() {
    let source = module(
        "func module @Atomic.second(%p: ptr, %v: i32) -> i32 engine(native) {\nbb0(%p: ptr, %v: i32):\n  atomic.store %v, %p\n  br bb1(%p)\nbb1(%addr: ptr):\n  %read = atomic.load [order=seq_cst] i32, %addr\n  return %read\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("atomic.load [order=seq_cst]"));
}

#[test]
fn incompatible_order_scope_and_type_are_rejected() {
    for (params, instruction) in [
        ("%p: ptr", "%v = atomic.load [order=release] i32, %p"),
        ("%p: ptr", "%v = atomic.load [order=acq_rel] i32, %p"),
        ("%p: ptr", "%v = atomic.load [scope=subgroup] i32, %p"),
        ("%p: ptr", "%v = atomic.load i1, %p"),
        (
            "%p: ptr",
            "%v = atomic.load [order=release] [order=relaxed] i32, %p",
        ),
        ("%p: ptr, %v: i32", "atomic.store [order=acquire] %v, %p"),
        ("%p: ptr, %v: i32", "atomic.store [order=acq_rel] %v, %p"),
        (
            "%p: ptr, %v: i32",
            "atomic.store [scope=threadgroup] %v, %p",
        ),
        ("%v: i32", "atomic.store %v, %v"),
        ("", "fence [order=invalid]"),
        ("", "fence [scope=thread] [scope=system]"),
        ("", "%value = fence"),
    ] {
        let source = module(&format!(
            "func module @Atomic.bad({params}) -> unit engine(native) {{\nbb0({params}):\n  {instruction}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid atomic instruction assembled: {instruction}"
            );
        }
    }
}

#[test]
fn atomic_wait_notify_result_ssa_values_round_trip() {
    let source = module(
        "func module @Atomic.sync(%ptr: ptr, %expected: i32, %timeout: i64, %count: i32) -> i32 engine(native) {\nbb0(%ptr: ptr, %expected: i32, %timeout: i64, %count: i32):\n  %status = atomic.wait %ptr, %expected, %timeout\n  %woken = atomic.notify %ptr, %count\n  %combined = add %status, %woken\n  return %combined\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains(" = atomic.wait %"));
    assert!(canonical.contains(" = atomic.notify %"));
}

#[test]
fn atomic_wait_on_later_block_round_trips() {
    let source = module(
        "func module @Atomic.wait_second(%ptr: ptr, %expected: i64, %timeout: i64) -> i32 engine(native) {\nbb0(%ptr: ptr, %expected: i64, %timeout: i64):\n  br bb1(%ptr, %expected, %timeout)\nbb1(%p: ptr, %e: i64, %t: i64):\n  %status = atomic.wait %p, %e, %t\n  return %status\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains("atomic.wait %"));
}

#[test]
fn atomic_wait_notify_reject_mistyped_arguments() {
    for (parameters, code) in [
        ("%p: ptr, %e: i16, %t: i64", "%r = atomic.wait %p, %e, %t"),
        ("%p: ptr, %e: i32, %t: i32", "%r = atomic.wait %p, %e, %t"),
        ("%p: ptr, %n: i64", "%r = atomic.notify %p, %n"),
        ("%p: i64, %n: i32", "%r = atomic.notify %p, %n"),
        ("%p: ptr, %n: i32", "atomic.notify %p, %n"),
    ] {
        let source = module(&format!(
            "func module @Atomic.invalid({parameters}) -> unit engine(native) {{\nbb0({parameters}):\n  {code}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "bad atomic operation encoded: {code}"
            );
        }
    }
}

#[test]
fn atomic_rmw_integer_and_float_operations_round_trip() {
    let source = module(
        "func module @Atomic.rmw(%p: ptr, %v: i32, %floatp: ptr, %f: f32) -> i32 engine(native) {\nbb0(%p: ptr, %v: i32, %floatp: ptr, %f: f32):\n  %old = atomic.rmw add [order=acq_rel] [scope=system] %p, %v\n  %floatold = atomic.rmw fadd [order=relaxed] %floatp, %f\n  %wrap = atomic.rmw uinc_wrap %p, %v\n  %result = add %old, %wrap\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    for mnemonic in [
        "atomic.rmw add [order=acq_rel] [scope=system]",
        "atomic.rmw fadd [order=relaxed]",
        "atomic.rmw uinc_wrap",
    ] {
        assert!(
            canonical.contains(mnemonic),
            "RMW spelling missing: {mnemonic}"
        );
    }
}

#[test]
fn weak_compare_exchange_tuple_and_failure_order_round_trip() {
    let source = module(
        "func module @Atomic.cmp(%p: ptr, %expected: i32, %new: i32) -> (i32, i1) engine(native) {\nbb0(%p: ptr, %expected: i32, %new: i32):\n  %result = atomic.cmpxchg [weak] [success_order=acq_rel] [failure_order=acquire] [scope=thread] %p, %expected, %new\n  return %result\n}",
    );
    let canonical = round_trip(&source);
    assert!(canonical.contains(
        "atomic.cmpxchg [weak] [success_order=acq_rel] [failure_order=acquire] [scope=thread]"
    ));
}

#[test]
fn compare_exchange_result_can_be_destructured_and_reused() {
    let source = module(
        "func module @Atomic.cmp(%p: ptr, %expected: i64, %new: i64) -> i64 engine(native) {\nbb0(%p: ptr, %expected: i64, %new: i64):\n  %result = atomic.cmpxchg %p, %expected, %new\n  %old = tuple.extract %result, 0\n  return %old\n}",
    );
    assert!(round_trip(&source).contains("tuple.extract %"));
}

#[test]
fn compare_exchange_rejects_too_strong_failure_and_mistyped_update() {
    for (params, operation) in [
        ("%p: ptr, %v: i32", "%r = atomic.rmw fadd %p, %v"),
        ("%p: ptr, %v: f32", "%r = atomic.rmw add %p, %v"),
        ("%p: ptr, %v: i32", "%r = atomic.rmw udec_wrap %p, %v"),
        ("%p: ptr, %v: i32", "%r = atomic.rmw usub_cond %p, %v"),
        (
            "%p: ptr, %v: i32",
            "%r = atomic.rmw add [order=unknown] %p, %v",
        ),
        (
            "%p: ptr, %v: i32",
            "%r = atomic.cmpxchg [success_order=release] [failure_order=acquire] %p, %v, %v",
        ),
        (
            "%p: ptr, %v: i32",
            "%r = atomic.cmpxchg [success_order=relaxed] [failure_order=seq_cst] %p, %v, %v",
        ),
        (
            "%p: ptr, %v: i32",
            "%r = atomic.cmpxchg [weak] [weak] %p, %v, %v",
        ),
        (
            "%p: ptr, %v: i32, %x: f32",
            "%r = atomic.cmpxchg %p, %v, %x",
        ),
    ] {
        let source = module(&format!(
            "func module @Atomic.invalid({params}) -> unit engine(native) {{\nbb0({params}):\n  {operation}\n  return\n}}"
        ));
        if let Ok(parsed) = parse_text_module(&source) {
            assert!(
                assemble_text_module(&parsed).is_err(),
                "invalid update assembled: {operation}"
            );
        }
    }
}
