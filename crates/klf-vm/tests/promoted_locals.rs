use klf_kir::parse_module;
use klf_vm::{Instr, Opcode};
use klf_vm::{Outcome, lower_module, run};

const MODULE: &str = r#"module @Locals lang "test" target "aarch64-apple-darwin" stage raw
func private @sum(%n: i64) -> i64 engine(runtime) {
bb0(%n: i64):
  %slot = stack.alloc i64
  %zero = const.int i64, 0
  %one = const.int i64, 1
  store [trivial] %zero to %slot
  br bb1(%one)
bb1(%i: i64):
  %done = icmp sgt %i, %n
  cond.br %done, bb2, bb3
bb2:
  %result = load [trivial] %slot
  stack.dealloc %slot
  return %result
bb3:
  %acc = load [trivial] %slot
  %next = add %acc, %i
  store [trivial] %next to %slot
  %next_i = add %i, %one
  br bb1(%next_i)
}
func private @narrow(%v: i8) -> i8 engine(runtime) {
bb0(%v: i8):
  %slot = stack.alloc i8
  store [trivial] %v to %slot
  %result = load [trivial] %slot
  return %result
}
func private @read(%p: *i64) -> i64 engine(runtime) noinline {
bb0(%p: *i64):
  %v = load [trivial] %p
  return %v
}
func private @escape(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %slot = stack.alloc i64
  store [trivial] %v to %slot
  %result = call @read(%slot)
  stack.dealloc %slot
  return %result
}
func private @zero() -> i64 engine(runtime) {
bb0:
  %slot = stack.alloc i64
  %v = load [trivial] %slot
  return %v
}
func private @dead(%v: i64) -> i64 engine(runtime) {
bb0(%v: i64):
  %slot = stack.alloc i64
  store [trivial] %v to %slot
  return %v
}
"#;

#[test]
fn scalar_slots_become_registers_without_changing_memory_semantics() {
    let program = lower_module(&parse_module(MODULE).unwrap()).unwrap();
    for (name, arguments, result) in [
        ("sum", vec![100], 5050),
        ("narrow", vec![0x1234], 0x34),
        ("escape", vec![123], 123),
        ("zero", vec![], 0),
        ("dead", vec![456], 456),
    ] {
        let index = program.function_index(name).unwrap();
        assert_eq!(
            run(&program, index, &arguments),
            Outcome::Return(Some(result))
        );
        let allocates = program.functions[index]
            .code
            .iter()
            .any(|word| Instr::decode(*word).unwrap().op == Opcode::Alloca);
        assert_eq!(allocates, name == "escape");
        if name == "dead" {
            assert_eq!(program.functions[index].code.len(), 1);
        }
    }
}
