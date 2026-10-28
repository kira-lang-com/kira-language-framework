//! Backend-parity suite for the Cranelift backend: every corpus program runs on
//! the VM and on Cranelift-generated native code and must agree.

#[path = "../../klf-llvm/tests/aggregates_corpus.rs"]
mod aggregates_corpus;
#[allow(dead_code)]
#[path = "../../klf-llvm/tests/corpus.rs"]
mod corpus;
#[path = "../../klf-llvm/tests/indirect_corpus.rs"]
mod indirect_corpus;
#[path = "../../klf-llvm/tests/jump_corpus.rs"]
mod jump_corpus;
mod support;
#[path = "../../klf-llvm/tests/unmanaged_corpus.rs"]
mod unmanaged_corpus;
#[path = "../../klf-llvm/tests/unwind_corpus.rs"]
mod unwind_corpus;
#[path = "../../klf-llvm/tests/variadic_corpus.rs"]
mod variadic_corpus;
#[path = "../../klf-llvm/tests/vector_corpus.rs"]
mod vector_corpus;
#[path = "../../klf-llvm/tests/witness_corpus.rs"]
mod witness_corpus;

use corpus::*;
use klf_vm::run_module;
use support::cranelift_run;

fn parity(source: &str, entry: &str, arguments: &[u64]) {
    let module = klf_kir::parse_module(source).expect("module parses");
    let vm = run_module(&module, entry, arguments).expect("module lowers on the VM");
    let native = cranelift_run(source, entry, arguments);
    assert_eq!(
        vm, native,
        "VM and Cranelift disagree on `{entry}` with {arguments:?}"
    );
}

#[test]
fn arithmetic_and_control_flow() {
    parity(ARITHMETIC, "add", &[7, 5]);
    parity(MAX, "max", &[3, 9]);
    parity(MAX, "max", &[42, 7]);
    parity(SUM, "sumto", &[5]);
    parity(SUM, "sumto", &[100]);
    parity(SWITCH, "classify", &[0]);
    parity(SWITCH, "classify", &[1]);
    parity(SWITCH, "classify", &[2]);
    parity(SWITCH, "classify", &[9]);
}

#[test]
fn try_call_splits_on_the_error() {
    parity(TRY_CALL, "caller", &[5]);
    parity(TRY_CALL, "caller", &[0]);
    parity(TRY_CALL, "relay", &[0]);
    parity(TRY_CALL, "relay", &[3]);
    parity(TRY_CALL, "forward", &[0]);
}

#[test]
fn address_ownership_matches_between_the_backends() {
    parity(OWNERSHIP, "copy_addr_retains", &[]);
    parity(OWNERSHIP, "copy_addr_assign_destroys_first", &[]);
    parity(OWNERSHIP, "move_addr_does_not_retain", &[]);
}

#[test]
fn dynamic_exclusivity_matches_between_the_backends() {
    parity(ACCESS, "two_reads", &[]);
    parity(ACCESS, "read_then_modify", &[]);
    parity(ACCESS, "static_access_is_free", &[]);

    let module = klf_kir::parse_module(ACCESS).expect("module parses");
    assert_eq!(
        run_module(&module, "read_then_modify", &[]).expect("lowers on the VM"),
        klf_vm::Outcome::Trap("KLF-RT-0003".to_owned()),
        "a modify against an open read conflicts"
    );
}

#[test]
fn reads_and_writes_a_global() {
    parity(GLOBALS, "read", &[]);
    parity(GLOBALS, "bump", &[]);
}

#[test]
fn initializers_run_before_the_entry() {
    let module = klf_kir::parse_module(INITIALIZER).expect("module parses");
    let vm = run_module(&module, "read", &[]).expect("module lowers on the VM");
    assert_eq!(vm, klf_vm::Outcome::Return(Some(42)));
    parity(INITIALIZER, "read", &[]);
}

