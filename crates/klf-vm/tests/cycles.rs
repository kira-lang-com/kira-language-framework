//! The debug cycle detector on the VM (section 18.6): a run that leaks a
//! cycle of counted objects fails with `KLF-RT-0101` under `KLF_CYCLES=error`
//! and keeps the heap-balance trap `KLF-RT-0100` otherwise.

use klf_vm::{Capabilities, Cycles, Outcome, lower_module_traced, run_traced};

const NODES: &str = r#"module @Nodes lang "c" target "aarch64-apple-darwin" stage canonical

type %Node = struct ridl { next: ref %Node, value: i64 }
policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

func module @self_loop() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Node
  %a = ref.addr %r
  %f = field.addr %a, 0
  %c = copy %r
  store [init] %c to %f
  destroy %r
  %z = const.int i64, 0
  return %z
}

func module @pair() -> i64 engine(runtime) {
bb0():
  %a = heap.alloc.counted %Node
  %b = heap.alloc.counted %Node
  %pa = ref.addr %a
  %fa = field.addr %pa, 0
  %cb = copy %b
  store [init] %cb to %fa
  %pb = ref.addr %b
  %fb = field.addr %pb, 0
  %ca = copy %a
  store [init] %ca to %fb
  destroy %a
  destroy %b
  %z = const.int i64, 0
  return %z
}

func module @plain_leak() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Node
  %z = const.int i64, 0
  return %z
}

func module @balanced() -> i64 engine(runtime) {
bb0():
  %r = heap.alloc.counted %Node
  destroy %r
  %z = const.int i64, 7
  return %z
}
"#;

fn run(entry: &str, cycles: Cycles) -> Outcome {
    let module = klf_kir::parse_module(NODES).expect("module parses");
    let (program, traces) = lower_module_traced(&module).expect("module lowers");
    assert!(!traces.is_empty(), "every allocation of `Node` is traced");
    let index = program.function_index(entry).expect("entry exists");
    run_traced(&program, &traces, index, &[], &Capabilities::new(), cycles)
}

fn trap(code: &str) -> Outcome {
    Outcome::Trap(code.to_owned())
}

#[test]
fn self_loop_is_a_leaked_cycle() {
    assert_eq!(run("self_loop", Cycles::Error), trap("KLF-RT-0101"));
    assert_eq!(run("self_loop", Cycles::Warn), trap("KLF-RT-0100"));
    assert_eq!(run("self_loop", Cycles::Off), trap("KLF-RT-0100"));
}

#[test]
fn two_node_cycle_is_a_leaked_cycle() {
    assert_eq!(run("pair", Cycles::Error), trap("KLF-RT-0101"));
}

#[test]
fn a_leak_without_a_cycle_keeps_the_balance_trap() {
    assert_eq!(run("plain_leak", Cycles::Error), trap("KLF-RT-0100"));
}

#[test]
fn a_balanced_run_returns() {
    assert_eq!(run("balanced", Cycles::Error), Outcome::Return(Some(7)));
}

#[test]
fn an_untraced_lowering_reports_no_cycles() {
    let module = klf_kir::parse_module(NODES).expect("module parses");
    let program = klf_vm::lower_module(&module).expect("module lowers");
    let index = program.function_index("pair").expect("entry exists");
    let outcome = run_traced(
        &program,
        &klf_vm::Traces::new(),
        index,
        &[],
        &Capabilities::new(),
        Cycles::Error,
    );
    assert_eq!(outcome, trap("KLF-RT-0100"));
}
