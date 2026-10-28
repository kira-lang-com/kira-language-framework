//! Backend-parity suite (section 31): every KIR program in the corpus runs on
//! the VM and on native code, and the two outcomes must agree. This is the M2
//! exit test — the VM half is the oracle, already checked in `klf-vm`.

mod aggregates_corpus;
mod corpus;
mod indirect_corpus;
mod jump_corpus;
mod support;
mod unmanaged_corpus;
mod unwind_corpus;
mod variadic_corpus;
mod vector_corpus;
mod witness_corpus;

use corpus::*;
use klf_vm::{Outcome, run_module};
use support::native_run;

fn parity(source: &str, entry: &str, arguments: &[u64]) {
    let module = klf_kir::parse_module(source).expect("module parses");
    let vm = run_module(&module, entry, arguments).expect("module lowers on the VM");
    let native = native_run(source, entry, arguments);
    assert_eq!(
        vm, native,
        "VM and native disagree on `{entry}` with {arguments:?}"
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
fn reads_and_writes_a_global() {
    parity(GLOBALS, "read", &[]);
    parity(GLOBALS, "bump", &[]);
}

#[test]
fn initializers_run_before_the_entry() {
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
fn arithmetic_edges_and_traps() {
    parity(DIV_ZERO, "divzero", &[10]);
    parity(OVERFLOW, "overflow", &[]);
    parity(WRAP, "wrap", &[]);
    parity(EDGES, "saturate", &[]);
    parity(EDGES, "unsigned_shift", &[0xFFFF_FFFF, 4]);
    parity(EDGES, "spin", &[0]);
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
        Outcome::Trap("KLF-RT-0003".to_owned()),
        "a modify against an open read conflicts"
    );
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

const RIDL_EXPORT: &str = r#"module @Ridl lang "test" target "aarch64-apple-darwin" stage raw

func exported @scale(%x: i64, %k: i64) -> i64 engine(runtime) {
bb0(%x: i64, %k: i64):
  %r = mul %x, %k
  return %r
}

func module @internal_scale(%x: i64) -> i64 engine(runtime) abi(ridl) {
bb0(%x: i64):
  return %x
}

func private @context_scale(%ctx: context *i64, %x: i64) -> i64 engine(runtime) abi(ridl) {
bb0(%ctx: context *i64, %x: i64):
  %context_value = load [trivial] %ctx
  %r = add %context_value, %x
  return %r
}

func exported @compute(%x: i64) -> i64 engine(runtime) {
bb0(%x: i64):
  %two = const.int i64, 2
  %r = call @scale(%x, %two)
  %s = call @internal_scale(%r)
  %callback = func.ref @internal_scale
  %platform_callback = bitcast %callback to fn(i64) -> i64
  %indirect = call.indirect %platform_callback(%s) : (i64) -> i64
  %context_slot = stack.alloc i64
  store [trivial] %s to %context_slot
  %context_callback = func.ref @context_scale
  %context_result = call.indirect %context_callback(%context_slot, %indirect) : (*i64, i64) -> i64
  %platform_context_callback = bitcast %context_callback to fn(*i64, i64) -> i64
  %platform_context_result = call.indirect %platform_context_callback(%context_slot, %context_result) : (*i64, i64) -> i64
  stack.dealloc %context_slot
  return %platform_context_result
}
"#;

#[test]
fn ridl_convention_uses_swiftcc_and_matches_vm() {
    let parsed = klf_kir::parse_module(RIDL_EXPORT).expect("module parses");
    let module = klf_kir_passes::monomorphize(&parsed);
    let diagnostics = klf_kir_verify::verify_module(&module);
    assert!(
        diagnostics.is_empty(),
        "generated function-pointer thunk must verify: {}",
        diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    let ir = klf_llvm::emit_program(&module, "compute", &[21]).expect("module emits");
    assert!(
        ir.contains("define swiftcc i64 @\"kir.scale\""),
        "exported function should lower to swiftcc:\n{ir}"
    );
    assert!(
        ir.contains("call swiftcc i64 @\"kir.compute\""),
        "call to an exported function should use swiftcc:\n{ir}"
    );
    assert!(
        ir.contains("define swiftcc i64 @\"kir.internal_scale\""),
        "internal RIDL function should use swiftcc:\n{ir}"
    );
    assert!(
        ir.contains("call swiftcc i64 @\"kir.internal_scale\""),
        "call to an internal RIDL function should use swiftcc:\n{ir}"
    );
    assert_eq!(
        ir.matches("call swiftcc i64 @\"kir.internal_scale\"")
            .count(),
        2,
        "direct call and ABI thunk should call the internal RIDL function with swiftcc:\n{ir}"
    );
    assert!(
        ir.contains("abi_thunk_internal_scale"),
        "a platform-compatible function-pointer adapter should be emitted:\n{ir}"
    );
    assert!(
        ir.contains("call i64 @\"kir.__klf_abi_thunk_"),
        "the platform indirect call should target the adapter:\n{ir}"
    );
    assert!(
        ir.contains("call swiftcc i64 @\"kir.context_scale\"(ptr swiftself"),
        "the RIDL indirect call should pass its context parameter in the context register:\n{ir}"
    );
    assert!(
        ir.contains("abi_thunk_context_scale_platform"),
        "a platform callback should receive a context-forwarding thunk:\n{ir}"
    );
    parity(RIDL_EXPORT, "compute", &[21]);
}

#[test]
fn error_register_propagates_failure() {
    let module = klf_kir::parse_module(FAILING).expect("module parses");
    let ir = klf_llvm::emit_program(&module, "caller", &[0]).expect("module emits");
    assert!(
        ir.contains("ptr %arg.err") && ir.contains("@klfrt_error_box"),
        "a failing function should carry the error out-pointer:\n{ir}"
    );
    parity(FAILING, "caller", &[0]);
    parity(FAILING, "caller", &[5]);
}

#[test]
fn memory_and_counting() {
    parity(MEMORY, "stack_roundtrip", &[7]);
    parity(MEMORY, "heap_ok", &[]);
    parity(MEMORY, "retained", &[]);
    parity(MEMORY, "leak", &[]);
    parity(COUNTED, "counted_aliases", &[]);
    parity(COUNTED, "counted_last_reference", &[]);
    parity(COUNTED, "counted_many_objects", &[3]);
    parity(COUNTED, "counted_wide", &[]);
    parity(COUNTED, "counted_dropped_by_loop", &[64]);
    parity(COUNTED, "counted_aliased_deeply", &[8]);
}

/// A `pairing: counted` type is where section 16.1's inline retain and release
/// fast paths apply: the slot's metadata word is reached from the pointer.
pub const COUNTED: &str = r#"module @Counted lang "test" target "aarch64-unknown-linux-gnu" stage raw

type %Cell = struct ridl { n: i64 }
policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: static, pairing: counted }

type %Wide = struct ridl { data: [3000 x i64] }
policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: static, pairing: counted }

func private @counted_aliases() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Cell
  %addr = ref.addr %r
  %field = field.addr %addr, 0
  %v = const.int i64, 7
  store [trivial] %v to %field
  %alias = copy %r
  %kept = copy %alias
  destroy %alias
  %back = load [trivial] %field
  destroy %kept
  destroy %r
  return %back
}

func private @counted_dropped_by_loop(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %zero = const.int i64, 0
  %one = const.int i64, 1
  %sum = const.int i64, 0
  br bb1(%sum, %zero)
bb1(%acc: i64, %i: i64):
  %done = icmp sge %i, %n
  cond.br %done, bb2(%acc), bb3(%acc, %i)
bb2(%total: i64):
  return %total
bb3(%acc2: i64, %i2: i64):
  %cell = heap.alloc.counted %Cell
  %addr = ref.addr %cell
  %field = field.addr %addr, 0
  store [trivial] %i2 to %field
  destroy %cell
  %next = add %acc2, %i2
  %step = add %i2, %one
  br bb1(%next, %step)
}

func private @counted_aliased_deeply(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %cell = heap.alloc.counted %Cell
  %addr = ref.addr %cell
  %field = field.addr %addr, 0
  %one = const.int i64, 1
  store [trivial] %one to %field
  %a = copy %cell
  %b = copy %a
  %c = copy %b
  destroy %b
  destroy %c
  %alias = copy %a
  destroy %alias
  destroy %a
  %back = load [trivial] %field
  destroy %cell
  return %back
}

/// Past the largest pooled slot the runtime serves a dedicated run of spans,
/// whose header is at the run base rather than at `p & ~0xFFFF`, so this one
/// counts through the runtime.
func private @counted_wide() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Wide
  %addr = ref.addr %r
  %alias = copy %r
  destroy %alias
  destroy %r
  %z = const.int i64, 0
  return %z
}