#[test]
fn materializes_a_string_constant() {
    parity(STRING_CONSTANT, "length", &[]);
}

#[test]
fn calls_through_a_function_pointer() {
    parity(INDIRECT, "apply", &[0, 21]);
    parity(INDIRECT, "apply", &[1, 10]);
}

#[test]
fn floats_and_conversions() {
    parity(FLOAT, "fadd", &[]);
    parity(FCMP, "below", &[1.5f64.to_bits()]);
    parity(FCMP, "below", &[5.0f64.to_bits()]);
    parity(CONVERT, "trunc_wide", &[300]);
    parity(CONVERT, "sext_byte", &[255]);
    parity(CONVERT, "zext_byte", &[255]);
    parity(CONVERT, "to_float", &[7]);
    parity(CONVERT, "from_float", &[3.9f64.to_bits()]);
}

#[test]
fn failure_propagates() {
    parity(FAILING, "caller", &[0]);
    parity(FAILING, "caller", &[5]);
}

#[test]
fn memory_and_counting() {
    parity(MEMORY, "stack_roundtrip", &[7]);
    parity(MEMORY, "heap_ok", &[]);
    parity(MEMORY, "retained", &[]);
    parity(MEMORY, "leak", &[]);
}

const KARC_FIB: &str = include_str!("../../klf-kir-passes/tests/fixtures/karc-fibonacci.kir");

#[test]
fn karc_frontend_output_runs_cranelift() {
    parity(KARC_FIB, "smallcpp_main", &[]);
}

fn lowered_parity(source: &str, entry: &str, arguments: &[u64]) -> klf_vm::Outcome {
    let parsed = klf_kir::parse_module(source).expect("module parses");
    let module = klf_kir_passes::monomorphize(&parsed);
    let vm = run_module(&module, entry, arguments).expect("module lowers on the VM");
    assert_eq!(
        vm,
        cranelift_run(source, entry, arguments),
        "VM and Cranelift agree"
    );
    vm
}

#[test]
fn closures_and_failing_function_pointers() {
    assert_eq!(
        lowered_parity(CLOSURES, "run", &[2]),
        klf_vm::Outcome::Return(Some(82))
    );
    assert_eq!(
        lowered_parity(FAILING_INDIRECT, "caller", &[0]),
        klf_vm::Outcome::Fail(99)
    );
    assert_eq!(
        lowered_parity(FAILING_INDIRECT, "caller", &[5]),
        klf_vm::Outcome::Return(Some(5))
    );
}

#[test]
fn bulk_zero_fill_preserves_empty_odd_and_complete_ranges() {
    const SOURCE: &str = r#"module @Fill lang "test" target "aarch64-apple-darwin" stage raw
func private @fill(%length: i64) -> i64 engine(runtime) {
bb0(%length: i64):
  %size = const.int i64, 17
  %align = const.int i64, 8
  %sentinel = const.int i8, 165
  %zero = const.int i8, 0
  %pointer = heap.alloc.raw %size, %align
  mem.set %pointer, %sentinel, %size
  mem.set %pointer, %zero, %length
  %head_address = bitcast %pointer to *i8
  %head = load [trivial] %head_address
  %wide_head = zext %head to i64
  %full = icmp eq %length, %size
  cond.br %full, bb1, bb2
bb1:
  %last = const.int i64, 16
  br bb3(%last)
bb2:
  br bb3(%length)
bb3(%index: i64):
  %array_address = bitcast %pointer to *[17 x i8]
  %tail_pointer = elem.addr %array_address, %index
  %tail = load [trivial] %tail_pointer
  %wide_tail = zext %tail to i64
  %sum = add %wide_head, %wide_tail
  heap.free.raw %pointer
  return %sum
}
"#;
    let module = klf_kir::parse_module(SOURCE).unwrap();
    for (length, expected) in [(0, 330), (1, 165), (7, 165), (9, 165), (17, 0)] {
        let vm = run_module(&module, "fill", &[length]).unwrap();
        assert_eq!(vm, klf_vm::Outcome::Return(Some(expected)));
        assert_eq!(cranelift_run(SOURCE, "fill", &[length]), vm);
    }
}

