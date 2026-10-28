//! Aggregates (section 12.6): `field.addr` and `elem.addr` compute field and
//! element addresses from a stack-allocated struct/array, and load/store through
//! them. The VM and native backends must agree.

mod support;

use klf_vm::{Outcome, run_module};
use support::cranelift_run;

const ENUM: &str = r#"module @Enum lang "c" target "aarch64-apple-darwin" stage canonical

type %Value = enum ridl { Empty, Number(i64) }
policy { copy: none, destroy: trivial, relocate: bitwise, exclusivity: none, pairing: none }

func module @main() -> i64 engine(native) {
bb0():
  %v = stack.alloc %Value
  %n = const.int i64, 42
  %payload = enum.payload.addr %v, 1
  store [init] %n to %payload
  enum.inject %v, 1
  %tag = enum.tag %v
  switch.enum %v, [0: bb_empty(), 1: bb_number(%payload)]
bb_empty():
  %zero = const.int i64, 0
  return %zero
bb_number(%p: *i64):
  %number = load [trivial] %p
  return %number
}
"#;

fn parity(source: &str, expected: u64) {
    let module = klf_kir::parse_module(source).expect("module parses");
    let diagnostics = klf_kir_verify::verify_module(&module);
    assert!(diagnostics.is_empty(), "verifies: {diagnostics:?}");
    let vm = run_module(&module, "main", &[]).expect("runs on VM");
    assert_eq!(vm, Outcome::Return(Some(expected)), "VM result");
    let native = cranelift_run(source, "main", &[]);
    assert_eq!(native, vm, "native and VM agree");
}

#[test]
fn enum_payload_and_match() {
    parity(ENUM, 42);
}