func private @counted_last_reference() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Cell
  %addr = ref.addr %r
  %field = field.addr %addr, 0
  %v = const.int i64, 9
  store [trivial] %v to %field
  destroy %r
  %z = const.int i64, 0
  return %z
}

func private @counted_many_objects(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %zero = const.int i64, 0
  %one = const.int i64, 1
  %sum = const.int i64, 0
  br bb1(%sum, %zero)
bb1(%acc: i64, %i: i64):
  %done = icmp sge %i, %n
  cond.br %done, bb2(%acc), bb3(%acc, %i)
bb2(%total: i64):
  return %total
bb3(%acc2: i64, %i2: i64):
  %cell = heap.alloc.counted %Cell
  %addr = ref.addr %cell
  %field = field.addr %addr, 0
  store [trivial] %i2 to %field
  %alias = copy %cell
  destroy %alias
  %next = add %acc2, %i2
  %step = add %i2, %one
  destroy %cell
  br bb1(%next, %step)
}
"#;

/// The body of one function of an emitted module.
fn function_of(ir: &str, name: &str) -> String {
    let start = ir
        .find(&format!("@\"kir.{name}\""))
        .unwrap_or_else(|| panic!("{name} is emitted"));
    let body = &ir[start..];
    let end = body.find("\n}\n").expect("the function closes") + 3;
    body[..end].to_owned()
}