#[test]
fn atomics_and_fences() {
    for seed in [0, 5, 1 << 40, u64::MAX] {
        parity(ATOMICS, "wide", &[seed]);
        parity(ATOMICS, "narrow", &[seed]);
        parity(ATOMICS, "exchange", &[seed]);
    }
}

#[test]
fn aggregate_values() {
    let cases: [(&str, u64); 8] = [
        ("pairs", 7),
        ("scaled", 4),
        ("tuples", 5),
        ("arrays", 8),
        ("shapes", 0),
        ("shapes", 6),
        ("fibonacci", 50),
        ("stored", 4),
    ];
    for (entry, argument) in cases {
        parity(aggregates_corpus::AGGREGATES, entry, &[argument]);
    }
}

#[test]
fn unmanaged_values() {
    let cases: [(&str, u64); 11] = [
        ("absolute", 42),
        ("checked", 3),
        ("checked", 1 << 32),
        ("picks", 3),
        ("picks", 20),
        ("fused", 2),
        ("fused", 0),
        ("raw", 4),
        ("sizes", 0),
        ("borrowed", 21),
        ("unowned", 41),
    ];
    for (entry, argument) in cases {
        parity(unmanaged_corpus::UNMANAGED, entry, &[argument]);
    }
}

#[test]
fn vector_lanes() {
    for (entry, argument) in [("lanes", 4), ("lanes", 10), ("clamped", 3), ("clamped", 20)] {
        parity(vector_corpus::VECTORS, entry, &[argument]);
    }
}

#[test]
fn indirect_branches() {
    for argument in [0, 1, 5] {
        parity(indirect_corpus::INDIRECT, "dispatch", &[argument]);
    }
}

#[test]
fn set_jump_is_refused_with_its_reason() {
    let module = klf_kir::parse_module(jump_corpus::JUMPS).expect("module parses");
    let error = klf_cranelift::emit_object(&module, "caught", &[3]).expect_err("refused");
    assert!(
        error.to_string().contains("`setjmp` returns twice"),
        "{error}"
    );
}

#[test]
fn concrete_witness_calls() {
    parity(witness_corpus::WITNESS, "describe", &[20]);
}

#[test]
fn unwinding_without_a_handler() {
    for argument in [0, 1] {
        parity(unwind_corpus::UNWIND, "guarded", &[argument]);
    }
}

#[test]
fn ridl_variadics() {
    for (entry, argument) in [("summed", 4), ("summed", 0), ("mixes", 3), ("mixes", 10)] {
        parity(variadic_corpus::VARIADIC, entry, &[argument]);
    }
}

#[test]
fn a_c_imports_variadic_tail_follows_the_platform() {
    for argument in [123_456_789_012_u64, (-5_i64).cast_unsigned()] {
        parity(variadic_corpus::FOREIGN_VARIADIC, "formatted", &[argument]);
    }
}

const THREAD_LOCALS: &str = r#"module @Tls lang "test" target "aarch64-apple-darwin" stage raw

global module @wide : i64 = zero tls
global module @narrow : i32 = zero tls

func module @tls() -> i64 engine(runtime) {
bb0():
  %t0 = global.addr @wide
  %t1 = bitcast %t0 to *i64
  %t2 = global.addr @narrow
  %t3 = bitcast %t2 to *i32
  %t4 = const.int i64, 81985529216486895
  store [assign] %t4 to %t1
  %t5 = const.int i32, 5
  store [assign] %t5 to %t3
  %t6 = load [trivial] %t1
  return %t6
}
"#;

#[test]
fn zero_initialized_thread_locals_do_not_overlap() {
    parity(THREAD_LOCALS, "tls", &[]);
}