/// Section 16.1's inline fast paths apply to a counted `ref T` and to nothing
/// else: a `ref i64` has no counted payload and must keep calling the runtime.
#[test]
fn only_a_counted_reference_is_counted_inline() {
    let counted = klf_kir::parse_module(COUNTED).expect("module parses");
    let ir = klf_llvm::emit_module(&counted).expect("module emits");
    assert!(ir.contains("atomicrmw add"), "{ir}");
    assert!(ir.contains("atomicrmw sub"), "{ir}");
    assert!(
        ir.contains("and i64 %t3, -65536"),
        "the span base must clear the offset inside the span: {ir}"
    );
    assert!(ir.contains("call void @klfrt_retain"), "{ir}");
    assert!(ir.contains("call void @klfrt_release"), "{ir}");

    let ir = klf_llvm::emit_program(&counted, "counted_wide", &[]).expect("emits");
    let wide = function_of(&ir, "counted_wide");
    assert!(
        wide.contains("call void @klfrt_retain"),
        "a wide allocation must count through the runtime: {wide}"
    );
    assert!(
        !wide.contains("atomicrmw"),
        "a wide allocation must not count inline: {wide}"
    );
    let pooled = function_of(&ir, "counted_aliases");
    assert!(pooled.contains("atomicrmw"), "{pooled}");

    let memory = klf_kir::parse_module(MEMORY).expect("module parses");
    let ir = klf_llvm::emit_module(&memory).expect("module emits");
    assert!(ir.contains("call void @klfrt_retain"), "{ir}");
    assert!(
        !ir.contains("atomicrmw"),
        "an unmanaged reference must not count inline: {ir}"
    );
}

const KARC_FIB: &str = include_str!("../../klf-kir-passes/tests/fixtures/karc-fibonacci.kir");

#[test]
fn karc_frontend_output_runs_native() {
    parity(KARC_FIB, "smallcpp_main", &[]);
}

fn lowered_parity(source: &str, entry: &str, arguments: &[u64]) -> klf_vm::Outcome {
    let parsed = klf_kir::parse_module(source).expect("module parses");
    let module = klf_kir_passes::monomorphize(&parsed);
    let diagnostics = klf_kir_verify::verify_module(&module);
    assert!(
        diagnostics.is_empty(),
        "lowered module verifies: {diagnostics:?}"
    );
    let vm = run_module(&module, entry, arguments).expect("module lowers on the VM");
    assert_eq!(
        vm,
        native_run(source, entry, arguments),
        "VM and LLVM agree"
    );
    vm
}

#[test]
fn closures_call_directly_and_through_a_callback_thunk() {
    assert_eq!(
        lowered_parity(CLOSURES, "run", &[2]),
        klf_vm::Outcome::Return(Some(82))
    );
}

#[test]
fn failing_function_pointers_propagate_failure() {
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
fn aggregates_by_address_cross_an_abi_thunk() {
    let module =
        klf_kir_passes::monomorphize(&klf_kir::parse_module(AGGREGATE_ADAPTER).expect("parses"));
    assert!(module.to_string().contains("__klf_abi_thunk_swap_platform"));
    assert_eq!(
        lowered_parity(AGGREGATE_ADAPTER, "run", &[3, 42]),
        klf_vm::Outcome::Return(Some(42))
    );
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
fn set_and_long_jumps() {
    for argument in [3, u64::MAX] {
        parity(jump_corpus::JUMPS, "caught", &[argument]);
    }
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
    // "7|123456789012|2.50" and "7|-5|2.50"
    for (argument, length) in [(123_456_789_012_u64, 19), ((-5_i64).cast_unsigned(), 9)] {
        assert_eq!(
            native_run(variadic_corpus::FOREIGN_VARIADIC, "formatted", &[argument]),
            Outcome::Return(Some(length)),
        );
        parity(variadic_corpus::FOREIGN_VARIADIC, "formatted", &[argument]);
    }
}
